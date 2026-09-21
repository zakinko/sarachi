//! 一周する導入処理。切って、三つの領域を埋めて、root だけ台ごとの鍵で暗号化する。
//!
//! 三つを別々の署名付きイメージにしているのは、回復環境に mkfs を持ち込まずに
//! 済ませるため。ESP も回復領域も root も、どれもファイルシステムの像なので、
//! 検証して書けばそのまま使える。OS ごとに mkfs.vfat / mkfs.ext4 / newfs /
//! newfs_msdos を揃える必要がなくなる。
//!
//! **鍵は必ずここで作る。** イメージに鍵が入っていると全台が同じマスター鍵を
//! 持つことになり、一台で鍵を破棄しても同じイメージを持つ者は誰でも復号できる。
//! LUKS のマスター鍵は後から変更できないので、配ってしまってからでは
//! 取り返しがつかない。
//!
//! 未解決: 作った鍵をどこに預けるか。実運用では TPM に封じるか、control plane に
//! 預けるか、パスフレーズを人が入れるかのいずれかになる。今はファイルに書き出す
//! だけで、**これは本番の姿ではない**。

use crate::{crypt, fetch, wipe};
use anyhow::{Context, Result, bail};
use sarachi_disk::{Layout, Partition, Role, RootKind, write_gpt};
use sarachi_image::{Manifest, SignedManifest, VerifyingKey, verify_and_write};
use std::fs::OpenOptions;
use std::path::Path;

const MAPPER_NAME: &str = "sarachi-root";
const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;

/// 配布物の置き場。`<base>/<name>.img` と `<base>/<name>.manifest` を読む。
pub struct Bundle<'a> {
    pub base: &'a str,
    /// 実 OS と回復環境の双方のローダが入った ESP の像。
    pub esp: Option<&'a str>,
    /// 回復環境そのものの像。**これを書くと、その機体は以後自分で回復できる。**
    pub recovery: Option<&'a str>,
    /// 暗号コンテナの中へ入る root の像。
    pub rootfs: &'a str,
}

pub struct Plan<'a> {
    pub disk: &'a Path,
    pub size_bytes: u64,
    pub sector_size: u64,
    /// root に何を載せるか。型 GUID と暗号層の両方がこれで決まる。
    pub root_kind: RootKind,
    pub bundle: Bundle<'a>,
    /// 作った鍵の書き出し先。本番では TPM か control plane へ。
    pub key_out: &'a Path,
    pub commit: bool,
    /// **試験用。** 作った鍵を 16 進でコンソールへ出す。
    /// 本番でこれを立ててはいけない。回復環境の出力がどこへ流れるか分からない。
    pub print_key: bool,
}

struct Piece {
    name: String,
    manifest: Manifest,
    role: Role,
}

fn fetch_manifest(base: &str, name: &str, trusted: &VerifyingKey) -> Result<Manifest> {
    let url = format!("{base}/{name}.manifest");
    let raw = fetch::get_all(&url, MANIFEST_LIMIT)?;
    let signed = SignedManifest::decode(&raw)
        .with_context(|| format!("{url} をマニフェストとして読めない"))?;
    let m = signed
        .verify(trusted)
        .with_context(|| format!("{url} の署名検証に失敗"))?;
    Ok(m.clone())
}

fn fits(m: &Manifest, p: &Partition, sector_size: u64) -> Result<()> {
    let cap = p.sectors() * sector_size;
    if m.total_size > cap {
        bail!(
            "{:?} 向けの像 {} MiB が領域 {} MiB に入らない",
            p.role,
            m.total_size / 1024 / 1024,
            cap / 1024 / 1024
        );
    }
    Ok(())
}

fn write_into(base: &str, piece: &Piece, dev: &Path) -> Result<u64> {
    let url = format!("{}/{}.img", base, piece.name);
    let got = fetch::get_from(&url, 0)?;
    let mut src = got.reader;
    let mut dst = OpenOptions::new()
        .write(true)
        .open(dev)
        .with_context(|| format!("{} を開けない", dev.display()))?;
    let stats = verify_and_write(&piece.manifest, &mut src, &mut dst, 0)?;
    dst.sync_all()?;
    Ok(stats.bytes_written)
}

