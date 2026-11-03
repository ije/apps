use crate::web::{base64, bytes};
use anyhow::{Context, Result, bail, ensure};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
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
    match op {
        "generate" if args[0] == "ed25519" => {
            let mut seed = [0; 32];
            if args[1].is_null() {
                rng.fill(&mut seed)
                    .map_err(|_| anyhow::anyhow!("Key generation failed"))?;
            } else {
                seed = bytes(&args[1])?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Invalid Ed25519 key"))?;
            }
            let key = signature::Ed25519KeyPair::from_seed_unchecked(&seed)
                .map_err(|_| anyhow::anyhow!("Invalid Ed25519 key"))?;
            Ok(json!({"private":base64(&seed),"public":base64(key.public_key().as_ref())}))
        }
        "generate" => {
            let signing = if args[0] == "P-384" {
                &signature::ECDSA_P384_SHA384_FIXED_SIGNING
            } else {
                &signature::ECDSA_P256_SHA256_FIXED_SIGNING
            };
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
        // `ECDSA` signatures are IEEE P1363 (r || s) and `ec` ones are DER. RSA keys are PKCS#1 DER.
        "sign" | "verify" => {
            let algorithm = args[0].as_str().context("Expected algorithm")?;
            let secret = bytes(&args[1])?;
            let public = bytes(&args[2])?;
            let data = bytes(&args[3])?;
            let signature = bytes(&args[4])?;
            let hash = args[5].as_str().unwrap_or("SHA-256");
            let p384 = public.len() == 97;
            if op == "verify" && algorithm != "HMAC" {
                let verification: &'static dyn signature::VerificationAlgorithm =
                    match (algorithm, hash, p384) {
                        ("ECDSA", "SHA-256", false) => &signature::ECDSA_P256_SHA256_FIXED,
                        ("ECDSA", "SHA-384", true) => &signature::ECDSA_P384_SHA384_FIXED,
                        ("ec", "SHA-256", false) => &signature::ECDSA_P256_SHA256_ASN1,
                        ("ec", "SHA-384", false) => &signature::ECDSA_P256_SHA384_ASN1,
                        ("ec", "SHA-256", true) => &signature::ECDSA_P384_SHA256_ASN1,
                        ("ec", "SHA-384", true) => &signature::ECDSA_P384_SHA384_ASN1,
                        ("ed25519", ..) => &signature::ED25519,
                        ("rsa", "SHA-1", _) => {
                            &signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY
                        }
                        ("rsa", "SHA-256", _) => {
                            &signature::RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY
                        }
                        ("rsa", "SHA-384", _) => &signature::RSA_PKCS1_2048_8192_SHA384,
                        ("rsa", "SHA-512", _) => {
                            &signature::RSA_PKCS1_1024_8192_SHA512_FOR_LEGACY_USE_ONLY
                        }
                        ("rsa-pss", "SHA-256", _) => &signature::RSA_PSS_2048_8192_SHA256,
                        ("rsa-pss", "SHA-384", _) => &signature::RSA_PSS_2048_8192_SHA384,
                        ("rsa-pss", "SHA-512", _) => &signature::RSA_PSS_2048_8192_SHA512,
                        _ => bail!("Unsupported signature algorithm: {algorithm} with {hash}"),
                    };
                return Ok(json!(
                    signature::UnparsedPublicKey::new(verification, public)
                        .verify(&data, &signature)
                        .is_ok()
                ));
            }
            let failed = |_| anyhow::anyhow!("Signing failed");
            match algorithm {
                "ECDSA" | "ec" => {
                    let signing = match (algorithm, hash, p384) {
                        ("ECDSA", "SHA-256", false) => &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                        ("ECDSA", "SHA-384", true) => &signature::ECDSA_P384_SHA384_FIXED_SIGNING,
                        ("ec", "SHA-256", false) => &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                        ("ec", "SHA-384", true) => &signature::ECDSA_P384_SHA384_ASN1_SIGNING,
                        _ => {
                            bail!("ECDSA signs P-256 keys with SHA-256 and P-384 keys with SHA-384")
                        }
                    };
                    let key = signature::EcdsaKeyPair::from_private_key_and_public_key(
                        signing, &secret, &public, &rng,
                    )
                    .map_err(|_| anyhow::anyhow!("Invalid ECDSA key"))?;
                    return Ok(base64(key.sign(&rng, &data).map_err(failed)?.as_ref()));
                }
                "ed25519" => {
                    let key = signature::Ed25519KeyPair::from_seed_and_public_key(&secret, &public)
                        .map_err(|_| anyhow::anyhow!("Invalid Ed25519 key"))?;
                    return Ok(base64(key.sign(&data).as_ref()));
                }
                "rsa" | "rsa-pss" => {
                    let padding: &'static dyn signature::RsaEncoding = match (algorithm, hash) {
                        ("rsa", "SHA-256") => &signature::RSA_PKCS1_SHA256,
                        ("rsa", "SHA-384") => &signature::RSA_PKCS1_SHA384,
                        ("rsa", "SHA-512") => &signature::RSA_PKCS1_SHA512,
                        ("rsa-pss", "SHA-256") => &signature::RSA_PSS_SHA256,
                        ("rsa-pss", "SHA-384") => &signature::RSA_PSS_SHA384,
                        ("rsa-pss", "SHA-512") => &signature::RSA_PSS_SHA512,
                        _ => bail!("RSA signs with SHA-256, SHA-384 or SHA-512"),
                    };
                    let key = signature::RsaKeyPair::from_der(&secret)
                        .map_err(|error| anyhow::anyhow!("Invalid RSA key: {error}"))?;
                    let mut output = vec![0; key.public().modulus_len()];
                    key.sign(padding, &rng, &data, &mut output)
                        .map_err(failed)?;
                    return Ok(base64(&output));
                }
                _ => {}
            }
            ensure!(algorithm == "HMAC", "Unsupported signature algorithm");
            let hash = match hash {
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
