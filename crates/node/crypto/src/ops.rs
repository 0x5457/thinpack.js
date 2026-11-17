use aes::cipher::block_padding::{NoPadding, Pkcs7};
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, StreamCipher};
use aes_gcm::aead::consts::U12;
use aes_gcm::aead::{AeadInPlace, generic_array::GenericArray};
use aes_gcm::{AesGcm, KeyInit as GcmKeyInit};
use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;
use md5::{Digest as _, Md5};
use ring::digest;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

pub(crate) const OPS: &[OpDecl] = &[
    op_crypto_random(),
    op_crypto_hash(),
    op_crypto_hmac(),
    op_crypto_timing_safe_equal(),
    op_crypto_verify(),
    op_crypto_sign(),
    op_crypto_cipher(),
    op_crypto_pbkdf2(),
    op_crypto_scrypt(),
    op_crypto_hkdf(),
];

fn crypto_error(message: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("ERR_CRYPTO_INVALID_DIGEST\0{message}"))
}

fn algorithm(name: &str) -> Option<&'static digest::Algorithm> {
    match name.to_ascii_lowercase().as_str() {
        "sha1" | "sha-1" => Some(&digest::SHA1_FOR_LEGACY_USE_ONLY),
        "sha256" | "sha-256" => Some(&digest::SHA256),
        "sha384" | "sha-384" => Some(&digest::SHA384),
        "sha512" | "sha-512" => Some(&digest::SHA512),
        _ => None,
    }
}

fn hmac_algorithm(name: &str) -> Option<hmac::Algorithm> {
    match name.to_ascii_lowercase().as_str() {
        "sha1" | "sha-1" => Some(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY),
        "sha256" | "sha-256" => Some(hmac::HMAC_SHA256),
        "sha384" | "sha-384" => Some(hmac::HMAC_SHA384),
        "sha512" | "sha-512" => Some(hmac::HMAC_SHA512),
        _ => None,
    }
}

#[op2]
#[buffer]
fn op_crypto_random(#[smi] length: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let mut buffer = vec![0u8; length as usize];
    SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| JsErrorBox::new("Error", "ERR_CRYPTO_RANDOM\0system random failed"))?;
    Ok(buffer.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_crypto_hash(#[string] name: &str, #[buffer] data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    if name.eq_ignore_ascii_case("md5") {
        return Ok(Md5::digest(data).to_vec().into_boxed_slice());
    }
    let algorithm = algorithm(name).ok_or_else(|| crypto_error(name))?;
    let digest = digest::digest(algorithm, data);
    Ok(digest.as_ref().to_vec().into_boxed_slice())
}

#[op2]
#[buffer]
fn op_crypto_hmac(
    #[string] name: &str,
    #[buffer] key: &[u8],
    #[buffer] data: &[u8],
) -> Result<Box<[u8]>, JsErrorBox> {
    if name.eq_ignore_ascii_case("md5") {
        return Ok(hmac_md5(key, data).into_boxed_slice());
    }
    let algorithm = hmac_algorithm(name).ok_or_else(|| crypto_error(name))?;
    let key = hmac::Key::new(algorithm, key);
    let tag = hmac::sign(&key, data);
    Ok(tag.as_ref().to_vec().into_boxed_slice())
}

