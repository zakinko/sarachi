//! 発行側の道具。イメージから署名付きマニフェストを作る。
//!
//! **署名鍵はオフラインで保持し、CI に置いてはいけない。** GitHub Releases から
//! 配る以上、GitHub を一つ落とされた時にイメージと署名の両方が手に入る状態を
//! 作ってはならない。信頼の根は GitHub の外に置く。

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signer, SigningKey};
use std::fs;
use unix_mdm_image::{DEFAULT_CHUNK, Manifest, SignedManifest};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<[u8; 32]> {
    let t: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if t.len() != 64 {
        bail!("鍵の形式が違う（16 進 64 文字）");
    }
    let mut raw = [0u8; 32];
    for i in 0..32 {
        raw[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16)?;
    }
    Ok(raw)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let dir = args.get(1).map(String::as_str).unwrap_or(".");
            let sk = format!("{dir}/signing.key");
            let pk = format!("{dir}/signing.pub");
            // 既にあるものを黙って潰すと、配布済みのイメージが全部
            // 検証できなくなる。
            if fs::metadata(&sk).is_ok() {
                bail!("{sk} が既にある。上書きしない");
            }
            let k = SigningKey::generate(&mut rand_core::OsRng);
            fs::write(&sk, hex(k.as_bytes()))?;
            fs::write(&pk, hex(k.verifying_key().as_bytes()))?;
            println!("署名鍵: {sk}  ← オフラインに置く。CI には入れない");
            println!("公開鍵: {pk}  ← 回復パーティションに焼き込む");
        }

        Some("sign") => {
            let (Some(image), Some(keyfile)) = (args.get(1), args.get(2)) else {
                bail!("使い方: sign <image> <signing.key> [out.manifest]");
            };
            let out = args.get(3).cloned().unwrap_or_else(|| format!("{image}.manifest"));
            let k = SigningKey::from_bytes(&unhex(&fs::read_to_string(keyfile)?)?);

            let data = fs::read(image).with_context(|| format!("{image} を読めない"))?;
            let m = Manifest::build(image, &data, DEFAULT_CHUNK)?;
            let sig = k.sign(&m.to_canonical_bytes());
            let sm = SignedManifest { manifest: m, signature: sig.to_bytes() };
            fs::write(&out, sm.encode())?;

            println!("{} に署名した（{} バイト / {} チャンク）",
                     image, data.len(), sm.manifest.chunks.len());
            println!("  → {out}");
        }

        _ => {
            eprintln!("sign keygen [dir]\nsign sign <image> <signing.key> [out.manifest]");
            std::process::exit(2);
        }
    }
    Ok(())
}
