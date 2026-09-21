//! 消去の実行。
//!
//! 段階は Windows の RemoteWipe CSP に合わせてある。Windows が doWipe と
//! doWipeProtected を分けているのは、前者が電源を入れ直すだけで回避できる
//! ためで、紛失・盗難時には後者を使えと仕様に明記されている。
//! 回復領域を残すかどうかも段階から決まる。第 1 段と第 2 段は消してから
//! 入れ直すので入れ直す主体が要り、第 3 段は入れ直さないので消してよい。

use crate::crypt;
use anyhow::{Context, Result};
use sarachi_disk::{Layout, Role, RootKind};
use sarachi_order::Level;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// パーティション名を作る。`/dev/sda` なら `/dev/sda3`、
/// `/dev/nvme0n1` や `/dev/mmcblk0` なら `p` を挟む。
pub fn part_path(disk: &Path, index: u32) -> PathBuf {
    let s = disk.to_string_lossy();
    let last = s.chars().last().unwrap_or(' ');
    if last.is_ascii_digit() {
        PathBuf::from(format!("{s}p{index}"))
    } else {
        PathBuf::from(format!("{s}{index}"))
    }
}

pub struct Report {
    pub crypto_erased: bool,
    pub gpt_destroyed: bool,
    pub recovery_destroyed: bool,
}

/// 先頭と末尾を潰す。GPT の主ヘッダと予備ヘッダの両方を消さないと、
/// 片方から復元されてパーティションが戻ってしまう。
fn destroy_gpt(disk: &Path, size_bytes: u64) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .open(disk)
        .with_context(|| format!("{} を開けない", disk.display()))?;
    let zero = vec![0u8; 1024 * 1024];

    f.seek(SeekFrom::Start(0))?;
    f.write_all(&zero)?;

    // 末尾 1MiB。予備 GPT ヘッダはここにある。
    if size_bytes > 2 * 1024 * 1024 {
        f.seek(SeekFrom::Start(size_bytes - zero.len() as u64))?;
        f.write_all(&zero)?;
    }
    f.flush()?;
    Ok(())
}

/// 回復領域の先頭を潰す。ここには我々のカーネルと initramfs が入っている。
/// 第 3 段でだけ消す。
fn destroy_recovery(disk: &Path, layout: &Layout) -> Result<()> {
    let Some(rec) = layout.get(Role::Recovery) else {
        return Ok(());
    };
    let mut f = OpenOptions::new().write(true).open(disk)?;
    f.seek(SeekFrom::Start(rec.first_lba * layout.sector_size))?;
    // 先頭 4MiB を潰せばファイルシステムの上部構造は壊れる。全面を上書き
    // しないのは、そこに意味が無いため。SSD と SD では上書きが消去として
    // 成立しないし、回復領域にユーザのデータは無い。
    f.write_all(&vec![0u8; 4 * 1024 * 1024])?;
    f.flush()?;
    Ok(())
}

pub fn run(
    disk: &Path,
    size_bytes: u64,
    sector_size: u64,
    root_kind: RootKind,
    level: Level,
    commit: bool,
) -> Result<Report> {
    let layout = Layout::plan(size_bytes, sector_size, root_kind)?;
    let root = part_path(disk, layout.get(Role::Root).map(|p| p.index).unwrap_or(3));

    println!("段階   : {:?}", level);
    println!(
        "回復領域: {}",
        if level.keeps_recovery() {
            "残す"
        } else {
            "消す"
        }
    );
    println!(
        "妨害耐性: {}",
        if level.persists() {
            "終わるまで再試行"
        } else {
            "中断されたら止まる"
        }
    );
    println!("対象    : {}", root.display());

    if !commit {
        println!("\ndry-run。実際に消すには --commit を付けること");
        return Ok(Report {
            crypto_erased: false,
            gpt_destroyed: false,
            recovery_destroyed: false,
        });
    }

    let mut r = Report {
        crypto_erased: false,
        gpt_destroyed: false,
        recovery_destroyed: false,
    };

    // 本体。鍵を破棄すればマスター鍵は復元できない。
    let cr = crypt::for_kind(root_kind)?;
    if cr.is_container(&root) {
        cr.erase(&root)?;
        r.crypto_erased = true;
        println!("crypto-erase: 完了（{} / {}）", cr.name(), root.display());
    } else {
        // 暗号化されていない機体では、鍵の破棄という手が使えない。
        // 黙って成功を返すと消したつもりで消えていないので、はっきり言う。
        println!(
            "crypto-erase: 対象が {} の容器ではない。鍵の破棄では消せない",
            cr.name()
        );
    }

    if !level.keeps_recovery() {
        // 鍵を破棄した時点でデータは復元不能だが、ヘッダが残っていると
        // 「ここに暗号化された何かが在った」と言い続ける。この段は
        // 何も残さないための段なので、痕跡も消す。
        if let Some(rootp) = layout.get(Role::Root) {
            let mut f = OpenOptions::new().write(true).open(disk)?;
            f.seek(SeekFrom::Start(rootp.first_lba * layout.sector_size))?;
            // LUKS2 のヘッダ領域は既定 16MiB。その先頭を潰せば足りる。
            f.write_all(&vec![0u8; 4 * 1024 * 1024])?;
            f.flush()?;
            println!("LUKS ヘッダ : 潰した");
        }

        destroy_recovery(disk, &layout)?;
        r.recovery_destroyed = true;
        println!("回復領域    : 潰した");

        destroy_gpt(disk, size_bytes)?;
        r.gpt_destroyed = true;
        println!("GPT         : 主・予備とも潰した");
    }
    Ok(r)
}
