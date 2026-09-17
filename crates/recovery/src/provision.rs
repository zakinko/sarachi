//! 一周する導入処理。切って、鍵を作って、中へ書く。
//!
//! **鍵は必ずここで作る。** イメージに鍵が入っていると全台が同じマスター鍵を
//! 持つことになり、crypto-erase が成立しなくなる。LUKS のマスター鍵は後から
//! 変更できないので、配ってしまってからでは取り返しがつかない。
//!
//! 未解決: 作った鍵をどこに預けるか。実運用では TPM に封じるか、
//! control plane に預けるか、パスフレーズを人が入れるかのいずれかになる。
//! 今はファイルに書き出すだけで、**これは本番の姿ではない**。

use crate::{crypt, fetch, wipe};
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::path::Path;
use unix_mdm_disk::{Layout, Role, write_gpt};
use unix_mdm_image::{SignedManifest, VerifyingKey, verify_and_write};

const MAPPER_NAME: &str = "unixmdm-root";

pub struct Plan<'a> {
    pub disk: &'a Path,
    pub size_bytes: u64,
    pub sector_size: u64,
    pub manifest_url: &'a str,
    pub image_url: &'a str,
    /// 作った鍵の書き出し先。本番では TPM か control plane へ。
    pub key_out: &'a Path,
    pub commit: bool,
    /// **試験用。** 作った鍵を 16 進でコンソールへ出す。
    /// 本番でこれを立てては駄目で、シリアルコンソールのログに鍵が残る。
    /// 回復環境の出力はどこへ流れるか分からない。
    pub print_key: bool,
}

pub fn run(plan: &Plan, trusted: &VerifyingKey) -> Result<()> {
    let layout = Layout::plan(plan.size_bytes, plan.sector_size)?;
    let root_part = layout.get(Role::Root).context("root パーティションが無い")?;
    let root_dev = wipe::part_path(plan.disk, root_part.index);

    print!("{}", layout.describe());

    // 先に署名を確かめる。ディスクを切ってから「イメージが偽物でした」では
    // 遅い。取り返しのつかない操作は、取り返しのつく検証を全て終えてから。
    println!("マニフェスト: {}", plan.manifest_url);
    let raw = fetch::get_all(plan.manifest_url, 4 * 1024 * 1024)?;
    let signed = SignedManifest::decode(&raw)?;
    let m = signed.verify(trusted).context("マニフェストの署名検証に失敗")?;
    println!("  署名: 検証を通った / {} / {} MiB（{} チャンク）",
             m.name, m.total_size / 1024 / 1024, m.chunks.len());

    if m.total_size > root_part.sectors() * plan.sector_size {
        bail!(
            "イメージ {} MiB が root 領域 {} MiB に入らない",
            m.total_size / 1024 / 1024,
            root_part.sectors() * plan.sector_size / 1024 / 1024
        );
    }

    if !plan.commit {
        println!("\ndry-run。実際に書くには --commit を付けること");
        return Ok(());
    }

    println!("\n1. GPT を書く");
    {
        let mut f = OpenOptions::new().write(true).open(plan.disk)
            .with_context(|| format!("{} を開けない", plan.disk.display()))?;
        write_gpt(&layout, &mut f)?;
    }
    // カーネルに新しいパーティション表を読ませる。これをしないと
    // /dev/<disk>3 が現れず、次の段で開けない。
    let _ = std::process::Command::new("busybox").args(["blockdev", "--rereadpt"])
        .arg(plan.disk).status();
    settle(&root_dev)?;

    println!("2. 台ごとの鍵で LUKS2 を作る");
    let key = crypt::generate_key()?;
    crypt::format(&root_dev, &key)?;
    std::fs::write(plan.key_out, &key)
        .with_context(|| format!("鍵を {} に書けない", plan.key_out.display()))?;
    println!("   鍵を {} に置いた（本番では TPM か control plane へ）", plan.key_out.display());
    if plan.print_key {
        // 試験用の経路。本番で通ってはいけない。
        println!("   [試験用] 鍵: {}", key.iter().map(|b| format!("{b:02x}")).collect::<String>());
    }

    println!("3. 開く");
    crypt::open(&root_dev, &key, MAPPER_NAME)?;
    let mapped = Path::new("/dev/mapper").join(MAPPER_NAME);

    println!("4. 検証しながら中へ書く");
    let got = fetch::get_from(plan.image_url, 0)?;
    let mut src = got.reader;
    let mut dst = OpenOptions::new().write(true).open(&mapped)
        .with_context(|| format!("{} を開けない", mapped.display()))?;
    let stats = verify_and_write(m, &mut src, &mut dst, 0)?;
    drop(dst);

    println!("   {} チャンク / {} MiB を書いた", stats.chunks_written, stats.bytes_written / 1024 / 1024);

    println!("5. 閉じる");
    crypt::close(MAPPER_NAME)?;

    println!("\n導入が完了した。root は台ごとに異なる鍵で暗号化されている。");
    Ok(())
}

/// デバイスノードが現れるのを待つ。devtmpfs は非同期に作るので、
/// 直後に開くと ENOENT になることがある。
fn settle(dev: &Path) -> Result<()> {
    for _ in 0..50 {
        if dev.exists() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    bail!("{} が現れない", dev.display())
}
