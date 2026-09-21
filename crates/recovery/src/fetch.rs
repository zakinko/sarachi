//! HTTP(S) からの取得。
//!
//! 自前で持っているのは、curl と OpenSSL を initramfs に入れずに済ませるため。
//! TLS は rustls なので、システムの証明書置き場が無い回復環境でも動く。
//!
//! Range 要求に対応しているのが肝。数百 MiB を悪い回線で落とすので、
//! 中断のたびに最初からやり直すのは現実的でない。GitHub Releases は CDN 配信で
//! Range に応えるため、途中から再開できる。

use anyhow::{Context, Result, bail};
use std::io::Read;

/// 一度に読む量。大きすぎると失敗時に捨てる量が増える。
const READ_CHUNK: usize = 64 * 1024;

pub struct Fetched {
    pub reader: Box<dyn Read + Send>,
    /// サーバが Range を受け入れたか。受け入れていなければ先頭から来ている。
    pub ranged: bool,
}

/// `offset` バイト目から取得する。0 を渡せば全体。
///
/// サーバが Range を無視して 200 を返すことがある。その場合 `ranged` が false に
/// なるので、呼び出し側は先頭から来ているものとして扱う必要がある。
/// **黙って続けると、途中から書くつもりの場所へ先頭のデータを書いてしまう。**
pub fn get_from(url: &str, offset: u64) -> Result<Fetched> {
    let mut req = ureq::get(url);
    if offset > 0 {
        req = req.set("Range", &format!("bytes={offset}-"));
    }
    let resp = req
        .call()
        .with_context(|| format!("{url} を取得できない"))?;

    let status = resp.status();
    let ranged = status == 206;
    if offset > 0 && !ranged && status != 200 {
        bail!("{url}: 予期しない応答 {status}");
    }
    // Content-Length は読んでいない。どれだけ書くかは manifest の総サイズが
    // 決めるので、サーバの申告を信じる必要がない。
    Ok(Fetched {
        reader: resp.into_reader(),
        ranged,
    })
}

/// 小さなものを丸ごと取る。マニフェストと署名に使う。
///
/// `limit` を置いているのは、回復環境の RAM が有限だから。
/// マニフェストのつもりで巨大なものを渡されて落ちる余地を残さない。
pub fn get_all(url: &str, limit: usize) -> Result<Vec<u8>> {
    let f = get_from(url, 0)?;
    let mut out = Vec::new();
    let mut r = f.reader.take(limit as u64 + 1);
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() > limit {
            bail!("{url} が大きすぎる（上限 {limit} バイト）");
        }
    }
    Ok(out)
}