#[op2]
#[buffer]
fn op_crypto_sign(
    #[string] algorithm: &str,
    #[buffer] key: &[u8],
    #[buffer] data: &[u8],
) -> Result<Box<[u8]>, JsErrorBox> {
    use ring::signature;

    let rng = ring::rand::SystemRandom::new();
    let invalid = |error: ring::error::KeyRejected| {
        JsErrorBox::new("Error", format!("ERR_CRYPTO_INVALID_KEY\0{error}"))
    };
    match algorithm {
        "rsa-sha256" | "rsa-sha384" | "rsa-sha512" => {
            let pair = signature::RsaKeyPair::from_pkcs8(key).map_err(invalid)?;
            let padding: &'static dyn signature::RsaEncoding = match algorithm {
                "rsa-sha384" => &signature::RSA_PKCS1_SHA384,
                "rsa-sha512" => &signature::RSA_PKCS1_SHA512,
                _ => &signature::RSA_PKCS1_SHA256,
            };
            let mut signature_bytes = vec![0u8; pair.public().modulus_len()];
            pair.sign(padding, &rng, data, &mut signature_bytes)
                .map_err(|error| JsErrorBox::new("Error", format!("ERR_CRYPTO_SIGN\0{error}")))?;
            Ok(signature_bytes.into_boxed_slice())
        }
        "ecdsa-p256-sha256" | "ecdsa-p384-sha384" => {
            let signing = if algorithm == "ecdsa-p384-sha384" {
                &signature::ECDSA_P384_SHA384_FIXED_SIGNING
            } else {
                &signature::ECDSA_P256_SHA256_FIXED_SIGNING
            };
            let pair = signature::EcdsaKeyPair::from_pkcs8(signing, key, &rng).map_err(invalid)?;
            let signature_bytes = pair
                .sign(&rng, data)
                .map_err(|error| JsErrorBox::new("Error", format!("ERR_CRYPTO_SIGN\0{error}")))?;
            Ok(signature_bytes.as_ref().to_vec().into_boxed_slice())
        }
        other => Err(JsErrorBox::new(
            "Error",
            format!("ERR_CRYPTO_SIGN\0unsupported algorithm {other}"),
        )),
    }
}

#[op2(fast)]
fn op_crypto_verify(
    #[string] algorithm: &str,
    #[buffer] key: &[u8],
    #[buffer] signature: &[u8],
    #[buffer] data: &[u8],
) -> Result<bool, JsErrorBox> {
    use ring::signature;
    let algorithm: &'static dyn signature::VerificationAlgorithm = match algorithm {
        "rsa-sha256" => &signature::RSA_PKCS1_2048_8192_SHA256,
        "rsa-sha384" => &signature::RSA_PKCS1_2048_8192_SHA384,
        "rsa-sha512" => &signature::RSA_PKCS1_2048_8192_SHA512,
        "ecdsa-p256-sha256" => &signature::ECDSA_P256_SHA256_FIXED,
        "ecdsa-p384-sha384" => &signature::ECDSA_P384_SHA384_FIXED,
        "rsa-pss-sha256" => &signature::RSA_PSS_2048_8192_SHA256,
        "rsa-pss-sha384" => &signature::RSA_PSS_2048_8192_SHA384,
        "rsa-pss-sha512" => &signature::RSA_PSS_2048_8192_SHA512,
        other => {
            return Err(JsErrorBox::new(
                "Error",
                format!("ERR_CRYPTO_VERIFY\0unsupported algorithm {other}"),
            ));
        }
    };
    Ok(signature::UnparsedPublicKey::new(algorithm, key)
        .verify(data, signature)
        .is_ok())
}

#[op2(fast)]
fn op_crypto_timing_safe_equal(#[buffer] left: &[u8], #[buffer] right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

fn hmac_md5(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..16].copy_from_slice(&Md5::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut inner = Md5::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(data);
    let inner = inner.finalize();

    let mut outer = Md5::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner);
    outer.finalize().to_vec()
}

fn cipher_error(code: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0invalid cipher parameters"))
}

