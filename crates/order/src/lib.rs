//! 署名付きの消去命令。
//!
//! この型が本プロジェクトの背骨にあたる。Intune から来ようが自前の
//! control plane から来ようが手で持ち込まれようが、**まずこの形に正規化**
//! してから扱う。エージェントの中核は Microsoft を一切知らない。
//!
//! 段階は Windows の RemoteWipe CSP をそのまま意味論として採っている。
//! Windows が doWipe と doWipeProtected を分けているのは、前者が「電源を
//! 入れ直すだけで回避できる」ためで、紛失・盗難時には後者を使えと
//! 明記されている。この区別は真似る価値がある。

use anyhow::{Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

mod codec;
pub use codec::MAGIC;

/// 消去の段階。Windows の三段に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 消して戻すが、登録と Wi-Fi 設定は保つ。
    /// Windows の `AutomaticRedeployment`（Autopilot Reset）に相当。
    Reset,
    /// 消して初期状態まで戻す。中断されたら巻き戻す。
    /// Windows の `doWipe` に相当し、同じく電源断で回避できる。
    Factory,
    /// 終わるまで再試行し、失敗・中断時はパーティションも消す。
    /// Windows の `doWipeProtected` に相当。紛失・盗難時はこれを使う。
    Destroy,
}

impl Level {
    pub(crate) fn as_u8(self) -> u8 {
        match self {
            Level::Reset => 1,
            Level::Factory => 2,
            Level::Destroy => 3,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            1 => Level::Reset,
            2 => Level::Factory,
            3 => Level::Destroy,
            _ => bail!("未知の消去段階: {v}"),
        })
    }

    /// 回復領域を残すか。第 3 段だけが消す。
    /// 残す理由は、第 1 段と第 2 段が「消してから入れ直す」であり、
    /// 入れ直す主体が回復領域にいるため。第 3 段は入れ直さないので消してよい。
    pub fn keeps_recovery(self) -> bool {
        self != Level::Destroy
    }

    /// 妨害されても続行するか。Windows が doWipeProtected を用意した理由。
    pub fn persists(self) -> bool {
        self == Level::Destroy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// この命令が宛てられた機体。一致しなければ実行しない。
    pub device_id: String,
    pub level: Level,
    /// 再送を弾くための一回限りの値。
    pub nonce: [u8; 16],
    pub issued_at: u64,
    pub expires_at: u64,
    /// 発行元の名前。監査のために残す。検証には使わない。
    pub issuer: String,
}

impl Order {
    /// 署名対象の正規形。ここが曖昧だと、同じ意味で違うバイト列が作れてしまい
    /// 署名の意味が崩れるので、順序も長さの持ち方も固定する。
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        codec::encode(self)
    }

    pub fn from_canonical_bytes(b: &[u8]) -> Result<Self> {
        codec::decode(b)
    }

    pub fn sign(self, key: &SigningKey) -> SignedOrder {
        let sig = key.sign(&self.to_canonical_bytes());
        SignedOrder { order: self, signature: sig.to_bytes() }
    }
}

#[derive(Debug, Clone)]
pub struct SignedOrder {
    pub order: Order,
    pub signature: [u8; 64],
}

/// 検証で弾かれた理由。呼び出し側が記録できるように型で返す。
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    BadSignature,
    Expired { now: u64, expires_at: u64 },
    NotYetValid { now: u64, issued_at: u64 },
    WrongDevice { expected: String, got: String },
    ReplayedNonce,
}

impl std::fmt::Display for Reject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reject::BadSignature => write!(f, "署名が一致しない"),
            Reject::Expired { now, expires_at } =>
                write!(f, "期限切れ（現在 {now}、期限 {expires_at}）"),
            Reject::NotYetValid { now, issued_at } =>
                write!(f, "発行時刻が未来（現在 {now}、発行 {issued_at}）"),
            Reject::WrongDevice { expected, got } =>
                write!(f, "宛先が違う（自分は {expected}、命令は {got}）"),
            Reject::ReplayedNonce => write!(f, "使用済みの nonce"),
        }
    }
}

impl std::error::Error for Reject {}

/// 見た nonce を覚えておく側。永続化の仕方は実装者に任せる。
pub trait NonceLog {
    fn seen(&self, nonce: &[u8; 16]) -> bool;
    fn remember(&mut self, nonce: &[u8; 16]);
}

impl SignedOrder {
    /// 実行してよいかを判定する。
    ///
    /// 順序に意味がある。**署名を最初に確かめる**のは、署名が通っていない
    /// 入力の中身について、期限も宛先も語る意味がないため。
    pub fn verify(
        &self,
        trusted: &VerifyingKey,
        self_device_id: &str,
        now: u64,
        log: &mut dyn NonceLog,
    ) -> std::result::Result<&Order, Reject> {
        let sig = Signature::from_bytes(&self.signature);
        trusted
            .verify(&self.order.to_canonical_bytes(), &sig)
            .map_err(|_| Reject::BadSignature)?;

        if now > self.order.expires_at {
            return Err(Reject::Expired { now, expires_at: self.order.expires_at });
        }
        // 発行時刻が未来の命令は、時計のずれか細工のどちらか。
        // どちらにせよ消去を始めてよい根拠にはならない。
        if now < self.order.issued_at {
            return Err(Reject::NotYetValid { now, issued_at: self.order.issued_at });
        }
        if self.order.device_id != self_device_id {
            return Err(Reject::WrongDevice {
                expected: self_device_id.to_string(),
                got: self.order.device_id.clone(),
            });
        }
        if log.seen(&self.order.nonce) {
            return Err(Reject::ReplayedNonce);
        }
        log.remember(&self.order.nonce);
        Ok(&self.order)
    }
}

