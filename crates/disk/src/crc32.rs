//! GPT のヘッダとエントリ配列に要る CRC-32（IEEE 802.3、zlib と同じ多項式）。
//!
//! crc32 クレートを引かずに持っているのは、回復環境を小さく保つため。
//! 依存を一つ増やすたびに initramfs が太り、それは RAM に常駐する分だけ
//! 効いてくる。40 行で済むものを外から持ってくる理由はない。

const POLY: u32 = 0xEDB8_8320;

fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

pub fn crc32(data: &[u8]) -> u32 {
    let t = table();
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = t[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 既知の値と一致する() {
        // CRC-32 の標準的な検査値。
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }
}