#[op2]
#[buffer]
fn op_crypto_cipher(
    #[string] algorithm: &str,
    #[buffer] key: &[u8],
    #[buffer] iv: &[u8],
    #[buffer] aad: &[u8],
    #[buffer] data: &[u8],
    #[smi] encrypt: u32,
    #[smi] padding: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let name = algorithm.to_ascii_lowercase();
    let rest = name
        .strip_prefix("aes-")
        .ok_or_else(|| cipher_error("ERR_CRYPTO_UNKNOWN_CIPHER"))?;
    let (bits, mode) = rest
        .split_once('-')
        .ok_or_else(|| cipher_error("ERR_CRYPTO_UNKNOWN_CIPHER"))?;
    let bits: usize = bits
        .parse()
        .map_err(|_| cipher_error("ERR_CRYPTO_UNKNOWN_CIPHER"))?;
    if key.len() != bits / 8 {
        return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN"));
    }

    match mode {
        "gcm" => {
            if iv.len() != 12 {
                return Err(cipher_error("ERR_CRYPTO_INVALID_IVLEN"));
            }
            if encrypt != 0 {
                gcm_encrypt(key, iv, aad, data)
            } else {
                gcm_decrypt(key, iv, aad, data)
            }
        }
        "cbc" => {
            if iv.len() != 16 {
                return Err(cipher_error("ERR_CRYPTO_INVALID_IVLEN"));
            }
            if encrypt != 0 {
                cbc_encrypt(key, iv, data, padding != 0)
            } else {
                cbc_decrypt(key, iv, data, padding != 0)
            }
        }
        "ctr" => {
            if iv.len() != 16 {
                return Err(cipher_error("ERR_CRYPTO_INVALID_IVLEN"));
            }
            ctr_apply(key, iv, data)
        }
        _ => Err(cipher_error("ERR_CRYPTO_UNKNOWN_CIPHER")),
    }
}

fn gcm_encrypt(key: &[u8], iv: &[u8], aad: &[u8], data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    macro_rules! run {
        ($cipher:ty) => {{
            let cipher = <$cipher>::new_from_slice(key)
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_KEYLEN"))?;
            let mut output = data.to_vec();
            let tag = cipher
                .encrypt_in_place_detached(GenericArray::from_slice(iv), aad, &mut output)
                .map_err(|_| cipher_error("ERR_CRYPTO_BAD_DECRYPT"))?;
            output.extend_from_slice(tag.as_slice());
            output.into_boxed_slice()
        }};
    }
    let output = match key.len() {
        16 => run!(aes_gcm::Aes128Gcm),
        24 => run!(AesGcm::<aes::Aes192, U12>),
        32 => run!(aes_gcm::Aes256Gcm),
        _ => return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN")),
    };
    Ok(output)
}

fn gcm_decrypt(key: &[u8], iv: &[u8], aad: &[u8], data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    if data.len() < 16 {
        return Err(cipher_error("ERR_CRYPTO_INVALID_AUTH_TAG"));
    }
    let (ciphertext, tag) = data.split_at(data.len() - 16);
    macro_rules! run {
        ($cipher:ty) => {{
            let cipher = <$cipher>::new_from_slice(key)
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_KEYLEN"))?;
            let mut output = ciphertext.to_vec();
            cipher
                .decrypt_in_place_detached(
                    GenericArray::from_slice(iv),
                    aad,
                    &mut output,
                    GenericArray::from_slice(tag),
                )
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_AUTH_TAG"))?;
            output.into_boxed_slice()
        }};
    }
    let output = match key.len() {
        16 => run!(aes_gcm::Aes128Gcm),
        24 => run!(AesGcm::<aes::Aes192, U12>),
        32 => run!(aes_gcm::Aes256Gcm),
        _ => return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN")),
    };
    Ok(output)
}

fn cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8], padding: bool) -> Result<Box<[u8]>, JsErrorBox> {
    macro_rules! run {
        ($cipher:ty) => {{
            let cipher = cbc::Encryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_IVLEN"))?;
            if padding {
                cipher.encrypt_padded_vec_mut::<Pkcs7>(data)
            } else {
                cipher.encrypt_padded_vec_mut::<NoPadding>(data)
            }
        }};
    }
    let output = match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        _ => return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN")),
    };
    Ok(output.into_boxed_slice())
}

fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8], padding: bool) -> Result<Box<[u8]>, JsErrorBox> {
    macro_rules! run {
        ($cipher:ty) => {{
            let cipher = cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_IVLEN"))?;
            let result = if padding {
                cipher.decrypt_padded_vec_mut::<Pkcs7>(data)
            } else {
                cipher.decrypt_padded_vec_mut::<NoPadding>(data)
            };
            result.map_err(|_| cipher_error("ERR_CRYPTO_BAD_DECRYPT"))?
        }};
    }
    let output = match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        _ => return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN")),
    };
    Ok(output.into_boxed_slice())
}

fn ctr_apply(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    macro_rules! run {
        ($cipher:ty) => {{
            let mut cipher = ctr::Ctr128BE::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_IVLEN"))?;
            let mut output = data.to_vec();
            cipher.apply_keystream(&mut output);
            output
        }};
    }
    let output = match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        _ => return Err(cipher_error("ERR_CRYPTO_INVALID_KEYLEN")),
    };
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_crypto_pbkdf2(
    #[string] digest_name: &str,
    #[buffer] password: &[u8],
    #[buffer] salt: &[u8],
    #[smi] iterations: u32,
    #[smi] keylen: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    use std::num::NonZeroU32;

    let algorithm = match digest_name.to_ascii_lowercase().as_str() {
        "sha1" | "sha-1" => ring::pbkdf2::PBKDF2_HMAC_SHA1,
        "sha256" | "sha-256" => ring::pbkdf2::PBKDF2_HMAC_SHA256,
        "sha384" | "sha-384" => ring::pbkdf2::PBKDF2_HMAC_SHA384,
        "sha512" | "sha-512" => ring::pbkdf2::PBKDF2_HMAC_SHA512,
        _ => return Err(crypto_error(digest_name)),
    };
    let iterations = NonZeroU32::new(iterations).ok_or_else(|| cipher_error("ERR_OUT_OF_RANGE"))?;
    let mut output = vec![0u8; keylen as usize];
    ring::pbkdf2::derive(algorithm, iterations, salt, password, &mut output);
    Ok(output.into_boxed_slice())
}

#[op2]
#[buffer]
fn op_crypto_scrypt(
    #[buffer] password: &[u8],
    #[buffer] salt: &[u8],
    #[smi] log_n: u32,
    #[smi] r: u32,
    #[smi] p: u32,
    #[smi] keylen: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let log_n =
        u8::try_from(log_n).map_err(|_| cipher_error("ERR_CRYPTO_INVALID_SCRYPT_PARAMS"))?;
    let params = scrypt::Params::new(log_n, r, p, keylen as usize)
        .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_SCRYPT_PARAMS"))?;
    let mut output = vec![0u8; keylen as usize];
    scrypt::scrypt(password, salt, &params, &mut output)
        .map_err(|_| cipher_error("ERR_CRYPTO_INVALID_SCRYPT_PARAMS"))?;
    Ok(output.into_boxed_slice())
}

struct HkdfLength(usize);

impl ring::hkdf::KeyType for HkdfLength {
    fn len(&self) -> usize {
        self.0
    }
}

#[op2]
#[buffer]
fn op_crypto_hkdf(
    #[string] digest_name: &str,
    #[buffer] ikm: &[u8],
    #[buffer] salt: &[u8],
    #[buffer] info: &[u8],
    #[smi] keylen: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let algorithm = match digest_name.to_ascii_lowercase().as_str() {
        "sha1" | "sha-1" => ring::hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY,
        "sha256" | "sha-256" => ring::hkdf::HKDF_SHA256,
        "sha384" | "sha-384" => ring::hkdf::HKDF_SHA384,
        "sha512" | "sha-512" => ring::hkdf::HKDF_SHA512,
        _ => return Err(crypto_error(digest_name)),
    };
    let salt = ring::hkdf::Salt::new(algorithm, salt);
    let prk = salt.extract(ikm);
    let mut output = vec![0u8; keylen as usize];
    prk.expand(&[info], HkdfLength(keylen as usize))
        .and_then(|okm| okm.fill(&mut output))
        .map_err(|_| crypto_error("hkdf"))?;
    Ok(output.into_boxed_slice())
}