/// 試験と鍵生成のため。実運用の署名鍵はオフラインで作り、CI には置かない。
pub fn generate_key() -> SigningKey {
    SigningKey::generate(&mut rand_core::OsRng)
}

pub fn random_nonce() -> [u8; 16] {
    use rand_core::RngCore;
    let mut n = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut n);
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct MemLog(HashSet<[u8; 16]>);
    impl NonceLog for MemLog {
        fn seen(&self, n: &[u8; 16]) -> bool { self.0.contains(n) }
        fn remember(&mut self, n: &[u8; 16]) { self.0.insert(*n); }
    }

    const NOW: u64 = 1_789_000_500;
    const DEV: &str = "test-device-01";

    fn order() -> Order {
        Order {
            device_id: DEV.into(),
            level: Level::Destroy,
            nonce: random_nonce(),
            issued_at: NOW - 100,
            expires_at: NOW + 3600,
            issuer: "test".into(),
        }
    }

    #[test]
    fn 正しい命令は通る() {
        let k = generate_key();
        let signed = order().sign(&k);
        let mut log = MemLog::default();
        assert!(signed.verify(&k.verifying_key(), DEV, NOW, &mut log).is_ok());
    }

    #[test]
    fn 別の鍵では通らない() {
        let signed = order().sign(&generate_key());
        let other = generate_key();
        let mut log = MemLog::default();
        assert_eq!(
            signed.verify(&other.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::BadSignature
        );
    }

    #[test]
    fn 中身を書き換えると通らない() {
        let k = generate_key();
        let mut signed = order().sign(&k);
        // 段階だけを Destroy に上げる、という一番やられたくない改竄。
        signed.order.level = Level::Reset;
        let mut log = MemLog::default();
        assert_eq!(
            signed.verify(&k.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::BadSignature
        );
    }

    #[test]
    fn 署名だけ別の命令から持ってきても通らない() {
        let k = generate_key();
        let a = order().sign(&k);
        let mut b = order().sign(&k);
        b.signature = a.signature;
        let mut log = MemLog::default();
        assert_eq!(
            b.verify(&k.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::BadSignature
        );
    }

    #[test]
    fn 期限切れは通らない() {
        let k = generate_key();
        let mut o = order();
        o.expires_at = NOW - 1;
        let signed = o.sign(&k);
        let mut log = MemLog::default();
        assert!(matches!(
            signed.verify(&k.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::Expired { .. }
        ));
    }

    #[test]
    fn 発行時刻が未来なら通らない() {
        // 時計のずれか細工か。どちらにせよ消去を始めてよい根拠にはならない。
        let k = generate_key();
        let mut o = order();
        o.issued_at = NOW + 1;
        let signed = o.sign(&k);
        let mut log = MemLog::default();
        assert!(matches!(
            signed.verify(&k.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::NotYetValid { .. }
        ));
    }

    #[test]
    fn 他人宛は通らない() {
        let k = generate_key();
        let signed = order().sign(&k);
        let mut log = MemLog::default();
        assert!(matches!(
            signed.verify(&k.verifying_key(), "someone-else", NOW, &mut log).unwrap_err(),
            Reject::WrongDevice { .. }
        ));
    }

    #[test]
    fn 同じ命令は二度通らない() {
        // 一度成功した消去命令を後から流し直されると、再導入したばかりの
        // 機体がまた消される。
        let k = generate_key();
        let signed = order().sign(&k);
        let mut log = MemLog::default();
        assert!(signed.verify(&k.verifying_key(), DEV, NOW, &mut log).is_ok());
        assert_eq!(
            signed.verify(&k.verifying_key(), DEV, NOW, &mut log).unwrap_err(),
            Reject::ReplayedNonce
        );
    }

    #[test]
    fn 弾かれた命令のnonceは使われたことにしない() {
        // 署名が通らない入力で nonce を消費できると、正規の命令を
        // 先回りして潰せてしまう。
        let k = generate_key();
        let o = order();
        let nonce = o.nonce;
        let bad = o.clone().sign(&generate_key());
        let mut log = MemLog::default();
        assert!(bad.verify(&k.verifying_key(), DEV, NOW, &mut log).is_err());
        assert!(!log.seen(&nonce), "弾いたのに nonce を消費している");

        let good = o.sign(&k);
        assert!(good.verify(&k.verifying_key(), DEV, NOW, &mut log).is_ok());
    }

    #[test]
    fn 段階ごとの性質がwindowsと対応する() {
        assert!(Level::Reset.keeps_recovery());
        assert!(Level::Factory.keeps_recovery());
        assert!(!Level::Destroy.keeps_recovery(), "第3段は回復領域も消す");

        assert!(!Level::Reset.persists());
        assert!(!Level::Factory.persists(), "doWipe は電源断で回避できる");
        assert!(Level::Destroy.persists(), "doWipeProtected は終わるまで続ける");
    }
}
