//! 鍵の逃がし方。
//!
//! 鍵を作るのは [`crate::crypt`]、預け先を決めるのはここ。消去は鍵を壊す
//! ことで成り立つので、**預け先がそのまま「本当に消えたか」を決める**。
//! 控えが残っていれば消したことにならず、どこにも無ければ救えない。
//!
//! BitLocker と同じく、一つの鍵を複数の手段で包む。control plane を正、
//! TPM を補助、パスフレーズを非常口とする。
//!
//! **TPM を正に据えないのは Raspberry Pi が持たないため。** 標的に含めた
//! 時点で、網の向こうに預ける形が必然になっている。NetBSD ではさらに強く、
//! cgd が keyslot を持たないので control plane 以外に健全な消去が無い。

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::PathBuf;

pub trait Escrow {
    /// 記録と報告のための名前。
    fn name(&self) -> &'static str;

    /// 鍵を預ける。
    ///
    /// **ここが成功しない限り、導入を先へ進めてはいけない。** 鍵がどこにも
    /// 無い台は、暗号化されているが誰も開けられない塊になる。消去の前に
    /// 救済が壊れる形で、しかも壊れたことは次に開こうとするまで分からない。
    fn store(&self, device_id: &str, key: &[u8]) -> Result<()>;

    /// 預けた鍵を取り戻す。
    ///
    /// 起動のたびにここから取る台がある。**NetBSD がそれ。** cgd は keyslot を
    /// 持たないので、鍵を手元に置くと消去がただのファイル削除になり、SD や
    /// SSD で成立しなくなる。手元に置かずここから取れば、消す物が手元に無い。
    fn fetch(&self, device_id: &str) -> Result<Vec<u8>>;
}

/// 預け先の選び方。
#[derive(Clone)]
pub enum Kind {
    /// ファイルへ書く。**試験用。**
    File(PathBuf),
    /// control plane へ預ける。本番の正。
    ControlPlane { base: String, token: String },
    /// TPM に封印する。補助。**まだ実装していない。**
    Tpm,
}

/// 預け先を選ぶ。
///
/// 実装していない物は、黙って別の手に落ちずに断る。暗号層と同じ扱いにする
/// のは、鍵の行き先が思っていたのと違うことが、消去の失敗と同じ重さを持つ
/// ため。預けたつもりで預かられていなければ、救えないか、消えていないかの
/// どちらかになる。
pub fn for_kind(kind: Kind) -> Result<Box<dyn Escrow>> {
    match kind {
        Kind::File(p) => Ok(Box::new(FileEscrow { path: p })),
        Kind::ControlPlane { base, token } => Ok(Box::new(ControlPlaneEscrow { base, token })),
        Kind::Tpm => bail!("TPM はまだ実装していない。実機で確かめるまで入れない"),
    }
}

// ---------------------------------------------------------------- ファイル

/// ファイルへ書く。試験用で、本番では使わない。
pub struct FileEscrow {
    path: PathBuf,
}

impl Escrow for FileEscrow {
    fn name(&self) -> &'static str {
        "ファイル（試験用）"
    }

    fn store(&self, _device_id: &str, key: &[u8]) -> Result<()> {
        std::fs::write(&self.path, key)
            .with_context(|| format!("鍵を {} に書けない", self.path.display()))
    }

    fn fetch(&self, _device_id: &str) -> Result<Vec<u8>> {
        std::fs::read(&self.path)
            .with_context(|| format!("鍵を {} から読めない", self.path.display()))
    }
}

// ---------------------------------------------------------------- control plane

/// control plane へ預ける。
///
/// 鍵そのものを本文に入れるので、**平文の http では使わない**。呼ぶ側では
/// なくここで断るのは、設定を書き間違えたときに黙って鍵が網へ出るのが一番
/// まずいため。
pub struct ControlPlaneEscrow {
    base: String,
    token: String,
}

