//! マニフェストの正規形。`unix-mdm-order` と同じ理由で手で書いている。
//! 署名対象のバイト列が一意でないと、同じ意味の別表現に署名を使い回せる。

use crate::Manifest;
use anyhow::{Result, bail};

pub const MAGIC: &[u8; 8] = b"UMDMIMG1";

const MAX_NAME: usize = 256;
/// チャンク数の上限。8MiB チャンクなら 512GiB ぶんで、実機のディスクに足りる。
/// 上限を置くのは、巨大な値で確保を誘発させないため。
const MAX_CHUNKS: usize = 65536;

pub fn encode(m: &Manifest) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + m.name.len() + m.chunks.len() * 32);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
    out.extend_from_slice(m.name.as_bytes());
    out.extend_from_slice(&m.total_size.to_le_bytes());
    out.extend_from_slice(&m.chunk_size.to_le_bytes());
    out.extend_from_slice(&(m.chunks.len() as u32).to_le_bytes());
    for c in &m.chunks {
        out.extend_from_slice(c);
    }
    out
}

pub fn decode(b: &[u8]) -> Result<Manifest> {
    let mut i = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        if i + n > b.len() {
            bail!("入力が途中で終わっている");
        }
        let s = &b[i..i + n];
        i += n;
        Ok(s)
    };

    if take(8)? != MAGIC {
        bail!("magic が違う。このイメージマニフェストではない");
    }
    let name_len = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
    if name_len > MAX_NAME {
        bail!("名前が長すぎる: {name_len}");
    }
    let name = std::str::from_utf8(take(name_len)?)?.to_string();
    let total_size = u64::from_le_bytes(take(8)?.try_into().unwrap());
    let chunk_size = u64::from_le_bytes(take(8)?.try_into().unwrap());
    if chunk_size == 0 {
        bail!("チャンク長が 0");
    }
    let n = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    if n > MAX_CHUNKS {
        bail!("チャンクが多すぎる: {n} > {MAX_CHUNKS}");
    }
    let mut chunks = Vec::with_capacity(n);
    for _ in 0..n {
        chunks.push(<[u8; 32]>::try_from(take(32)?).unwrap());
    }
    if i != b.len() {
        bail!("末尾に余分なバイトがある（{} バイト）", b.len() - i);
    }
    Ok(Manifest { name, total_size, chunk_size, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest::build("alpine-aarch64-minimum.img", &vec![7u8; 100], 32).unwrap()
    }

    #[test]
    fn 往復する() {
        let m = sample();
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn 末尾に足すと弾く() {
        let mut b = encode(&sample());
        b.push(0);
        assert!(decode(&b).is_err());
    }

    #[test]
    fn 切り詰めると弾く() {
        let b = encode(&sample());
        for n in 0..b.len() {
            assert!(decode(&b[..n]).is_err(), "{n} バイトで通ってしまった");
        }
    }

    #[test]
    fn チャンク数が過大なら確保する前に弾く() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u64.to_le_bytes());
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&b).is_err());
    }

    #[test]
    fn チャンク長0を弾く() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode(&b).is_err());
    }
}
