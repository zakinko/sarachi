//! 回復パーティションを含むディスク配置。
//!
//! 配置は WinRE を参考にしているが、回復領域を root の**前**に置く点が異なる。
//! WinRE が Windows パーティションの直後に置くのは、Windows を縮めて回復領域を
//! 広げられるようにするためで、収まらなかった場合に古い回復領域が孤児になる
//! 失敗モードが文書化されている。こちらは回復環境が固定サイズの成果物として
//! 丸ごと置換されるので広げる必要がなく、前に置いたほうが root を末尾まで
//! 自由に伸ばせる。
//!
//! パーティションの unique GUID は毎回新しく生成する。イメージを dd で配ると
//! GUID まで複製され、全台が同じ同一性を持ってしまうため。

use anyhow::{Result, bail};
use uuid::Uuid;

/// 1MiB 境界に揃える。SSD の消去ブロックと、ほぼ全ての RAID ストライプに対して
/// 安全側に倒れる慣例値。
const ALIGNMENT_BYTES: u64 = 1024 * 1024;

/// GPT の主ヘッダとパーティションエントリ配列が占める先頭領域。
const GPT_PRIMARY_SECTORS: u64 = 34;
/// 末尾の予備ヘッダ。
const GPT_SECONDARY_SECTORS: u64 = 33;

const ESP_BYTES: u64 = 512 * 1024 * 1024;
const RECOVERY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// これを下回る root しか取れないディスクは対象外とする。
const ROOT_MIN_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// EFI System Partition。
const TYPE_ESP: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
/// 本プロジェクト固有。既存のどの type GUID でもないものを振ることで、
/// どの OS にも自動マウントされず、かつ我々の領域だと一目で分かるようにする。
const TYPE_RECOVERY: &str = "5f9a1c7e-4b2d-4e8a-9c3f-1d6b8e0a7c24";
/// Linux LUKS。中身は回復環境が台ごとに新しい鍵で作る。
const TYPE_LUKS: &str = "ca7d7ccb-63ed-4c53-861c-1742536059cc";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 実 OS と回復環境の双方のローダを置く。
    Esp,
    /// カーネル + initramfs。これが WinPE 相当。
    Recovery,
    /// 暗号コンテナ。鍵は回復環境が生成するので台ごとに必ず異なる。
    Root,
}

impl Role {
    fn type_guid(self) -> Uuid {
        let s = match self {
            Role::Esp => TYPE_ESP,
            Role::Recovery => TYPE_RECOVERY,
            Role::Root => TYPE_LUKS,
        };
        Uuid::parse_str(s).expect("組み込みの type GUID が壊れている")
    }

    fn label(self) -> &'static str {
        match self {
            Role::Esp => "ESP",
            Role::Recovery => "SARACHI-RECOVERY",
            Role::Root => "SARACHI-ROOT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub index: u32,
    pub role: Role,
    pub type_guid: Uuid,
    /// 台ごとに新規生成される。dd による複製で衝突しないことの担保。
    pub unique_guid: Uuid,
    pub label: String,
    pub first_lba: u64,
    /// 終端を含む。
    pub last_lba: u64,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub sector_size: u64,
    pub total_sectors: u64,
    /// ディスク自体の GUID。これも毎回新規に振る。
    pub disk_guid: Uuid,
    pub partitions: Vec<Partition>,
}

impl Layout {
    /// 与えられたディスクに対する配置を決める。ディスクには一切書き込まない。
    pub fn plan(total_bytes: u64, sector_size: u64) -> Result<Self> {
        if !sector_size.is_power_of_two() || !(512..=4096).contains(&sector_size) {
            bail!("扱えないセクタ長: {sector_size}");
        }
        if ALIGNMENT_BYTES % sector_size != 0 {
            bail!("セクタ長 {sector_size} が 1MiB 境界を割り切らない");
        }

        let total_sectors = total_bytes / sector_size;
        let align = ALIGNMENT_BYTES / sector_size;

        let required = ESP_BYTES + RECOVERY_BYTES + ROOT_MIN_BYTES;
        if total_bytes < required {
            bail!(
                "ディスクが小さすぎる: {} MiB しかないが、最低 {} MiB 要る \
                 (ESP {} + 回復 {} + root 下限 {})",
                total_bytes / 1024 / 1024,
                required / 1024 / 1024,
                ESP_BYTES / 1024 / 1024,
                RECOVERY_BYTES / 1024 / 1024,
                ROOT_MIN_BYTES / 1024 / 1024,
            );
        }

        // 末尾は予備 GPT ヘッダの手前まで。そこから下へ 1MiB 境界に丸める。
        let usable_end = total_sectors - GPT_SECONDARY_SECTORS - 1;
        let last_usable = (usable_end + 1) / align * align - 1;

        let mut cursor = GPT_PRIMARY_SECTORS.div_ceil(align) * align;
        let mut partitions = Vec::new();

        for (index, (role, bytes)) in [
            (Role::Esp, Some(ESP_BYTES)),
            (Role::Recovery, Some(RECOVERY_BYTES)),
            (Role::Root, None),
        ]
        .into_iter()
        .enumerate()
        {
            let last_lba = match bytes {
                Some(b) => cursor + b / sector_size - 1,
                // root は末尾まで使い切る。dd 方式で必要だった
                // 「後からパーティションを広げる」処理がこれで不要になる。
                None => last_usable,
            };
            if last_lba > last_usable {
                bail!("{:?} がディスク末尾を越える", role);
            }
            partitions.push(Partition {
                index: index as u32 + 1,
                role,
                type_guid: role.type_guid(),
                unique_guid: Uuid::new_v4(),
                label: role.label().to_string(),
                first_lba: cursor,
                last_lba,
            });
            cursor = last_lba + 1;
        }

        Ok(Layout {
            sector_size,
            total_sectors,
            disk_guid: Uuid::new_v4(),
            partitions,
        })
    }

