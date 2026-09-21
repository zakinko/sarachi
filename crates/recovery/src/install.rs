//! 取得・検証・書き込みを繋ぐ。
//!
//! 順序に意味がある。**マニフェストの署名を先に確かめ、各チャンクを書く前に
//! ハッシュを照合する。** 逆にすると、未検証のバイトがディスクに載る。
//! 遠隔で入れ直す仕組みは、裏返せば遠隔でルートキットを配る経路そのものなので、
//! ここを緩めてよい理由はない。

use crate::fetch;
use anyhow::{Context, Result, bail};
use sarachi_image::VerifyingKey;
use sarachi_image::{SignedManifest, verify_and_write};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// マニフェストの上限。これを超えるものはマニフェストではない。
const MANIFEST_LIMIT: usize = 4 * 1024 * 1024;

/// 信頼する公開鍵を読む。
///
/// 回復パーティションに焼き込んでおく。**署名鍵のほうはオフラインに置き、
/// CI には決して入れない。** GitHub Releases から取ってくる以上、GitHub を
/// 一つ落とされた時にイメージと署名の両方が手に入る状態を作ってはいけない。
/// 信頼の根は GitHub の外に置く。
pub fn load_key(path: &Path) -> Result<VerifyingKey> {
    let mut b = Vec::new();
    File::open(path)
        .with_context(|| format!("公開鍵 {} を読めない", path.display()))?
        .read_to_end(&mut b)?;

    // 生の 32 バイトか、16 進 64 文字のどちらでも受ける。
    let raw: [u8; 32] = if b.len() == 32 {
        b.try_into().unwrap()
    } else {
        let t: String = String::from_utf8_lossy(&b)
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        if t.len() != 64 {
            bail!("公開鍵の形式が分からない（32 バイトか 16 進 64 文字）");
        }
        let mut k = [0u8; 32];
        for i in 0..32 {
            k[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16)?;
        }
        k
    };
    VerifyingKey::from_bytes(&raw).context("公開鍵として妥当でない")
}

pub struct Plan<'a> {
    pub manifest_url: &'a str,
    pub image_url: &'a str,
    pub target: &'a Path,
    pub resume_from: usize,
    pub commit: bool,
}

pub fn run(plan: &Plan, trusted: &VerifyingKey) -> Result<()> {
    println!("マニフェスト: {}", plan.manifest_url);
    let raw = fetch::get_all(plan.manifest_url, MANIFEST_LIMIT)?;
    println!("  {} バイト取得", raw.len());

    let signed = SignedManifest::decode(&raw)?;
    // ここを通らないマニフェストは使わない。
    let m = signed
        .verify(trusted)
        .context("マニフェストの署名検証に失敗")?;

    println!("  署名: 検証を通った");
    println!("  名前: {}", m.name);
    println!(
        "  大きさ: {} MiB（{} チャンク x {} MiB）",
        m.total_size / 1024 / 1024,
        m.chunks.len(),
        m.chunk_size / 1024 / 1024
    );

    if plan.resume_from > 0 {
        println!("  チャンク {} から再開", plan.resume_from);
    }

    if !plan.commit {
        println!("\ndry-run。実際に書くには --commit を付けること");
        return Ok(());
    }

    let offset = plan.resume_from as u64 * m.chunk_size;
    let got = fetch::get_from(plan.image_url, offset)?;

    // サーバが Range を無視して 200 を返すことがある。黙って続けると、
    // 途中から書くつもりの場所へ先頭のデータを書いてしまう。
    if plan.resume_from > 0 && !got.ranged {
        bail!(
            "再開を要求したがサーバが Range に応えていない。\n\
             先頭から書き直すか、Range に応える配布元を使うこと"
        );
    }

    let mut dst = OpenOptions::new()
        .write(true)
        .open(plan.target)
        .with_context(|| format!("{} を開けない", plan.target.display()))?;
    dst.seek(SeekFrom::Start(offset))?;

    let mut src = got.reader;
    let stats = verify_and_write(m, &mut src, &mut dst, plan.resume_from)?;
    dst.flush()?;

    println!(
        "\n{} に書いた（{} チャンク / {} MiB、{} チャンクは再開で飛ばした）",
        plan.target.display(),
        stats.chunks_written,
        stats.bytes_written / 1024 / 1024,
        stats.chunks_skipped
    );
    Ok(())
}
