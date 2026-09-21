//! 暗号層。
//!
//! ブロック層で暗号化し、鍵を破棄して消す、という一つの形で全標的を覆う。
//! 実装は OS ごとに違う（Linux は LUKS、FreeBSD と DragonFly は geli、
//! NetBSD は cgd、OpenBSD は softraid crypto）が、上から見た振る舞いは同じに
//! なるよう trait にしてある。**消去の側のコードが OS ごとに分岐しない**のが
//! この形の一番の利点で、消去は設計全体の土台なので、そこを揃える価値がある。
//!
//! `open` が開いた先のパスを返すのは、それが OS ごとに違うため。LUKS は
//! `/dev/mapper/<name>`、geli は `<device>.eli`、cgd は `/dev/cgd<N>`、
//! OpenBSD は新しく現れる `/dev/sd<N>` になる。呼び出し側が組み立てられない。
//!
//! 鍵はここで作る。**イメージには一切入らない。** 配ったイメージに鍵が
//! 入っていると全台が同じマスター鍵を持ち、一台で鍵を破棄しても同じイメージを
//! 持つ者は誰でも復号できる。配ってしまってからでは取り返しがつかない。

use anyhow::{Context, Result, bail};
use sarachi_disk::RootKind;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 鍵の長さ。暗号層に渡す乱数で、マスター鍵そのものではない。
const KEY_BYTES: usize = 64;

pub trait Crypt {
    /// 記録と報告のための名前。
    fn name(&self) -> &'static str;

    /// 新しい鍵で容器を作る。**中身は失われる。**
    fn format(&self, device: &Path, key: &[u8]) -> Result<()>;

    /// 開いて、中身が現れたパスを返す。
    fn open(&self, device: &Path, key: &[u8], name: &str) -> Result<PathBuf>;

    fn close(&self, name: &str) -> Result<()>;

    /// crypto-erase。**これが消去の本体。**
    ///
    /// 上書きではなく鍵の破棄で消すのは、速いからではなく、SD と SSD では
    /// ウェアレベリングのせいで上書きが消去として成立しないため。
    /// Raspberry Pi を対象に含める以上、これ以外に手が無い。
    fn erase(&self, device: &Path) -> Result<()>;

    /// 自分の容器かどうか。消す前に確かめる。
    fn is_container(&self, device: &Path) -> bool;
}

/// root に載る物から暗号層を選ぶ。
///
/// 実装していない OS では、黙って別の手を使うのではなく断る。消去は
/// 取り返しがつかないので、確かめていない経路を通してはいけない。
pub fn for_kind(kind: RootKind) -> Result<Box<dyn Crypt>> {
    match kind {
        RootKind::LinuxLuks => Ok(Box::new(Luks)),
        RootKind::FreeBsdZfs | RootKind::FreeBsdUfs => bail!(
            "geli はまだ実装していない（FreeBSD / GhostBSD / DragonFly）。\n\
             実機で確かめるまで入れない"
        ),
        RootKind::NetBsdCgd | RootKind::NetBsdFfs => {
            bail!("cgd はまだ実装していない（NetBSD）。実機で確かめるまで入れない")
        }
        RootKind::OpenBsdData => {
            bail!("softraid crypto はまだ実装していない（OpenBSD）。実機で確かめるまで入れない")
        }
    }
}

/// 台ごとの鍵を作る。
///
/// `/dev/urandom` から取る。Linux の `urandom` は初期化後は `random` と同じ
/// 品質で、初期化前は読みが待たされるだけなので、`crng init done` を待つ必要は
/// ない。
pub fn generate_key() -> Result<Vec<u8>> {
    let mut key = vec![0u8; KEY_BYTES];
    std::fs::File::open("/dev/urandom")
        .context("/dev/urandom を開けない")?
        .read_exact(&mut key)?;
    Ok(key)
}

// ---------------------------------------------------------------- LUKS

pub struct Luks;

impl Luks {
    /// 絶対パスで呼ぶ。initramfs の PATH は当てにできず、
    /// `Command::new("cryptsetup")` は spawn 時に ENOENT を返す。その誤りは
    /// 「cryptsetup が壊れている」とも「ライブラリが足りない」とも読めるので、
    /// 原因にたどり着くのに手間がかかる。
    fn exe() -> &'static str {
        for p in [
            "/sbin/cryptsetup",
            "/usr/sbin/cryptsetup",
            "/bin/cryptsetup",
        ] {
            if Path::new(p).exists() {
                return p;
            }
        }
        "cryptsetup"
    }

    fn run(what: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<()> {
        use std::io::Write;
        use std::process::Stdio;

        let exe = Self::exe();
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
}

impl Crypt for Luks {
    fn name(&self) -> &'static str {
        "LUKS2"
    }

    fn format(&self, device: &Path, key: &[u8]) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        Self::run(
            "LUKS2 の作成",
            &[
                "luksFormat",
                "--type",
                "luks2",
                "--batch-mode",
                "--pbkdf",
                "argon2id",
                // 回復環境の RAM は有限で、Pi では 2GB しかない。既定のまま
                // argon2id を回すと空きメモリの半分を取りにいって落ちうる。
                "--pbkdf-memory",
                "65536",
                "--key-file",
                "-",
                &dev,
            ],
            Some(key),
        )
    }

    fn open(&self, device: &Path, key: &[u8], name: &str) -> Result<PathBuf> {
        let dev = device.to_string_lossy().to_string();
        Self::run(
            "LUKS の展開",
            &["open", "--key-file", "-", &dev, name],
            Some(key),
        )?;
        Ok(Path::new("/dev/mapper").join(name))
    }

    fn close(&self, name: &str) -> Result<()> {
        Self::run("LUKS の閉鎖", &["close", name], None)
    }

    fn erase(&self, device: &Path) -> Result<()> {
        let dev = device.to_string_lossy().to_string();
        Self::run("crypto-erase", &["luksErase", "--batch-mode", &dev], None)
    }

    fn is_container(&self, device: &Path) -> bool {
        let dev = device.to_string_lossy().to_string();
        Command::new(Self::exe())
            .args(["isLuks", &dev])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linuxは実装がある() {
        let c = match for_kind(RootKind::LinuxLuks) {
            Ok(c) => c,
            Err(e) => panic!("Linux で実装が見つからない: {e}"),
        };
        assert_eq!(c.name(), "LUKS2");
    }

    #[test]
    fn 実装していないosは断る() {
        // 黙って別の手を使ってはいけない。消去は取り返しがつかないので、
        // 確かめていない経路は通さない。
        for k in [
            RootKind::FreeBsdZfs,
            RootKind::FreeBsdUfs,
            RootKind::NetBsdCgd,
            RootKind::NetBsdFfs,
            RootKind::OpenBsdData,
        ] {
            let e = match for_kind(k) {
                Ok(_) => panic!("{k:?} は実装していないのに通った"),
                Err(e) => e,
            };
            assert!(
                e.to_string().contains("実装していない"),
                "{k:?} の断り方: {e}"
            );
        }
    }

    #[test]
    fn 鍵は毎回異なり十分に長い() {
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        assert_eq!(a.len(), KEY_BYTES);
        assert_ne!(a, b, "同じ鍵が二度出た");
    }
}
