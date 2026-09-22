//! 預かった鍵の置き場。
//!
//! 台ごとに一つのファイル。**小さく、目で追えることを優先する。** データ
//! ベースを挟まないのは、鍵を預かる物の中身が読み切れなくなるのが一番
//! まずいため。台数が増えて困るようになったら、その時に変える。

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("置き場を作れない: {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// 台の名前をファイル名にしてよいか。
    ///
    /// `..` や `/` を通すと置き場の外へ書ける。台の名前は外から来るので、
    /// ここで狭める。英数字と `-` と `_` だけ。
    fn safe(device_id: &str) -> bool {
        !device_id.is_empty()
            && device_id.len() <= 128
            && device_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    fn path(&self, device_id: &str) -> Result<PathBuf> {
        if !Self::safe(device_id) {
            bail!("台の名前に使えない文字がある: {device_id}");
        }
        Ok(self.dir.join(device_id))
    }

    /// 預かる。
    ///
    /// **既に在る鍵は上書きしない。** 同じ名前で二度来たら、二台目が一台目の
    /// 鍵を消してしまう。台を作り直したときは、先に失効させてから預ける。
    /// 上書きを許すと、事故で救えなくなる方向にしか転ばない。
    pub fn put(&self, device_id: &str, key: &[u8]) -> Result<()> {
        let p = self.path(device_id)?;
        if p.exists() {
            bail!("{device_id} の鍵は既に在る。作り直すなら先に失効させること");
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode_0600()
            .open(&p)
            .with_context(|| format!("鍵を書けない: {}", p.display()))?;
        f.write_all(key)?;
        f.sync_all()?;
        Ok(())
    }

    pub fn get(&self, device_id: &str) -> Result<Vec<u8>> {
        let p = self.path(device_id)?;
        std::fs::read(&p).with_context(|| format!("{device_id} の鍵が無い"))
    }

    /// 失効させる。**これが消去の本体になる台がある。**
    ///
    /// NetBSD の cgd は keyslot を持たないので、鍵を手元に置かず起動のたびに
    /// ここから取る形にする。その場合、消去はここで失効させることそのもの
    /// になる。だから「印を付けて隠す」ではなく、本当に消す。
    ///
    /// 上書きしてから消すのは、ファイルシステムが中身を残すことへの気休め
    /// ではある。本当の保証は「もう渡さない」ことの方で、そちらは確実。
    pub fn revoke(&self, device_id: &str) -> Result<()> {
        let p = self.path(device_id)?;
        if let (Ok(meta), Ok(mut f)) = (
            std::fs::metadata(&p),
            std::fs::OpenOptions::new().write(true).open(&p),
        ) {
            let _ = f.write_all(&vec![0u8; meta.len() as usize]);
            let _ = f.sync_all();
        }
        std::fs::remove_file(&p).with_context(|| format!("{device_id} の鍵を消せない"))
    }

    /// 在るか。試験で「失効させたら本当に消えたか」を見るのに使う。
    #[cfg(test)]
    pub fn exists(&self, device_id: &str) -> bool {
        self.path(device_id).map(|p| p.exists()).unwrap_or(false)
    }
}

/// `create_new` に権限を付ける。unix 以外では何もしない。
trait Mode0600 {
    fn mode_0600(&mut self) -> &mut Self;
}

impl Mode0600 for std::fs::OpenOptions {
    #[cfg(unix)]
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
    #[cfg(not(unix))]
    fn mode_0600(&mut self) -> &mut Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "sarachi-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn 預けて取り出して失効させられる() {
        let d = tmp();
        let s = Store::open(&d).unwrap();
        s.put("dev-1", &[1, 2, 3]).unwrap();
        assert_eq!(s.get("dev-1").unwrap(), vec![1, 2, 3]);
        s.revoke("dev-1").unwrap();
        assert!(!s.exists("dev-1"), "失効させたのに残っている");
        assert!(s.get("dev-1").is_err(), "失効後に取り出せてしまう");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 既に在る鍵は上書きしない() {
        // 同じ名前で二度来たら、二台目が一台目の鍵を消してしまう。
        // 上書きを許すと、事故で救えなくなる方向にしか転ばない。
        let d = tmp();
        let s = Store::open(&d).unwrap();
        s.put("dev-1", &[1]).unwrap();
        assert!(s.put("dev-1", &[2]).is_err(), "上書きできてしまった");
        assert_eq!(s.get("dev-1").unwrap(), vec![1], "元の鍵が壊れた");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 置き場の外へは書かせない() {
        // 台の名前は外から来る。
        let d = tmp();
        let s = Store::open(&d).unwrap();
        for bad in ["../x", "a/b", "", "..", "a b", "日本語"] {
            assert!(s.put(bad, &[1]).is_err(), "{bad:?} を通してしまった");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
