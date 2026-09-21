//! GPT の書き出し。
//!
//! `sgdisk` や `gpart` を呼ばずに自前で持っているのは、回復環境が
//! OpenBSD や NetBSD を含む全標的で同じ実行体として動く必要があるため。
//! それらに `sgdisk` は無く、あったとしても呼び出し規約が揃わない。

use crate::crc32::crc32;
use crate::layout::Layout;
use anyhow::{Result, bail};
use std::io::{Seek, SeekFrom, Write};

const SIGNATURE: &[u8; 8] = b"EFI PART";
const REVISION: u32 = 0x0001_0000;
const HEADER_SIZE: u32 = 92;
const ENTRY_COUNT: u32 = 128;
const ENTRY_SIZE: u32 = 128;

/// エントリ配列が占めるセクタ数。512B なら 32、4096B なら 4。
fn entry_array_sectors(sector_size: u64) -> u64 {
    (ENTRY_COUNT as u64 * ENTRY_SIZE as u64).div_ceil(sector_size)
}

/// 保護 MBR。GPT を知らない道具がディスクを「空き」と誤認して
/// 書き潰すのを防ぐためだけに置く。
fn protective_mbr(total_sectors: u64) -> [u8; 512] {
    let mut mbr = [0u8; 512];
    let e = &mut mbr[446..462];
    e[0] = 0x00; // 起動可能ではない
    e[1..4].copy_from_slice(&[0x00, 0x02, 0x00]); // 開始 CHS
    e[4] = 0xEE; // 種別: GPT protective
    e[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]); // 終了 CHS（飽和）
    e[8..12].copy_from_slice(&1u32.to_le_bytes()); // 開始 LBA
    // 2TiB を超えるディスクでは 32bit に収まらないので飽和させる。
    let n = u32::try_from(total_sectors - 1).unwrap_or(u32::MAX);
    e[12..16].copy_from_slice(&n.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

/// パーティションエントリ配列。128 個ぶんを常に書く。
fn entry_array(layout: &Layout) -> Vec<u8> {
    let mut buf = vec![0u8; (ENTRY_COUNT * ENTRY_SIZE) as usize];
    for p in &layout.partitions {
        let off = ((p.index - 1) * ENTRY_SIZE) as usize;
        let e = &mut buf[off..off + ENTRY_SIZE as usize];
        // GPT の GUID は前 3 フィールドがリトルエンディアンで後ろ 2 つが
        // ビッグエンディアンという混在表現。uuid の to_bytes_le がその形。
        e[0..16].copy_from_slice(&p.type_guid.to_bytes_le());
        e[16..32].copy_from_slice(&p.unique_guid.to_bytes_le());
        e[32..40].copy_from_slice(&p.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&p.last_lba.to_le_bytes());
        // 属性は 0。回復領域を隠し属性にするかは、実機で
        // どのブートマネージャがどう扱うか見てから決める。
        e[48..56].copy_from_slice(&0u64.to_le_bytes());
        // 名前は UTF-16LE で 36 文字ぶん。溢れたら切る。
        for (i, u) in p.label.encode_utf16().take(36).enumerate() {
            let o = 56 + i * 2;
            e[o..o + 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    buf
}

#[allow(clippy::too_many_arguments)]
fn header(
    layout: &Layout,
    my_lba: u64,
    alternate_lba: u64,
    entry_lba: u64,
    first_usable: u64,
    last_usable: u64,
    entries_crc: u32,
) -> Vec<u8> {
    let mut h = vec![0u8; HEADER_SIZE as usize];
    h[0..8].copy_from_slice(SIGNATURE);
    h[8..12].copy_from_slice(&REVISION.to_le_bytes());
    h[12..16].copy_from_slice(&HEADER_SIZE.to_le_bytes());
    // 16..20 はヘッダ自身の CRC。計算時は 0 でなければならないので後で入れる。
    h[24..32].copy_from_slice(&my_lba.to_le_bytes());
    h[32..40].copy_from_slice(&alternate_lba.to_le_bytes());
    h[40..48].copy_from_slice(&first_usable.to_le_bytes());
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[56..72].copy_from_slice(&layout.disk_guid.to_bytes_le());
    h[72..80].copy_from_slice(&entry_lba.to_le_bytes());
    h[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
    h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&h);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// 配置を GPT として書き出す。
///
/// 書き込み先はイメージファイルでもブロックデバイスでもよいが、
/// **どちらであるかを判断するのは呼び出し側の責任**。この関数は
/// 渡されたものを黙って上書きする。
pub fn write_gpt<W: Write + Seek>(layout: &Layout, w: &mut W) -> Result<()> {
    let ss = layout.sector_size;
    let total = layout.total_sectors;
    let eas = entry_array_sectors(ss);

    let first_usable = 2 + eas;
    let last_usable = total - 1 - 1 - eas;

    for p in &layout.partitions {
        if p.first_lba < first_usable || p.last_lba > last_usable {
            bail!(
                "{:?} ({}..{}) が使用可能範囲 {}..{} の外にある",
                p.role,
                p.first_lba,
                p.last_lba,
                first_usable,
                last_usable
            );
        }
    }

    let entries = entry_array(layout);
    let ecrc = crc32(&entries);

    let primary_entry_lba = 2;
    let backup_entry_lba = total - 1 - eas;
    let backup_header_lba = total - 1;

    // LBA 0: 保護 MBR
    w.seek(SeekFrom::Start(0))?;
    w.write_all(&protective_mbr(total))?;

    // LBA 1: 主ヘッダ
    let ph = header(
        layout,
        1,
        backup_header_lba,
        primary_entry_lba,
        first_usable,
        last_usable,
        ecrc,
    );
    w.seek(SeekFrom::Start(ss))?;
    w.write_all(&ph)?;
    w.write_all(&vec![0u8; (ss as usize) - ph.len()])?;

    // LBA 2..: 主エントリ配列
    w.seek(SeekFrom::Start(primary_entry_lba * ss))?;
    w.write_all(&entries)?;

    // 末尾手前: 予備エントリ配列
    w.seek(SeekFrom::Start(backup_entry_lba * ss))?;
    w.write_all(&entries)?;

    // 最終 LBA: 予備ヘッダ。my/alternate と entry_lba が主と入れ替わる。
    let bh = header(
        layout,
        backup_header_lba,
        1,
        backup_entry_lba,
        first_usable,
        last_usable,
        ecrc,
    );
    w.seek(SeekFrom::Start(backup_header_lba * ss))?;
    w.write_all(&bh)?;
    w.write_all(&vec![0u8; (ss as usize) - bh.len()])?;

    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Role, RootKind};
    use std::collections::BTreeMap;

    const GIB: u64 = 1024 * 1024 * 1024;

    /// 書かれた所だけを覚える受け皿。
    ///
    /// `Cursor::new(vec![0u8; bytes])` にしていたら、64GiB のディスクを試すだけで
    /// 64GiB を確保しようとしていた。overcommit する OS では素通りするが、
    /// OpenBSD は確保を断り、FreeBSD は OOM で殺した。CI で両方に当たって
    /// 分かったもので、手元の macOS では見えなかった。
    ///
    /// 実際のディスクへの書き込みも疎なので、こちらのほうが本物に近い。
    #[derive(Default)]
    struct Sparse {
        pos: u64,
        bytes: BTreeMap<u64, u8>,
    }

    impl Sparse {
        /// 書いていない所は 0 として読む。実ディスクの未書き込み領域と同じ。
        fn read_at(&self, off: u64, n: usize) -> Vec<u8> {
            (0..n as u64)
                .map(|i| *self.bytes.get(&(off + i)).unwrap_or(&0))
                .collect()
        }

        fn u8_at(&self, off: u64) -> u8 {
            *self.bytes.get(&off).unwrap_or(&0)
        }
    }

    impl Write for Sparse {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            for (i, b) in buf.iter().enumerate() {
                self.bytes.insert(self.pos + i as u64, *b);
            }
            self.pos += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Seek for Sparse {
        fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
            self.pos = match p {
                SeekFrom::Start(n) => n,
                SeekFrom::Current(n) => (self.pos as i64 + n) as u64,
                SeekFrom::End(_) => {
                    return Err(std::io::Error::other("End からの seek は使わない"));
                }
            };
            Ok(self.pos)
        }
    }

    fn written(bytes: u64, ss: u64) -> (Layout, Sparse) {
        let l = Layout::plan(bytes, ss, RootKind::LinuxLuks).unwrap();
        let mut c = Sparse::default();
        write_gpt(&l, &mut c).unwrap();
        (l, c)
    }

    #[test]
    fn 保護mbrが正しい() {
        let (_, img) = written(64 * GIB, 512);
        assert_eq!(img.u8_at(510), 0x55);
        assert_eq!(img.u8_at(511), 0xAA);
        assert_eq!(img.u8_at(446 + 4), 0xEE, "種別が GPT protective でない");
        // 64GiB は 512B セクタで 134,217,728 セクタ。32bit に収まるので実値が入る。
        assert_eq!(
            u32::from_le_bytes(img.read_at(458, 4).try_into().unwrap()),
            (64 * GIB / 512 - 1) as u32
        );
    }

    #[test]
    fn 二テビバイト超で保護mbrが飽和する() {
        // 32bit LBA に収まらない大きさ。ここで飽和しないと、GPT を知らない
        // 道具から見たディスクの大きさが巻き戻り、末尾が空きに見えてしまう。
        let l = Layout::plan(4 * 1024 * GIB, 512, RootKind::LinuxLuks).unwrap();
        let mbr = protective_mbr(l.total_sectors);
        assert_eq!(
            u32::from_le_bytes(mbr[458..462].try_into().unwrap()),
            u32::MAX
        );
    }

    #[test]
    fn 主ヘッダの署名とcrcが通る() {
        let (l, img) = written(64 * GIB, 512);
        let h = img.read_at(l.sector_size, 92);
        assert_eq!(&h[0..8], SIGNATURE);

        let mut probe = h.clone();
        let stored = u32::from_le_bytes(probe[16..20].try_into().unwrap());
        probe[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(crc32(&probe), stored, "ヘッダ CRC が合わない");
    }

    #[test]
    fn 予備ヘッダが末尾にあり主と対になる() {
        let (l, img) = written(64 * GIB, 512);
        let last = (l.total_sectors - 1) * l.sector_size;
        let bh = img.read_at(last, 92);
        assert_eq!(&bh[0..8], SIGNATURE);
        assert_eq!(
            u64::from_le_bytes(bh[24..32].try_into().unwrap()),
            l.total_sectors - 1
        );
        assert_eq!(
            u64::from_le_bytes(bh[32..40].try_into().unwrap()),
            1,
            "予備の alternate は主ヘッダを指すはず"
        );
    }

    #[test]
    fn エントリが配置と一致する() {
        let (l, img) = written(64 * GIB, 512);
        let base = 2 * l.sector_size;
        for p in &l.partitions {
            let o = base + ((p.index - 1) * ENTRY_SIZE) as u64;
            assert_eq!(img.read_at(o, 16), p.type_guid.to_bytes_le());
            assert_eq!(img.read_at(o + 16, 16), p.unique_guid.to_bytes_le());
            assert_eq!(
                u64::from_le_bytes(img.read_at(o + 32, 8).try_into().unwrap()),
                p.first_lba
            );
            assert_eq!(
                u64::from_le_bytes(img.read_at(o + 40, 8).try_into().unwrap()),
                p.last_lba
            );
        }
    }

    #[test]
    fn ラベルがutf16leで入る() {
        let (l, img) = written(64 * GIB, 512);
        let rec = l.get(Role::Recovery).unwrap();
        let o = 2 * l.sector_size + ((rec.index - 1) * ENTRY_SIZE) as u64 + 56;
        let units: Vec<u16> = (0..rec.label.len() as u64)
            .map(|i| u16::from_le_bytes(img.read_at(o + i * 2, 2).try_into().unwrap()))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), rec.label);
    }

    #[test]
    fn 四千九十六バイトセクタでも通る() {
        let (l, img) = written(64 * GIB, 4096);
        let h = img.read_at(4096, 92);
        assert_eq!(&h[0..8], SIGNATURE);
        // 4096B ではエントリ配列が 4 セクタで済むので、使用可能域が前に出る。
        assert_eq!(u64::from_le_bytes(h[40..48].try_into().unwrap()), 6);
        assert_eq!(
            u64::from_le_bytes(h[48..56].try_into().unwrap()),
            l.total_sectors - 6
        );
    }

    #[test]
    fn 主と予備でエントリ配列のcrcが一致する() {
        let (l, img) = written(64 * GIB, 512);
        let ss = l.sector_size;
        let eas = entry_array_sectors(ss);
        let n = (ENTRY_COUNT * ENTRY_SIZE) as usize;
        let primary = img.read_at(2 * ss, n);
        let backup_lba = l.total_sectors - 1 - eas;
        let backup = img.read_at(backup_lba * ss, n);
        assert_eq!(primary, backup, "予備エントリ配列が主と違う");
    }
}
