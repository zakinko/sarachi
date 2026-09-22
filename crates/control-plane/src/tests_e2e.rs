//! 実際に socket を通す試験。
//!
//! 単体で本文の読み書きが合っていても、**回復環境の側と同じ言葉を話して
//! いなければ意味が無い**。本文の形は escrow.rs が出すものをそのまま書く。
//! ここが食い違うと、導入の終盤——鍵を預ける所——で初めて分かる。

use super::*;
use std::io::{Read, Write};
use std::net::TcpStream;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn spawn() -> (String, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "sarachi-cp-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let srv = Arc::new(Server {
        store: store::Store::open(&dir).unwrap(),
        token: TOKEN.to_string(),
    });
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let srv = Arc::clone(&srv);
            std::thread::spawn(move || srv.handle(&s));
        }
    });
    (addr, dir)
}

fn call(addr: &str, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n",
        body.len()
    );
    if let Some(t) = token {
        req.push_str(&format!("authorization: Bearer {t}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    s.flush().unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out).to_string();
    let code: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = match out.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => out[i + 4..].to_vec(),
        None => Vec::new(),
    };
    (code, body)
}

#[test]
fn 預けて取り出して失効させるまで通る() {
    let (addr, dir) = spawn();
    // 本文の形は escrow.rs が出すものと同じ。
    let body = r#"{"device_id":"dev-1","key":"deadbeef"}"#;

    let (c, _) = call(&addr, "POST", "/v1/escrow", Some(TOKEN), body);
    assert_eq!(c, 201, "預けられない");

    let (c, k) = call(&addr, "GET", "/v1/key/dev-1", Some(TOKEN), "");
    assert_eq!(c, 200);
    assert_eq!(k, vec![0xde, 0xad, 0xbe, 0xef], "渡ってきた鍵が違う");

    let (c, _) = call(&addr, "POST", "/v1/revoke/dev-1", Some(TOKEN), "");
    assert_eq!(c, 200, "失効させられない");

    // ここが本番。失効させた後に渡ってはいけない。
    let (c, _) = call(&addr, "GET", "/v1/key/dev-1", Some(TOKEN), "");
    assert_eq!(c, 404, "失効させたのに鍵が渡った");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tokenが無ければ何も通さない() {
    let (addr, dir) = spawn();
    let body = r#"{"device_id":"dev-1","key":"aa"}"#;
    for (m, p, b) in [
        ("POST", "/v1/escrow", body),
        ("GET", "/v1/key/dev-1", ""),
        ("POST", "/v1/revoke/dev-1", ""),
    ] {
        let (c, _) = call(&addr, m, p, None, b);
        assert_eq!(c, 401, "{m} {p} が token 無しで通った");
        let (c, _) = call(&addr, m, p, Some("wrong-token-wrong-token-wrong-xx"), b);
        assert_eq!(c, 401, "{m} {p} が違う token で通った");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 同じ台の鍵を二度預けさせない() {
    // 二台目が一台目の鍵を消すと、一台目が救えなくなる。
    let (addr, dir) = spawn();
    let a = r#"{"device_id":"dev-1","key":"aabb"}"#;
    let b = r#"{"device_id":"dev-1","key":"ccdd"}"#;
    assert_eq!(call(&addr, "POST", "/v1/escrow", Some(TOKEN), a).0, 201);
    assert_eq!(
        call(&addr, "POST", "/v1/escrow", Some(TOKEN), b).0,
        409,
        "上書きできてしまった"
    );
    let (_, k) = call(&addr, "GET", "/v1/key/dev-1", Some(TOKEN), "");
    assert_eq!(k, vec![0xaa, 0xbb], "元の鍵が壊れた");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 置き場の外を指す名前は通さない() {
    let (addr, dir) = spawn();
    let (c, _) = call(
        &addr,
        "POST",
        "/v1/escrow",
        Some(TOKEN),
        r#"{"device_id":"../escape","key":"aa"}"#,
    );
    assert_ne!(c, 201, "置き場の外へ書けてしまった");
    let _ = std::fs::remove_dir_all(&dir);
}
