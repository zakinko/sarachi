//! LUKS の操作。
//!
//! LUKS2 を自前で書き起こしていないのは、大きすぎるからというより、
//! **鍵の扱いを一箇所でも間違えれば消去そのものが成立しなくなる**ため。
//! 実績のある実装に任せる。
//!
//! 鍵はここで生成する。**イメージには一切入らない。** dd で配ったイメージに
//! 鍵が入っていると、全台が同じマスター鍵を持つことになり、一台で鍵を破棄しても
//! 同じイメージを持つ者は誰でも復号できる。LUKS のマスター鍵は後から変更できない
//! ので、これは配った後では取り返しがつかない。

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::Path;
use std::process::Command;

/// 鍵の長さ。LUKS のパスフレーズとして使う乱数で、マスター鍵そのものではない。
const KEY_BYTES: usize = 64;

/// 絶対パスで呼ぶ。initramfs の PATH は当てにできず、`Command::new("cryptsetup")`
/// は spawn 時に ENOENT を返す。その誤りは「cryptsetup が壊れている」ようにも
/// 「ライブラリが足りない」ようにも読めるので、原因にたどり着くのに手間がかかる。
fn cryptsetup_path() -> &'static str {
    for p in ["/sbin/cryptsetup", "/usr/sbin/cryptsetup", "/bin/cryptsetup"] {
        if Path::new(p).exists() {
            return p;
        }
    }
    "cryptsetup"
}

fn run(what: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let exe = cryptsetup_path();
    let mut c = Command::new(exe);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        c.stdin(Stdio::piped());
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("{exe} を起動できない（{what}）"))?;
    if let (Some(data), Some(mut s)) = (stdin, child.stdin.take()) {
        s.write_all(data)?;
        drop(s);
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{what} に失敗（{}）\n  {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// 台ごとの鍵を作る。
///
/// `/dev/urandom` から取るのは、回復環境が起動直後で乱数プールが浅い可能性が
/// あるため……ではなく、Linux の `urandom` は初期化後は `random` と同じ品質で、
/// 初期化前は読みが待たされるだけだから。カーネルログの
/// `random: crng init done` を待つ必要はない。
pub fn generate_key() -> Result<Vec<u8>> {
    let mut key = vec![0u8; KEY_BYTES];
    std::fs::File::open("/dev/urandom")
        .context("/dev/urandom を開けない")?
        .read_exact(&mut key)?;
    Ok(key)
}

/// 新しい鍵で LUKS2 コンテナを作る。**中身は失われる。**
pub fn format(device: &Path, key: &[u8]) -> Result<()> {
    let dev = device.to_string_lossy().to_string();
    run(
        "LUKS2 の作成",
        &[
            "luksFormat",
            "--type", "luks2",
            "--batch-mode",
            "--pbkdf", "argon2id",
            // 回復環境の RAM は有限で、Pi では 2GB しかない。既定のまま
            // argon2id を回すと空きメモリの半分を取りにいって落ちうる。
            "--pbkdf-memory", "65536",
            "--key-file", "-",
            &dev,
        ],
        Some(key),
    )
}

pub fn open(device: &Path, key: &[u8], name: &str) -> Result<()> {
    let dev = device.to_string_lossy().to_string();
    run("LUKS の展開", &["open", "--key-file", "-", &dev, name], Some(key))
}

pub fn close(name: &str) -> Result<()> {
    run("LUKS の閉鎖", &["close", name], None)
}

/// crypto-erase。鍵スロットを破棄する。
///
/// **これが消去の本体。** 上書きではなく鍵の破棄で消すのは、速いからではなく、
/// SD や SSD ではウェアレベリングのせいで上書きが消去として成立しないため。
/// Raspberry Pi を対象に含める以上、これ以外に手が無い。
/// 所要時間は容量に依存しない（実測 20ms、4GiB の全面上書きは 9,260ms）。
pub fn erase(device: &Path) -> Result<()> {
    let dev = device.to_string_lossy().to_string();
    run("crypto-erase", &["luksErase", "--batch-mode", &dev], None)
}

/// LUKS コンテナかどうか。消す前に確かめる。
pub fn is_luks(device: &Path) -> bool {
    let dev = device.to_string_lossy().to_string();
    Command::new(cryptsetup_path())
        .args(["isLuks", &dev])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
