//! RSA key material: PKCS#1 and PKCS#8, SPKI, JWK components and generation.

use std::fmt::Display;

use deno_error::JsErrorBox;
use der::Decode as _;
use der::asn1::ObjectIdentifier;
use pkcs1::{
    DecodeRsaPrivateKey, EncodeRsaPrivateKey, EncodeRsaPublicKey, RsaPublicKey as Pkcs1PublicKey,
};
use pkcs8::EncodePrivateKey as _;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, RsaPrivateKey, RsaPublicKey};
use spki::{EncodePublicKey as _, SubjectPublicKeyInfoRef};

use super::{coded_error, crypto_error, invalid_key};

const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const RSA_PSS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10");

/// The largest modulus the JWK path accepts; Node leaves this to OpenSSL, and a bound keeps
/// a hostile JWK from turning into an unbounded allocation.
const MAX_MODULUS_BITS: usize = 16_384;

pub(super) struct Private {
    pub(super) key: RsaPrivateKey,
    pub(super) pss: bool,
}

pub(super) struct Public {
    pub(super) key: RsaPublicKey,
    pub(super) pss: bool,
}

pub(super) fn generate(bits: u32, exponent: u32, pss: bool) -> Result<Private, JsErrorBox> {
    let bits = usize::try_from(bits)
        .map_err(|_| coded_error("ERR_OUT_OF_RANGE", "invalid modulus length"))?;
    let exponent = BigUint::from(exponent);
    let key = RsaPrivateKey::new_with_exp(&mut rand_core::OsRng, bits, &exponent)
        .map_err(|error| crypto_error("ERR_CRYPTO_KEYGEN_FAILED", &error))?;
    finish(key, pss)
}

/// Parse an unencrypted PKCS#8 document, including the RSASSA-PSS algorithm identifier that
/// OpenSSL emits for an `rsa-pss` key. The stored form is always `rsaEncryption`, because the
/// signing ops hand the DER to ring, which rejects the PSS identifier.
pub(super) fn parse_pkcs8_private(der: &[u8]) -> Result<Option<Private>, JsErrorBox> {
    let Ok(info) = pkcs8::PrivateKeyInfo::try_from(der) else {
        return Ok(None);
    };
    let pss = match info.algorithm.oid {
        RSA_ENCRYPTION => false,
        RSA_PSS => true,
        _ => return Ok(None),
    };
    let key = RsaPrivateKey::from_pkcs1_der(info.private_key).map_err(invalid_key)?;
    finish(key, pss).map(Some)
}

pub(super) fn parse_pkcs1_private(der: &[u8]) -> Option<Private> {
    RsaPrivateKey::from_pkcs1_der(der)
        .ok()
        .and_then(|key| finish(key, false).ok())
}

pub(super) fn parse_spki_public(der: &[u8]) -> Result<Option<Public>, JsErrorBox> {
    let Ok(info) = SubjectPublicKeyInfoRef::try_from(der) else {
        return Ok(None);
    };
    let pss = match info.algorithm.oid {
        RSA_ENCRYPTION => false,
        RSA_PSS => true,
        _ => return Ok(None),
    };
    let key = decode_public_der(info.subject_public_key.raw_bytes())?;
    Ok(Some(Public { key, pss }))
}

pub(super) fn parse_pkcs1_public(der: &[u8]) -> Option<Public> {
    decode_public_der(der)
        .ok()
        .map(|key| Public { key, pss: false })
}

/// Decode the PKCS#1 `RSAPublicKey` by hand: `RsaPublicKey::from_pkcs1_der` goes through
/// `RsaPublicKey::new`, which caps the modulus at 4096 bits, while the private and JWK paths
/// accept larger keys.
fn decode_public_der(der: &[u8]) -> Result<RsaPublicKey, JsErrorBox> {
    let key = Pkcs1PublicKey::from_der(der).map_err(invalid_key)?;
    RsaPublicKey::new_with_max_size(
        BigUint::from_bytes_be(key.modulus.as_bytes()),
        BigUint::from_bytes_be(key.public_exponent.as_bytes()),
        MAX_MODULUS_BITS,
    )
    .map_err(invalid_key)
}