impl ControlPlaneEscrow {
    /// 平文で鍵をやりとりさせない。
    ///
    /// 呼ぶ側ではなくここで断るのは、設定を書き間違えたときに黙って鍵が網へ
    /// 出るのが一番まずいため。預けるときも取るときも通る。
    fn require_https(&self) -> Result<()> {
        if !self.base.starts_with("https://") {
            bail!(
                "control plane は https でなければならない（{}）。\n\
                 鍵をやりとりするので、平文で出すわけにいかない",
                self.base
            );
        }
        Ok(())
    }

    /// 鍵は本文に入れる。URL に入れるとサーバの access log と、途中の
    /// proxy の log に残る。
    fn body(device_id: &str, key: &[u8]) -> String {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        format!("{{\"device_id\":\"{device_id}\",\"key\":\"{hex}\"}}")
    }
}

impl Escrow for ControlPlaneEscrow {
    fn name(&self) -> &'static str {
        "control plane"
    }

    fn store(&self, device_id: &str, key: &[u8]) -> Result<()> {
        self.require_https()?;
        let url = format!("{}/v1/escrow", self.base.trim_end_matches('/'));
        let res = ureq::post(&url)
            .set("authorization", &format!("Bearer {}", self.token))
            .set("content-type", "application/json")
            .send_string(&Self::body(device_id, key));
        match res {
            Ok(_) => Ok(()),
            Err(e) => bail!("control plane に鍵を預けられない（{url}）: {e}"),
        }
    }

    fn fetch(&self, device_id: &str) -> Result<Vec<u8>> {
        self.require_https()?;
        let url = format!("{}/v1/key/{device_id}", self.base.trim_end_matches('/'));
        let res = ureq::get(&url)
            .set("authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| anyhow::anyhow!("control plane から鍵を取れない（{url}）: {e}"))?;
        let mut key = Vec::new();
        res.into_reader()
            .read_to_end(&mut key)
            .context("鍵を読み切れない")?;
        if key.is_empty() {
            bail!("control plane が空の鍵を返した（{url}）");
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ファイルへ預けられる() {
        let d = std::env::temp_dir().join(format!("sarachi-escrow-{}", std::process::id()));
        let e = for_kind(Kind::File(d.clone())).expect("ファイルは実装がある");
        e.store("dev-1", &[1, 2, 3, 4]).expect("預けられない");
        assert_eq!(std::fs::read(&d).unwrap(), vec![1, 2, 3, 4]);
        let _ = std::fs::remove_file(&d);
    }

    #[test]
    fn 平文のhttpには預けない() {
        // 設定を書き間違えたときに黙って鍵が網へ出るのが一番まずい。
        let e = for_kind(Kind::ControlPlane {
            base: "http://example.invalid".into(),
            token: "t".into(),
        })
        .expect("control plane は実装がある");
        let err = match e.store("dev-1", &[0u8; 64]) {
            Ok(()) => panic!("平文の http に預けてしまった"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("https"), "断り方: {err}");
    }

    #[test]
    fn 鍵は本文に入れる() {
        // URL に入れるとサーバと途中の proxy の log に残る。
        let b = ControlPlaneEscrow::body("dev-1", &[0xde, 0xad, 0xbe, 0xef]);
        assert!(b.contains("\"key\":\"deadbeef\""), "{b}");
        assert!(b.contains("\"device_id\":\"dev-1\""), "{b}");
    }

    #[test]
    fn ファイルから取り戻せる() {
        let d = std::env::temp_dir().join(format!("sarachi-escrow-f-{}", std::process::id()));
        let e = for_kind(Kind::File(d.clone())).unwrap();
        e.store("dev-1", &[9, 8, 7]).unwrap();
        assert_eq!(e.fetch("dev-1").unwrap(), vec![9, 8, 7]);
        let _ = std::fs::remove_file(&d);
    }

    #[test]
    fn 取り戻すときも平文のhttpは断る() {
        let e = for_kind(Kind::ControlPlane {
            base: "http://example.invalid".into(),
            token: "t".into(),
        })
        .unwrap();
        let err = match e.fetch("dev-1") {
            Ok(_) => panic!("平文の http から取ってしまった"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("https"), "断り方: {err}");
    }

    #[test]
    fn tpmは断る() {
        let err = match for_kind(Kind::Tpm) {
            Ok(_) => panic!("実装していないのに通った"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("実装していない"), "{err}");
    }
}
