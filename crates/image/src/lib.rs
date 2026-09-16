//! 署名付きイメージマニフェストと、書き込み前に検証する取り込み。
//!
//! 「遠隔で消して入れ直す」は、裏返せば**遠隔でルートキットを配る経路**そのもの
//! なので、設計全体で最も危険な場所がここにあたる。
//!
//! 大きなイメージの署名を末尾でまとめて検証すると、検証が終わる頃には
//! 未検証のデータを既にディスクへ書き終えている。それでは守れないので、
//! **チャンクごとのハッシュを署名済みマニフェストに入れ、各チャンクを書く前に
//! 照合する**。署名の検証は小さなマニフェストに対して一度だけ行い、
//! 以降はハッシュの照合だけで済む。未検証のバイトがディスクに触れることはない。
//!
//! 鍵の扱いについては、公開鍵を回復パーティションに焼き込み、**署名鍵は
//! オフラインに置いて CI には決して入れない**。GitHub Releases から取ってくる
//! 以上、GitHub を一つ落とされた時にイメージと署名の両方が手に入る状態を
//! 作ってはいけない。信頼の根は GitHub の外に置く。

use anyhow::{Result, bail};
use ed25519_dalek::{Signature, Verifier as _};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

mod codec;
pub use codec::MAGIC;

// 利用側が ed25519-dalek を直接引かずに済むよう、鍵の型だけ通す。
// 回復環境の依存は少ないほどよい。
pub use ed25519_dalek::VerifyingKey;

/// 既定のチャンク長。小さくするとマニフェストが太り、大きくすると
/// 再開の粒度が粗くなる。8MiB なら 512MiB のイメージで 64 個。
pub const DEFAULT_CHUNK: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// どのイメージか。取り違えを防ぐために署名対象に含める。
    pub name: String,
    pub total_size: u64,
    pub chunk_size: u64,
    /// 各チャンクの SHA-256。最後のチャンクだけは短いことがある。
    pub chunks: Vec<[u8; 32]>,
}

impl Manifest {
    /// 手元のデータからマニフェストを作る。署名する側が使う。
    pub fn build(name: &str, data: &[u8], chunk_size: u64) -> Result<Self> {
        if chunk_size == 0 {
            bail!("チャンク長が 0");
        }
        let chunks = data
            .chunks(chunk_size as usize)
            .map(|c| {
                let mut h = Sha256::new();
                h.update(c);
                h.finalize().into()
            })
            .collect();
        Ok(Manifest {
            name: name.to_string(),
            total_size: data.len() as u64,
            chunk_size,
            chunks,
        })
    }

    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        codec::encode(self)
    }

    pub fn from_canonical_bytes(b: &[u8]) -> Result<Self> {
        codec::decode(b)
    }

    /// 指定のチャンクが何バイトあるはずか。末尾だけ短くなる。
    pub fn chunk_len(&self, index: usize) -> u64 {
        let start = index as u64 * self.chunk_size;
        (self.total_size - start).min(self.chunk_size)
    }
}

#[derive(Debug, Clone)]
pub struct SignedManifest {
    pub manifest: Manifest,
    pub signature: [u8; 64],
}

impl SignedManifest {
    /// 配布される形。正規形のバイト列の後ろに 64 バイトの署名を付ける。
    /// 署名を別ファイルにしないのは、取り違えと片落ちを避けるため。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.manifest.to_canonical_bytes();
        out.extend_from_slice(&self.signature);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < 64 {
            bail!("署名付きマニフェストとしては短すぎる");
        }
        let (body, sig) = b.split_at(b.len() - 64);
        Ok(SignedManifest {
            manifest: Manifest::from_canonical_bytes(body)?,
            signature: sig.try_into().unwrap(),
        })
    }

    /// 署名を確かめてマニフェストを取り出す。
    /// **これを通っていないマニフェストを使ってはいけない。**
    pub fn verify(&self, trusted: &VerifyingKey) -> Result<&Manifest> {
        let sig = Signature::from_bytes(&self.signature);
        if trusted.verify(&self.manifest.to_canonical_bytes(), &sig).is_err() {
            bail!("マニフェストの署名が一致しない");
        }
        // 自己矛盾したマニフェストは、署名が通っていても受けない。
        // 署名鍵が正しくても、内容が壊れていれば書けない。
        let expect = self.manifest.total_size.div_ceil(self.manifest.chunk_size) as usize;
        if self.manifest.chunks.len() != expect {
            bail!(
                "チャンク数が総サイズと合わない（{} 個あるが {} 個のはず）",
                self.manifest.chunks.len(), expect
            );
        }
        Ok(&self.manifest)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub chunks_written: usize,
    pub bytes_written: u64,
    pub chunks_skipped: usize,
}

