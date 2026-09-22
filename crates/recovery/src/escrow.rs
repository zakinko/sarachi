//! 鍵の逃がし方。
//!
//! 台ごとの鍵を作るのは [`crate::crypt`] だが、それをどこへ預けるかはここ。
//!
//! **預け先がそのまま「本当に消えたか」を決める。** 消去は鍵を壊すことで
//! 成り立っているので、控えがどこかに残っていれば消したことにならない。逆に
//! どこにも無ければ、消したい時ではなく**救いたい時**に台が戻らない。
//!
//! # 三枚重ねにする
//!
//! BitLocker と同じ形を採る。あちらは FVEK を VMK が包み、VMK を複数の
//! protector が包む。protector を足し引きしてもディスクを暗号化し直さずに
//! 済む構造で、LUKS の「マスター鍵＋keyslot」と同じ。
//!
//! | | 位置 | できること | 諦めるもの |
//! |---|---|---|---|
//! | control plane | **正** | 中央で失効できる。台が壊れても管理者が救える | 起動時に網が要る |
//! | TPM | 補助 | 網なしで自分で開く | **Pi に無い**。PCR が変われば開かない |
//! | パスフレーズ | 非常口 | 仕掛けが要らない | 無人で起動できない |
//!
//! **TPM を正に据えられないのは、標的に Raspberry Pi を含めたため。** そこが
//! BitLocker をそのまま写せない唯一の点で、裏を返せば control plane を正に
//! 置く判断は Pi を入れた時点でほぼ決まっている。
//!
//! Windows で回復パスワードが預けられる先は、今は Entra ID の device object。
//! 我々も同じ所へ預ければ、**管理者の手順が Windows と同じになる**。
//! `libhimmelblau` が使える以上、そこは他の Linux 用 MDM に無い筋になる。

use anyhow::{Context, Result, bail};
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
        Kind::Tpm => bail!(
            "TPM はまだ実装していない。実機で確かめるまで入れない。\n\
             なお TPM は補助であって正ではない。Raspberry Pi が持たないので、\n\
             これを前提にした設計にはできない"
        ),
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
        if !self.base.starts_with("https://") {
            bail!(
                "control plane は https でなければならない（{}）。\n\
                 鍵を本文に入れるので、平文で出すわけにいかない",
                self.base
            );
        }
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
    fn tpmは断る() {
        let err = match for_kind(Kind::Tpm) {
            Ok(_) => panic!("実装していないのに通った"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("実装していない"), "{err}");
    }
}
