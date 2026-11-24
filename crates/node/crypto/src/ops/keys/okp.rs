//! Ed25519 and X25519 key material in the RFC 8410 encodings Node emits.
//!
//! The curve crates carry the arithmetic; the PKCS#8/SPKI wrapper is assembled here because
//! X25519 has no PKCS#8 implementation in `x25519-dalek`, and the Ed25519 crate writes the
//! public key into the PKCS#8 document while Node writes the seed only.

use deno_error::JsErrorBox;
use der::asn1::{BitString, ObjectIdentifier, OctetString, OctetStringRef};
use der::{Decode as _, Encode as _};
use ed25519_dalek::{SigningKey, VerifyingKey};
use pkcs8::PrivateKeyInfo;
use spki::{AlgorithmIdentifierRef, SubjectPublicKeyInfoOwned};
use x25519_dalek::{PublicKey, StaticSecret};

use super::invalid_key;

const ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
const X25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.110");

pub(super) fn generate_ed25519() -> SigningKey {
    SigningKey::generate(&mut rand_core::OsRng)
}

pub(super) fn generate_x25519() -> StaticSecret {
    StaticSecret::random_from_rng(rand_core::OsRng)
}

pub(super) fn ed25519_private_der(key: &SigningKey) -> Result<Vec<u8>, JsErrorBox> {
    rfc8410_private(ED25519, &key.to_bytes())
}

pub(super) fn ed25519_public_der(key: &VerifyingKey) -> Result<Vec<u8>, JsErrorBox> {
    rfc8410_public(ED25519, key.as_bytes())
}

pub(super) fn ed25519_parse_private(der: &[u8]) -> Result<Option<SigningKey>, JsErrorBox> {
    let Some(seed) = rfc8410_private_bytes(der, ED25519)? else {
        return Ok(None);
    };
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| invalid_key("Ed25519 private key has the wrong length"))?;
    Ok(Some(SigningKey::from_bytes(&seed)))
}

pub(super) fn ed25519_parse_public(der: &[u8]) -> Result<Option<VerifyingKey>, JsErrorBox> {
    let Some(bytes) = rfc8410_public_bytes(der, ED25519) else {
        return Ok(None);
    };
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_key("Ed25519 public key has the wrong length"))?;
    VerifyingKey::from_bytes(&bytes)
        .map(Some)
        .map_err(invalid_key)
}

pub(super) fn x25519_private_der(key: &StaticSecret) -> Result<Vec<u8>, JsErrorBox> {
    rfc8410_private(X25519, &key.to_bytes())
}

pub(super) fn x25519_public_der(key: &PublicKey) -> Result<Vec<u8>, JsErrorBox> {
    rfc8410_public(X25519, key.as_bytes())
}

pub(super) fn x25519_parse_private(der: &[u8]) -> Result<Option<StaticSecret>, JsErrorBox> {
    let Some(bytes) = rfc8410_private_bytes(der, X25519)? else {
        return Ok(None);
    };
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_key("X25519 private key has the wrong length"))?;
    Ok(Some(StaticSecret::from(bytes)))
}

pub(super) fn x25519_parse_public(der: &[u8]) -> Result<Option<PublicKey>, JsErrorBox> {
    let Some(bytes) = rfc8410_public_bytes(der, X25519) else {
        return Ok(None);
    };
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_key("X25519 public key has the wrong length"))?;
    Ok(Some(PublicKey::from(bytes)))
}

fn rfc8410_private(oid: ObjectIdentifier, raw: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    let inner = OctetString::new(raw)
        .and_then(|inner| inner.to_der())
        .map_err(invalid_key)?;
    PrivateKeyInfo::new(
        AlgorithmIdentifierRef {
            oid,
            parameters: None,
        },
        &inner,
    )
    .to_der()
    .map_err(invalid_key)
}

fn rfc8410_public(oid: ObjectIdentifier, raw: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    let info: SubjectPublicKeyInfoOwned = spki::SubjectPublicKeyInfo {
        algorithm: spki::AlgorithmIdentifier {
            oid,
            parameters: None,
        },
        subject_public_key: BitString::from_bytes(raw).map_err(invalid_key)?,
    };
    info.to_der().map_err(invalid_key)
}

fn rfc8410_private_bytes(der: &[u8], oid: ObjectIdentifier) -> Result<Option<Vec<u8>>, JsErrorBox> {
    let Ok(info) = PrivateKeyInfo::try_from(der) else {
        return Ok(None);
    };
    if info.algorithm.oid != oid || info.algorithm.parameters.is_some() {
        return Ok(None);
    }
    let inner = OctetStringRef::from_der(info.private_key).map_err(invalid_key)?;
    Ok(Some(inner.as_bytes().to_vec()))
}

fn rfc8410_public_bytes(der: &[u8], oid: ObjectIdentifier) -> Option<Vec<u8>> {
    let info = spki::SubjectPublicKeyInfoRef::try_from(der).ok()?;
    if info.algorithm.oid != oid {
        return None;
    }
    Some(info.subject_public_key.raw_bytes().to_vec())
}
