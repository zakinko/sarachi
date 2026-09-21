//! ブロックデバイスの列挙。
//!
//! sysfs を直に読んでいるのは、回復環境に lsblk も udev も置きたくないため。
//! 見るのは `/sys/block` だけで、そこに無いものは相手にしない。

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Disk {
    pub name: String,
    pub path: PathBuf,
    /// sysfs の size は常に 512 バイト単位。論理セクタ長とは別物なので混ぜない。
    pub size_bytes: u64,
    pub logical_sector_size: u64,
    pub removable: bool,
    pub rotational: bool,
    pub model: Option<String>,
}

impl Disk {
    pub fn describe(&self) -> String {
        format!(
            "{:<8} {:>9} MiB  sector {}  {}{}{}",
            self.name,
            self.size_bytes / 1024 / 1024,
            self.logical_sector_size,
            if self.removable { "removable " } else { "" },
            if self.rotational {
                "rotational "
            } else {
                "ssd "
            },
            self.model.as_deref().unwrap_or(""),
        )
    }
}

fn read_u64(p: &PathBuf) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// 消去や書き込みの対象になり得るものだけを返す。
pub fn list() -> Result<Vec<Disk>> {
    let mut out = Vec::new();
    for e in fs::read_dir("/sys/block").context("/sys/block を読めない")? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();

        // loop と ram と device-mapper は対象にしない。前二つは実体が
        // ファイルかメモリで、dm は下にある実デバイスを消すべきだから。
        if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("dm-") {
            continue;
        }
        let base = e.path();
        let size_sectors = read_u64(&base.join("size")).unwrap_or(0);
        if size_sectors == 0 {
            continue;
        }
        out.push(Disk {
            path: PathBuf::from("/dev").join(&name),
            name,
            // sysfs の size は 512 バイト固定単位。
            size_bytes: size_sectors * 512,
            logical_sector_size: read_u64(&base.join("queue/logical_block_size")).unwrap_or(512),
            removable: read_u64(&base.join("removable")).unwrap_or(0) == 1,
            rotational: read_u64(&base.join("queue/rotational")).unwrap_or(0) == 1,
            model: fs::read_to_string(base.join("device/model"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
