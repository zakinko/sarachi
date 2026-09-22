//! 最小の HTTP。
//!
//! 枠組みを入れないのは、**鍵を預かる物の中身が読み切れなくなるのが一番
//! まずい**ため。要るのは三つの経路だけで、それなら std で足りる。
//!
//! **TLS はここで終端しない。** 前段（nginx なり）に任せる。回復環境の側は
//! https でなければ鍵を送らないので、前段が無ければ繋がらないだけで済む。
//! 黙って平文で流れる形にはならない。

use anyhow::{Result, bail};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

pub struct Request {
    pub method: String,
    pub path: String,
    pub auth: Option<String>,
    pub body: Vec<u8>,
}

/// 受ける本文の上限。鍵は数十 byte なので、これで十分すぎる。
/// 上限を置かないと、送りつけられた分だけメモリを取る。
const MAX_BODY: usize = 64 * 1024;

pub fn read_request(stream: &TcpStream) -> Result<Request> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut it = line.split_whitespace();
    let method = it.next().unwrap_or_default().to_string();
    let path = it.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        bail!("要求の形が違う");
    }

    let mut len = 0usize;
    let mut auth = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = match h.split_once(':') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => continue,
        };
        match k.as_str() {
            "content-length" => len = v.parse().unwrap_or(0),
            "authorization" => auth = Some(v),
            _ => {}
        }
    }
    if len > MAX_BODY {
        bail!("本文が長すぎる: {len}");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        auth,
        body,
    })
}

pub fn respond(mut stream: &TcpStream, code: u16, body: &[u8]) -> Result<()> {
    let reason = match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {code} {reason}\r\n\
         content-length: {}\r\n\
         content-type: application/octet-stream\r\n\
         cache-control: no-store\r\n\
         connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// 時間で漏らさない比較。
///
/// 素朴な `==` は食い違った所で返るので、掛かる時間から token を一文字ずつ
/// 当てられる。預かっているものを考えれば、ここは惜しまない。
pub fn secret_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// `{"device_id":"…","key":"<16進>"}` から取り出す。
///
/// JSON の parser を入れない。受ける形が一つしかないので、そこだけ読む。
pub fn parse_escrow(body: &[u8]) -> Result<(String, Vec<u8>)> {
    let s = std::str::from_utf8(body)?;
    let dev = field(s, "device_id").ok_or_else(|| anyhow::anyhow!("device_id が無い"))?;
    let hex = field(s, "key").ok_or_else(|| anyhow::anyhow!("key が無い"))?;
    if hex.len() % 2 != 0 || hex.is_empty() {
        bail!("鍵の 16 進が壊れている");
    }
    let mut key = Vec::with_capacity(hex.len() / 2);
    let h = hex.as_bytes();
    for i in (0..h.len()).step_by(2) {
        let b = std::str::from_utf8(&h[i..i + 2])?;
        key.push(u8::from_str_radix(b, 16)?);
    }
    Ok((dev, key))
}

fn field(s: &str, name: &str) -> Option<String> {
    let pat = format!("\"{name}\"");
    let i = s.find(&pat)? + pat.len();
    let rest = &s[i..];
    let c = rest.find(':')? + 1;
    let rest = &rest[c..];
    let q1 = rest.find('"')? + 1;
    let rest = &rest[q1..];
    let q2 = rest.find('"')?;
    Some(rest[..q2].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 本文から取り出せる() {
        let (d, k) = parse_escrow(br#"{"device_id":"dev-1","key":"deadbeef"}"#).unwrap();
        assert_eq!(d, "dev-1");
        assert_eq!(k, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn 壊れた本文は断る() {
        for bad in [
            &br#"{"device_id":"d"}"#[..],
            &br#"{"key":"aa"}"#[..],
            &br#"{"device_id":"d","key":"abc"}"#[..],
            &br#"{"device_id":"d","key":"zz"}"#[..],
            &br#"{"device_id":"d","key":""}"#[..],
        ] {
            assert!(
                parse_escrow(bad).is_err(),
                "{:?} を通した",
                std::str::from_utf8(bad)
            );
        }
    }

    #[test]
    fn 時間で漏らさない比較が正しく判定する() {
        assert!(secret_eq("abc", "abc"));
        assert!(!secret_eq("abc", "abd"));
        assert!(!secret_eq("abc", "ab"));
        assert!(secret_eq("", ""));
    }
}