/// 検証しながら書き出す。
///
/// `from_chunk` を指定すると途中から再開する。数百 MiB を悪い回線で落とすので、
/// 中断のたびに最初からやり直すのは現実的でない。GitHub Releases は Range
/// 要求に応えるので、呼び出し側はそのチャンクの先頭バイトから読ませればよい。
///
/// 照合に失敗したチャンクは**書かずに**その場で止める。既に書いたぶんは
/// 残るが、それらは検証を通ったものだけなので、続きから再開できる。
pub fn verify_and_write<R: Read, W: Write>(
    manifest: &Manifest,
    src: &mut R,
    dst: &mut W,
    from_chunk: usize,
) -> Result<Stats> {
    if from_chunk > manifest.chunks.len() {
        bail!("再開位置 {} がチャンク数 {} を越えている", from_chunk, manifest.chunks.len());
    }
    let mut stats = Stats { chunks_skipped: from_chunk, ..Default::default() };
    let mut buf = vec![0u8; manifest.chunk_size as usize];

    for index in from_chunk..manifest.chunks.len() {
        let want = manifest.chunk_len(index) as usize;
        src.read_exact(&mut buf[..want]).map_err(|e| {
            anyhow::anyhow!("チャンク {} を読めない（{} バイト要る）: {}", index, want, e)
        })?;

        let mut h = Sha256::new();
        h.update(&buf[..want]);
        let got: [u8; 32] = h.finalize().into();

        // ここが要。照合してから書く。逆にすると、未検証のバイトが
        // ディスクに載ってしまう。
        if got != manifest.chunks[index] {
            bail!(
                "チャンク {} のハッシュが一致しない。書かずに中止した\n  期待 {}\n  実際 {}",
                index, hex(&manifest.chunks[index]), hex(&got)
            );
        }

        dst.write_all(&buf[..want])?;
        stats.chunks_written += 1;
        stats.bytes_written += want as u64;
    }
    dst.flush()?;
    Ok(stats)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Cursor;

    const CHUNK: u64 = 64;

    fn key() -> SigningKey {
        SigningKey::generate(&mut rand_core::OsRng)
    }

    fn payload(n: usize) -> Vec<u8> {
        // 位置で中身が変わるようにして、ずれを検出できるようにする。
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn signed(name: &str, data: &[u8], k: &SigningKey) -> SignedManifest {
        let m = Manifest::build(name, data, CHUNK).unwrap();
        let sig = k.sign(&m.to_canonical_bytes());
        SignedManifest { manifest: m, signature: sig.to_bytes() }
    }

    #[test]
    fn 正しいイメージは丸ごと書ける() {
        let k = key();
        let data = payload(1000);
        let sm = signed("ok.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();

        let mut out = Vec::new();
        let st = verify_and_write(m, &mut Cursor::new(&data), &mut out, 0).unwrap();
        assert_eq!(out, data);
        assert_eq!(st.bytes_written, data.len() as u64);
        assert_eq!(st.chunks_written, m.chunks.len());
    }

    #[test]
    fn 壊れたチャンクは書かれずに止まる() {
        // この設計の要。末尾でまとめて検証する方式だと、ここで既に
        // 全部ディスクに載っている。
        let k = key();
        let data = payload(1000);
        let sm = signed("bad.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();

        // 3 番目のチャンクの真ん中を 1 バイト書き換える。
        let bad_index = 3usize;
        let mut tampered = data.clone();
        tampered[bad_index * CHUNK as usize + 10] ^= 0xFF;

        let mut out = Vec::new();
        let err = verify_and_write(m, &mut Cursor::new(&tampered), &mut out, 0).unwrap_err();
        assert!(err.to_string().contains("一致しない"), "{err}");

        // 書かれたのは検証を通った前半だけで、壊れたチャンクは 1 バイトも
        // 出ていないこと。
        assert_eq!(out.len(), bad_index * CHUNK as usize,
                   "壊れたチャンクのバイトが書き出されている");
        assert_eq!(out, &data[..bad_index * CHUNK as usize]);
    }

    #[test]
    fn 先頭のチャンクが壊れていれば一切書かれない() {
        let k = key();
        let data = payload(1000);
        let sm = signed("bad0.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();

        let mut tampered = data.clone();
        tampered[0] ^= 0xFF;

        let mut out = Vec::new();
        assert!(verify_and_write(m, &mut Cursor::new(&tampered), &mut out, 0).is_err());
        assert!(out.is_empty(), "1 バイトも書いてはいけない");
    }

    #[test]
    fn 途中から再開できる() {
        // 数百 MiB を悪い回線で落とすので、中断のたびに最初からやり直すのは
        // 現実的でない。
        let k = key();
        let data = payload(1000);
        let sm = signed("resume.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();

        let from = 5usize;
        let offset = from * CHUNK as usize;
        let mut out = Vec::new();
        let st = verify_and_write(m, &mut Cursor::new(&data[offset..]), &mut out, from).unwrap();

        assert_eq!(out, &data[offset..]);
        assert_eq!(st.chunks_skipped, from);
        assert_eq!(st.chunks_written, m.chunks.len() - from);
    }

    #[test]
    fn 別の鍵で署名されたマニフェストは通らない() {
        let data = payload(300);
        let sm = signed("x.img", &data, &key());
        assert!(sm.verify(&key().verifying_key()).is_err());
    }

    #[test]
    fn マニフェストを書き換えると通らない() {
        let k = key();
        let data = payload(300);
        let mut sm = signed("x.img", &data, &k);
        // 差し替えたいイメージのハッシュを 1 つ埋め込む、という攻撃。
        sm.manifest.chunks[0] = [0xAA; 32];
        assert!(sm.verify(&k.verifying_key()).is_err());
    }

    #[test]
    fn 自己矛盾したマニフェストは署名が通っても拒む() {
        // 署名鍵が正しくても、内容が壊れていれば書けない。
        let k = key();
        let data = payload(300);
        let mut m = Manifest::build("x.img", &data, CHUNK).unwrap();
        m.chunks.pop();
        let sig = k.sign(&m.to_canonical_bytes());
        let sm = SignedManifest { manifest: m, signature: sig.to_bytes() };
        let err = sm.verify(&k.verifying_key()).unwrap_err();
        assert!(err.to_string().contains("チャンク数"), "{err}");
    }

    #[test]
    fn 入力が足りなければ止まる() {
        let k = key();
        let data = payload(1000);
        let sm = signed("short.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();

        let mut out = Vec::new();
        let err = verify_and_write(m, &mut Cursor::new(&data[..500]), &mut out, 0).unwrap_err();
        assert!(err.to_string().contains("読めない"), "{err}");
    }

    #[test]
    fn 再開位置が範囲外なら断る() {
        let k = key();
        let data = payload(300);
        let sm = signed("x.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();
        let mut out = Vec::new();
        assert!(verify_and_write(m, &mut Cursor::new(&data), &mut out, 9999).is_err());
    }

    #[test]
    fn 署名付きマニフェストが往復する() {
        let k = key();
        let sm = signed("rt.img", &payload(300), &k);
        let back = SignedManifest::decode(&sm.encode()).unwrap();
        assert_eq!(back.manifest, sm.manifest);
        assert_eq!(back.signature, sm.signature);
        assert!(back.verify(&k.verifying_key()).is_ok());
    }

    #[test]
    fn 署名を一ビット変えると通らない() {
        let k = key();
        let sm = signed("rt.img", &payload(300), &k);
        let mut enc = sm.encode();
        let last = enc.len() - 1;
        enc[last] ^= 1;
        let back = SignedManifest::decode(&enc).unwrap();
        assert!(back.verify(&k.verifying_key()).is_err());
    }

    #[test]
    fn 短すぎる入力を弾く() {
        assert!(SignedManifest::decode(&[0u8; 32]).is_err());
    }

    #[test]
    fn 末尾のチャンクが短くても通る() {
        let k = key();
        // チャンク長の倍数でない大きさ。
        let data = payload(CHUNK as usize * 3 + 7);
        let sm = signed("tail.img", &data, &k);
        let m = sm.verify(&k.verifying_key()).unwrap();
        assert_eq!(m.chunk_len(3), 7);

        let mut out = Vec::new();
        verify_and_write(m, &mut Cursor::new(&data), &mut out, 0).unwrap();
        assert_eq!(out, data);
    }
}
