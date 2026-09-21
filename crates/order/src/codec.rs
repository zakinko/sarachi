//! 署名対象の正規形。
//!
//! 手で書いているのは、署名する対象のバイト列が**一意でなければならない**ため。
//! 同じ意味の命令から違うバイト列が作れると、片方で通した署名がもう片方に
//! 使い回せてしまい、署名の意味が崩れる。serde と汎用フォーマットを噛ませると
//! その一意性が実装依存になるので、順序も長さの持ち方もここで固定する。

use crate::{Level, Order};
use anyhow::{Result, bail};

/// 形式が変わったら末尾の数字を上げる。古い署名が新しい解釈で
/// 通ってしまわないよう、magic も署名対象に含める。
pub const MAGIC: &[u8; 8] = b"SARAORD1";

/// 長さの上限。ここを開けておくと、巨大な入力で資源を食わせられる。
const MAX_STR: usize = 256;

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

pub fn encode(o: &Order) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + o.device_id.len() + o.issuer.len());
    out.extend_from_slice(MAGIC);
    put_str(&mut out, &o.device_id);
    out.push(o.level.as_u8());
    out.extend_from_slice(&o.nonce);
    out.extend_from_slice(&o.issued_at.to_le_bytes());
    out.extend_from_slice(&o.expires_at.to_le_bytes());
    put_str(&mut out, &o.issuer);
    out
}

struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.i + n > self.b.len() {
            bail!(
                "入力が途中で終わっている（{} 要るが {} しかない）",
                n,
                self.b.len() - self.i
            );
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        if n > MAX_STR {
            bail!("文字列が長すぎる: {n} > {MAX_STR}");
        }
        Ok(std::str::from_utf8(self.take(n)?)?.to_string())
    }
}

pub fn decode(b: &[u8]) -> Result<Order> {
    let mut c = Cur { b, i: 0 };
    if c.take(8)? != MAGIC {
        bail!("magic が違う。この形式の命令ではない");
    }
    let device_id = c.string()?;
    let level = Level::from_u8(c.take(1)?[0])?;
    let nonce: [u8; 16] = c.take(16)?.try_into().unwrap();
    let issued_at = c.u64()?;
    let expires_at = c.u64()?;
    let issuer = c.string()?;

    // 末尾に余りがあるものは受けない。余剰バイトを許すと、同じ命令に
    // 対して複数のバイト列が成立してしまう。
    if c.i != b.len() {
        bail!("末尾に余分なバイトがある（{} バイト）", b.len() - c.i);
    }
    Ok(Order {
        device_id,
        level,
        nonce,
        issued_at,
        expires_at,
        issuer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random_nonce;

    fn sample() -> Order {
        Order {
            device_id: "lima-alpine-arm64".into(),
            level: Level::Destroy,
            nonce: random_nonce(),
            issued_at: 1_789_000_000,
            expires_at: 1_789_003_600,
            issuer: "sarachi control plane".into(),
        }
    }

    #[test]
    fn 往復する() {
        let o = sample();
        assert_eq!(decode(&encode(&o)).unwrap(), o);
    }

    #[test]
    fn 同じ命令からは必ず同じバイト列が出る() {
        let o = sample();
        assert_eq!(encode(&o), encode(&o.clone()));
    }

    #[test]
    fn 末尾に足すと弾く() {
        // 余剰を許すと、同じ命令に複数の表現が成立して署名の一意性が崩れる。
        let mut b = encode(&sample());
        b.push(0);
        assert!(decode(&b).is_err());
    }

    #[test]
    fn magicが違えば弾く() {
        let mut b = encode(&sample());
        b[0] = b'X';
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
    fn 長すぎる文字列を弾く() {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&(u16::MAX).to_le_bytes());
        assert!(decode(&b).is_err());
    }

    #[test]
    fn 未知の段階を弾く() {
        let o = sample();
        let mut b = encode(&o);
        let off = 8 + 2 + o.device_id.len();
        b[off] = 99;
        assert!(decode(&b).is_err());
    }
}