pub fn run(plan: &Plan, trusted: &VerifyingKey) -> Result<()> {
    let layout = Layout::plan(plan.size_bytes, plan.sector_size, plan.root_kind)?;
    print!("{}", layout.describe());

    // **ディスクに触る前に、確かめられることを全て確かめる。**
    // 切ってから「イメージが偽物でした」では遅い。取り返しのつかない操作は、
    // 取り返しのつく検証を全て終えてから。
    println!("配布物を取得して署名を確かめる: {}", plan.bundle.base);
    let mut pieces = Vec::new();
    for (name, role) in [
        (plan.bundle.esp, Role::Esp),
        (plan.bundle.recovery, Role::Recovery),
        (Some(plan.bundle.rootfs), Role::Root),
    ] {
        let Some(name) = name else { continue };
        let m = fetch_manifest(plan.bundle.base, name, trusted)?;
        let part = layout.get(role).context("領域が無い")?;
        fits(&m, part, plan.sector_size)?;
        println!(
            "  {:<9} {:<28} {} MiB / {} チャンク  署名 OK",
            format!("{:?}", role),
            m.name,
            m.total_size / 1024 / 1024,
            m.chunks.len()
        );
        pieces.push(Piece {
            name: name.to_string(),
            manifest: m,
            role,
        });
    }

    if !plan.commit {
        println!("\ndry-run。実際に書くには --commit を付けること");
        return Ok(());
    }

    println!("\n1. GPT を書く");
    {
        let mut f = OpenOptions::new()
            .write(true)
            .open(plan.disk)
            .with_context(|| format!("{} を開けない", plan.disk.display()))?;
        write_gpt(&layout, &mut f)?;
        f.sync_all()?;
    }
    reread_partitions(plan.disk);

    // 暗号化しない領域を先に埋める。ESP と回復領域には秘密が入らないので
    // 暗号化しない。回復領域を暗号化すると、鍵を失った機体が自分で
    // 回復できなくなり、回復環境の目的そのものと衝突する。
    for piece in pieces.iter().filter(|p| p.role != Role::Root) {
        let part = layout.get(piece.role).unwrap();
        let dev = wipe::part_path(plan.disk, part.index);
        settle(&dev)?;
        println!("2. {:?} へ書く（{}）", piece.role, dev.display());
        let n = write_into(plan.bundle.base, piece, &dev)?;
        println!("   {} MiB", n / 1024 / 1024);
    }

    let rootfs = pieces
        .iter()
        .find(|p| p.role == Role::Root)
        .context("root の像が無い")?;
    let root_part = layout.get(Role::Root).unwrap();
    let root_dev = wipe::part_path(plan.disk, root_part.index);
    settle(&root_dev)?;

    let cr = crypt::for_kind(plan.root_kind)?;
    println!(
        "3. 台ごとの鍵で {} を作る（{}）",
        cr.name(),
        root_dev.display()
    );
    let key = crypt::generate_key()?;
    cr.format(&root_dev, &key)?;
    std::fs::write(plan.key_out, &key)
        .with_context(|| format!("鍵を {} に書けない", plan.key_out.display()))?;
    println!(
        "   鍵を {} に置いた（本番では TPM か control plane へ）",
        plan.key_out.display()
    );
    if plan.print_key {
        println!(
            "   [試験用] 鍵: {}",
            key.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }

    println!("4. 開いて、検証しながら中へ書く");
    let mapped = cr.open(&root_dev, &key, MAPPER_NAME)?;
    let n = write_into(plan.bundle.base, rootfs, &mapped)?;
    println!("   {} MiB", n / 1024 / 1024);

    println!("5. 閉じる");
    cr.close(MAPPER_NAME)?;

    println!("\n導入が完了した。");
    println!("  root は台ごとに異なる鍵で暗号化されている。");
    if plan.bundle.recovery.is_some() {
        println!("  回復領域が入ったので、この機体は以後、自分で回復できる。");
    }
    Ok(())
}

/// カーネルに新しいパーティション表を読ませる。これをしないと
/// `/dev/<disk>N` が現れず、次の段で開けない。
fn reread_partitions(disk: &Path) {
    for prog in ["blockdev", "busybox"] {
        let mut c = std::process::Command::new(prog);
        if prog == "busybox" {
            c.arg("blockdev");
        }
        let _ = c.arg("--rereadpt").arg(disk).status();
    }
}

/// デバイスノードが現れるのを待つ。devtmpfs は非同期に作るので、
/// 直後に開くと ENOENT になることがある。
fn settle(dev: &Path) -> Result<()> {
    for _ in 0..100 {
        if dev.exists() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    bail!(
        "{} が現れない。パーティション表の再読み込みが効いていない",
        dev.display()
    )
}