    pub fn get(&self, role: Role) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.role == role)
    }

    /// 書き込む前に人間が読んで確認するための表。
    pub fn describe(&self) -> String {
        let mut out = format!(
            "ディスク {} セクタ x {}B = {} GiB\ndisk GUID {}\n\n",
            self.total_sectors,
            self.sector_size,
            self.total_sectors * self.sector_size / 1024 / 1024 / 1024,
            self.disk_guid,
        );
        out.push_str("  # 役割      先頭LBA       終端LBA      サイズ  ラベル\n");
        for p in &self.partitions {
            out.push_str(&format!(
                "  {} {:<9} {:>11} {:>13} {:>9} MiB  {}\n",
                p.index,
                format!("{:?}", p.role),
                p.first_lba,
                p.last_lba,
                p.sectors() * self.sector_size / 1024 / 1024,
                p.label,
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn 隙間なく末尾まで使い切る() {
        let l = Layout::plan(64 * GIB, 512).unwrap();
        assert_eq!(l.partitions.len(), 3);
        for w in l.partitions.windows(2) {
            assert_eq!(w[0].last_lba + 1, w[1].first_lba, "パーティション間に隙間");
        }
        let last = l.partitions.last().unwrap();
        assert!(last.last_lba < l.total_sectors - GPT_SECONDARY_SECTORS);
    }

    #[test]
    fn 全て1mib境界に揃う() {
        for sector_size in [512, 4096] {
            let l = Layout::plan(64 * GIB, sector_size).unwrap();
            let align = ALIGNMENT_BYTES / sector_size;
            for p in &l.partitions {
                assert_eq!(p.first_lba % align, 0, "{:?} の先頭が非整列", p.role);
            }
        }
    }

    #[test]
    fn 固定サイズが仕様どおり() {
        let l = Layout::plan(64 * GIB, 512).unwrap();
        let esp = l.get(Role::Esp).unwrap();
        let rec = l.get(Role::Recovery).unwrap();
        assert_eq!(esp.sectors() * 512, ESP_BYTES);
        assert_eq!(rec.sectors() * 512, RECOVERY_BYTES);
    }

    #[test]
    fn rootがディスクに応じて伸びる() {
        let small = Layout::plan(16 * GIB, 512).unwrap();
        let large = Layout::plan(512 * GIB, 512).unwrap();
        assert!(
            large.get(Role::Root).unwrap().sectors() > small.get(Role::Root).unwrap().sectors(),
            "root が末尾まで伸びていない"
        );
    }

    #[test]
    fn 小さすぎるディスクは断る() {
        assert!(Layout::plan(4 * GIB, 512).is_err());
    }

    #[test]
    fn guidは毎回異なる() {
        // dd による複製で同一性が衝突しないことの担保。
        let a = Layout::plan(64 * GIB, 512).unwrap();
        let b = Layout::plan(64 * GIB, 512).unwrap();
        assert_ne!(a.disk_guid, b.disk_guid);
        for (pa, pb) in a.partitions.iter().zip(&b.partitions) {
            assert_ne!(pa.unique_guid, pb.unique_guid, "{:?} の GUID が重複", pa.role);
            assert_eq!(pa.type_guid, pb.type_guid, "type GUID は固定のはず");
        }
    }
}
