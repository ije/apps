use crate::web::{base64, bytes};
use anyhow::{Context, Result, bail, ensure};
use ring::{
    aead, hmac,
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use serde_json::{Value, json};

fn der<'a>(input: &mut &'a [u8], tag: u8) -> Result<&'a [u8]> {
    ensure!(input.len() >= 2 && input[0] == tag, "Invalid DER key");
    let mut length = usize::from(input[1]);
    let mut offset = 2;
    if length & 128 != 0 {
        let count = length & 127;
        ensure!(
            count > 0 && count <= 4 && input.len() >= 2 + count,
            "Invalid DER length"
        );
        length = 0;
        for &byte in &input[2..2 + count] {
            length = (length << 8) | usize::from(byte);
        }
        offset += count;
    }
    ensure!(input.len() >= offset + length, "Truncated DER key");
    let value = &input[offset..offset + length];
    *input = &input[offset + length..];
    Ok(value)
}

pub(crate) fn call(op: &str, args: &Value) -> Result<Value> {
    let rng = SystemRandom::new();
    let signing = &signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    match op {
        "generate" => {
            let document = signature::EcdsaKeyPair::generate_pkcs8(signing, &rng)
                .map_err(|_| anyhow::anyhow!("Key generation failed"))?;
            let key = signature::EcdsaKeyPair::from_pkcs8(signing, document.as_ref(), &rng)
                .map_err(|_| anyhow::anyhow!("Invalid ECDSA key"))?;
            let mut outer = der(&mut document.as_ref(), 0x30)?;
            der(&mut outer, 2)?;
            der(&mut outer, 0x30)?;
            let mut octets = der(&mut outer, 4)?;
            let mut inner = der(&mut octets, 0x30)?;
            der(&mut inner, 2)?;
            let private = der(&mut inner, 4)?;
            Ok(
                json!({"private":base64(private),"public":base64(key.public_key().as_ref()),"pkcs8":base64(document.as_ref())}),
            )
        }
        "sign" | "verify" => {
            let algorithm = args[0].as_str().context("Expected algorithm")?;
            let secret = bytes(&args[1])?;
            let public = bytes(&args[2])?;
            let data = bytes(&args[3])?;
            let signature = bytes(&args[4])?;
            if algorithm == "ECDSA" {
                if op == "verify" {
                    return Ok(json!(
                        signature::UnparsedPublicKey::new(
                            &signature::ECDSA_P256_SHA256_FIXED,
                            public
                        )
                        .verify(&data, &signature)
                        .is_ok()
                    ));
                }
                let key = signature::EcdsaKeyPair::from_private_key_and_public_key(
                    signing, &secret, &public, &rng,
                )
                .map_err(|_| anyhow::anyhow!("Invalid ECDSA key"))?;
                return Ok(base64(
                    key.sign(&rng, &data)
                        .map_err(|_| anyhow::anyhow!("ECDSA signing failed"))?
                        .as_ref(),
                ));
            }
            ensure!(algorithm == "HMAC", "Unsupported signature algorithm");
            let hash = match args[5].as_str().unwrap_or("SHA-256") {
                "SHA-1" => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
                "SHA-256" => hmac::HMAC_SHA256,
                "SHA-384" => hmac::HMAC_SHA384,
                "SHA-512" => hmac::HMAC_SHA512,
                _ => bail!("Unsupported HMAC digest"),
            };
            let key = hmac::Key::new(hash, &secret);
            if op == "verify" {
                Ok(json!(hmac::verify(&key, &data, &signature).is_ok()))
            } else {
                Ok(base64(hmac::sign(&key, &data).as_ref()))
            }
        }
        "encrypt" | "decrypt" => {
            let key = bytes(&args[0])?;
            let iv = bytes(&args[1])?;
            let aad = bytes(&args[2])?;
            let mut data = bytes(&args[3])?;
            let algorithm = match key.len() {
                16 => &aead::AES_128_GCM,
                32 => &aead::AES_256_GCM,
                _ => bail!("AES-GCM requires a 128 or 256 bit key"),
            };
            let key = aead::LessSafeKey::new(
                aead::UnboundKey::new(algorithm, &key)
                    .map_err(|_| anyhow::anyhow!("Invalid AES key"))?,
            );
            let nonce = aead::Nonce::try_assume_unique_for_key(&iv)
                .map_err(|_| anyhow::anyhow!("AES-GCM requires a 12 byte IV"))?;
            let aad = aead::Aad::from(aad);
            if op == "encrypt" {
                key.seal_in_place_append_tag(nonce, aad, &mut data)
                    .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
                Ok(base64(&data))
            } else {
                let value = key
                    .open_in_place(nonce, aad, &mut data)
                    .map_err(|_| anyhow::anyhow!("Authentication failed"))?;
                Ok(base64(value))
            }
        }
        "equal" => {
            let a = bytes(&args[0])?;
            let b = bytes(&args[1])?;
            ensure!(
                a.len() == b.len(),
                "Input buffers must have the same length"
            );
            #[allow(deprecated)]
            let equal = ring::constant_time::verify_slices_are_equal(&a, &b).is_ok();
            Ok(json!(equal))
        }
        _ => bail!("Unsupported crypto operation: {op}"),
    }
}