fn finish(mut key: RsaPrivateKey, pss: bool) -> Result<Private, JsErrorBox> {
    key.validate()
        .and_then(|()| key.precompute())
        .map_err(invalid_key)?;
    Ok(Private { key, pss })
}

pub(super) fn pkcs8_der(key: &RsaPrivateKey) -> Result<Vec<u8>, JsErrorBox> {
    key.to_pkcs8_der()
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

pub(super) fn spki_der(key: &RsaPublicKey) -> Result<Vec<u8>, JsErrorBox> {
    key.to_public_key_der()
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

pub(super) fn pkcs1_private_der(key: &RsaPrivateKey) -> Result<Vec<u8>, JsErrorBox> {
    key.to_pkcs1_der()
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

pub(super) fn pkcs1_public_der(key: &RsaPublicKey) -> Result<Vec<u8>, JsErrorBox> {
    key.to_pkcs1_der()
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

pub(super) fn modulus_length(key: &RsaPublicKey) -> Result<u32, JsErrorBox> {
    u32::try_from(key.n().bits())
        .map_err(|_| coded_error("ERR_CRYPTO_INVALID_KEY", "modulus is too large"))
}

pub(super) fn public_numbers(key: &RsaPublicKey) -> (Vec<u8>, Vec<u8>) {
    (key.n().to_bytes_be(), key.e().to_bytes_be())
}

pub(super) struct PrivateNumbers {
    pub(super) n: Vec<u8>,
    pub(super) e: Vec<u8>,
    pub(super) d: Vec<u8>,
    pub(super) p: Vec<u8>,
    pub(super) q: Vec<u8>,
    pub(super) dp: Vec<u8>,
    pub(super) dq: Vec<u8>,
    pub(super) qi: Vec<u8>,
}

pub(super) fn private_numbers(key: &RsaPrivateKey) -> Result<PrivateNumbers, JsErrorBox> {
    let [p, q] = key.primes() else {
        return Err(invalid_jwk("multi-prime RSA keys are not supported"));
    };
    let dp = key
        .dp()
        .ok_or_else(|| invalid_jwk("RSA key is missing its CRT parameters"))?;
    let dq = key
        .dq()
        .ok_or_else(|| invalid_jwk("RSA key is missing its CRT parameters"))?;
    let qi = key
        .qinv()
        .ok_or_else(|| invalid_jwk("RSA key is missing its CRT parameters"))?
        .to_biguint()
        .ok_or_else(|| invalid_jwk("RSA key has a negative CRT coefficient"))?;
    Ok(PrivateNumbers {
        n: key.n().to_bytes_be(),
        e: key.e().to_bytes_be(),
        d: key.d().to_bytes_be(),
        p: p.to_bytes_be(),
        q: q.to_bytes_be(),
        dp: dp.to_bytes_be(),
        dq: dq.to_bytes_be(),
        qi: qi.to_bytes_be(),
    })
}

pub(super) fn from_jwk_private(
    modulus: &[u8],
    exponent: &[u8],
    private_exponent: &[u8],
    prime1: &[u8],
    prime2: &[u8],
) -> Result<Private, JsErrorBox> {
    let modulus = BigUint::from_bytes_be(modulus);
    if modulus.bits() > MAX_MODULUS_BITS {
        return Err(invalid_jwk("RSA modulus is too large"));
    }
    let key = RsaPrivateKey::from_components(
        modulus,
        BigUint::from_bytes_be(exponent),
        BigUint::from_bytes_be(private_exponent),
        vec![
            BigUint::from_bytes_be(prime1),
            BigUint::from_bytes_be(prime2),
        ],
    )
    .map_err(|error| invalid_jwk(&error))?;
    finish(key, false)
}

pub(super) fn from_jwk_public(modulus: &[u8], exponent: &[u8]) -> Result<Public, JsErrorBox> {
    let key = RsaPublicKey::new_with_max_size(
        BigUint::from_bytes_be(modulus),
        BigUint::from_bytes_be(exponent),
        MAX_MODULUS_BITS,
    )
    .map_err(|error| invalid_jwk(&error))?;
    Ok(Public { key, pss: false })
}

fn invalid_jwk(error: impl Display) -> JsErrorBox {
    coded_error("ERR_CRYPTO_INVALID_JWK", &error.to_string())
}
