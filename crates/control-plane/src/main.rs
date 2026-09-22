//! 鍵を預かり、消すときに失効させる。
//!
//! # なぜ要るのか
//!
//! 回復環境は台ごとに違う鍵を作って root を暗号化する。消去はその鍵を壊す
//! ことで成り立つ。**鍵の置き場所がそのまま「本当に消えたか」を決める。**
//!
//! BitLocker と同じ三枚重ねを採り、ここを正に据えた。TPM を正にできないのは
//! Raspberry Pi が持たないため。詳しくは回復環境側の `escrow` を見ること。
//!
//! **NetBSD ではこれが必須になる。** cgd は keyslot を持たないので、鍵を
//! 手元に置かず起動のたびにここから取る形にする。その台の消去は、ここで
//! 失効させることそのものになる。
//!
//! # 経路
//!
//! ```text
//! POST /v1/escrow            預かる。{"device_id":"…","key":"<16進>"}
//! GET  /v1/key/<device_id>   渡す。起動時に鍵を取りに来る台のため
//! POST /v1/revoke/<id>       失効させる。**これが消去の本体になる台がある**
//! ```
//!
//! すべて `Authorization: Bearer <token>` が要る。
//!
//! # 置いていないもの
//!
//! **TLS を終端しない。** 前段に任せる。回復環境は https でなければ鍵を
//! 送らないので、前段が無ければ繋がらないだけで、黙って平文で流れる形には
//! ならない。
//!
//! **台ごとに token を分けていない。** 今は一つ。分けるのは台の登録という
//! 別の仕組みが要る話で、そこはまだ無い。**この状態では、一台分の token が
//! 漏れれば全台の鍵が取れる。** 運用に載せる前に必ず直すこと。

mod http;
mod store;

#[cfg(test)]
mod tests_e2e;

use anyhow::{Context, Result};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

struct Server {
    store: store::Store,
    token: String,
}

impl Server {
    fn handle(&self, stream: &TcpStream) {
        let req = match http::read_request(stream) {
            Ok(r) => r,
            Err(_) => {
                let _ = http::respond(stream, 400, b"");
                return;
            }
        };

        // 認証を最初に見る。経路ごとに書くと、足したときに忘れる。
        let ok = req
            .auth
            .as_deref()
            .and_then(|a| a.strip_prefix("Bearer "))
            .map(|t| http::secret_eq(t, &self.token))
            .unwrap_or(false);
        if !ok {
            let _ = http::respond(stream, 401, b"");
            return;
        }

        let (code, body) = self.route(&req);
        let _ = http::respond(stream, code, &body);
    }

    fn route(&self, req: &http::Request) -> (u16, Vec<u8>) {
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/v1/escrow") => match http::parse_escrow(&req.body) {
                Ok((dev, key)) => match self.store.put(&dev, &key) {
                    Ok(()) => {
                        eprintln!("預かった: {dev}");
                        (201, Vec::new())
                    }
                    // 既に在る鍵は上書きしない。二台目が一台目の鍵を消す事故を
                    // 通すと、救えなくなる方向にしか転ばない。
                    Err(e) => {
                        eprintln!("預かれない: {dev}: {e}");
                        (409, Vec::new())
                    }
                },
                Err(_) => (400, Vec::new()),
            },
            ("GET", p) if p.starts_with("/v1/key/") => {
                let dev = &p["/v1/key/".len()..];
                match self.store.get(dev) {
                    Ok(k) => (200, k),
                    Err(_) => (404, Vec::new()),
                }
            }
            ("POST", p) if p.starts_with("/v1/revoke/") => {
                let dev = &p["/v1/revoke/".len()..];
                match self.store.revoke(dev) {
                    Ok(()) => {
                        // 消したことは記録に残す。消えたかどうかを後から
                        // 問われるのは、たいていこの行が要る場面になる。
                        eprintln!("失効させた: {dev}");
                        (200, Vec::new())
                    }
                    Err(_) => (404, Vec::new()),
                }
            }
            _ => (404, Vec::new()),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let opt = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let addr = opt("--listen").unwrap_or_else(|| "127.0.0.1:8443".into());
    let dir = PathBuf::from(opt("--store").unwrap_or_else(|| "/var/db/sarachi".into()));

    // token は引数で受けない。ps に出る。
    let token = std::env::var("SARACHI_TOKEN")
        .context("SARACHI_TOKEN を環境で渡すこと（引数にすると ps に出る）")?;
    if token.len() < 32 {
        anyhow::bail!("SARACHI_TOKEN が短すぎる（32 文字以上）");
    }

    let srv = Arc::new(Server {
        store: store::Store::open(&dir)?,
        token,
    });
    let l = TcpListener::bind(&addr).with_context(|| format!("{addr} で待てない"))?;
    eprintln!("待っている: {addr}（置き場 {}）", dir.display());
    eprintln!("TLS はここで終端しない。前段に置くこと。");

    for s in l.incoming() {
        let s = match s {
            Ok(s) => s,
            Err(_) => continue,
        };
        let srv = Arc::clone(&srv);
        std::thread::spawn(move || srv.handle(&s));
    }
    Ok(())
}
