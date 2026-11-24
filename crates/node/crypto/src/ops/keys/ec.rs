//! Elliptic-curve key material over the `RustCrypto` curve crates.
//!
//! Only the key formats live here: signatures stay with ring, which already parses the PKCS#8
//! documents this module emits.

use deno_error::JsErrorBox;
use der::Decode as _;
use der::Encode as _;
use der::asn1::ObjectIdentifier;
use p256::elliptic_curve::sec1::ToEncodedPoint as _;
use pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _};
use spki::{DecodePublicKey as _, EncodePublicKey as _};

use super::{coded_error, invalid_key};

const P256_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const P384_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
const P521_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.35");
const SECP256K1_OID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.10");

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Curve {
    P256,
    P384,
    P521,
    Secp256k1,
}

impl Curve {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "P-256" | "prime256v1" | "secp256r1" => Some(Self::P256),
            "P-384" | "secp384r1" => Some(Self::P384),
            "P-521" | "secp521r1" => Some(Self::P521),
            "secp256k1" => Some(Self::Secp256k1),
            _ => None,
        }
    }

    /// The name Node reports in `asymmetricKeyDetails.namedCurve`; these are OpenSSL's short
    /// names, not the JWK spelling.
    pub(super) const fn key_name(self) -> &'static str {
        match self {
            Self::P256 => "prime256v1",
            Self::P384 => "secp384r1",
            Self::P521 => "secp521r1",
            Self::Secp256k1 => "secp256k1",
        }
    }

    pub(super) fn from_jwk_name(name: &str) -> Option<Self> {
        match name {
            "P-256" => Some(Self::P256),
            "P-384" => Some(Self::P384),
            "P-521" => Some(Self::P521),
            "secp256k1" => Some(Self::Secp256k1),
            _ => None,
        }
    }

    pub(super) const fn jwk_name(self) -> &'static str {
        match self {
            Self::P256 => "P-256",
            Self::P384 => "P-384",
            Self::P521 => "P-521",
            Self::Secp256k1 => "secp256k1",
        }
    }

    const fn oid(self) -> ObjectIdentifier {
        match self {
            Self::P256 => P256_OID,
            Self::P384 => P384_OID,
            Self::P521 => P521_OID,
            Self::Secp256k1 => SECP256K1_OID,
        }
    }

    const fn from_oid(oid: ObjectIdentifier) -> Option<Self> {
        match oid {
            P256_OID => Some(Self::P256),
            P384_OID => Some(Self::P384),
            P521_OID => Some(Self::P521),
            SECP256K1_OID => Some(Self::Secp256k1),
            _ => None,
        }
    }
}

pub(super) enum Private {
    P256(p256::SecretKey),
    P384(p384::SecretKey),
    P521(p521::SecretKey),
    Secp256k1(k256::SecretKey),
}

pub(super) enum Public {
    P256(p256::PublicKey),
    P384(p384::PublicKey),
    P521(p521::PublicKey),
    Secp256k1(k256::PublicKey),
}

pub(super) fn generate(curve: Curve) -> Private {
    let mut rng = rand_core::OsRng;
    match curve {
        Curve::P256 => Private::P256(p256::SecretKey::random(&mut rng)),
        Curve::P384 => Private::P384(p384::SecretKey::random(&mut rng)),
        Curve::P521 => Private::P521(p521::SecretKey::random(&mut rng)),
        Curve::Secp256k1 => Private::Secp256k1(k256::SecretKey::random(&mut rng)),
    }
}

pub(super) fn parse_pkcs8(der: &[u8]) -> Option<Private> {
    p256::SecretKey::from_pkcs8_der(der)
        .ok()
        .map(Private::P256)
        .or_else(|| p384::SecretKey::from_pkcs8_der(der).ok().map(Private::P384))
        .or_else(|| p521::SecretKey::from_pkcs8_der(der).ok().map(Private::P521))
        .or_else(|| {
            k256::SecretKey::from_pkcs8_der(der)
                .ok()
                .map(Private::Secp256k1)
        })
}

pub(super) fn parse_sec1(der: &[u8]) -> Option<Private> {
    let key = sec1::EcPrivateKey::from_der(der).ok()?;
    let curve = match &key.parameters {
        Some(sec1::EcParameters::NamedCurve(oid)) => Curve::from_oid(*oid)?,
        // Legacy keys may omit the curve parameters; the embedded public key still names the
        // curve, and a key without either is rejected rather than guessed.
        _ => return infer_curve(key.private_key, key.public_key),
    };
    private_from_parts(curve, key.private_key, key.public_key).ok()
}

fn infer_curve(private: &[u8], public: Option<&[u8]>) -> Option<Private> {
    let public = public?;
    [Curve::P256, Curve::P384, Curve::P521, Curve::Secp256k1]
        .into_iter()
        .find_map(|curve| private_from_parts(curve, private, Some(public)).ok())
}

fn private_from_parts(
    curve: Curve,
    private: &[u8],
    public: Option<&[u8]>,
) -> Result<Private, JsErrorBox> {
    let key = match curve {
        Curve::P256 => p256::SecretKey::from_slice(private)
            .map(Private::P256)
            .map_err(|error| invalid_key(error.to_string()))?,
        Curve::P384 => p384::SecretKey::from_slice(private)
            .map(Private::P384)
            .map_err(|error| invalid_key(error.to_string()))?,
        Curve::P521 => p521::SecretKey::from_slice(private)
            .map(Private::P521)
            .map_err(|error| invalid_key(error.to_string()))?,
        Curve::Secp256k1 => k256::SecretKey::from_slice(private)
            .map(Private::Secp256k1)
            .map_err(|error| invalid_key(error.to_string()))?,
    };
    if let Some(public) = public
        && public_point(&public_key(&key)) != public
    {
        return Err(invalid_key(
            "SEC1 public key does not match its private key",
        ));
    }
    Ok(key)
}

pub(super) fn parse_spki(der: &[u8]) -> Option<Public> {
    p256::PublicKey::from_public_key_der(der)
        .ok()
        .map(Public::P256)
        .or_else(|| {
            p384::PublicKey::from_public_key_der(der)
                .ok()
                .map(Public::P384)
        })
        .or_else(|| {
            p521::PublicKey::from_public_key_der(der)
                .ok()
                .map(Public::P521)
        })
        .or_else(|| {
            k256::PublicKey::from_public_key_der(der)
                .ok()
                .map(Public::Secp256k1)
        })
}

pub(super) const fn curve(key: &Private) -> Curve {
    match key {
        Private::P256(_) => Curve::P256,
        Private::P384(_) => Curve::P384,
        Private::P521(_) => Curve::P521,
        Private::Secp256k1(_) => Curve::Secp256k1,
    }
}

pub(super) const fn public_curve(key: &Public) -> Curve {
    match key {
        Public::P256(_) => Curve::P256,
        Public::P384(_) => Curve::P384,
        Public::P521(_) => Curve::P521,
        Public::Secp256k1(_) => Curve::Secp256k1,
    }
}

pub(super) fn public_key(key: &Private) -> Public {
    match key {
        Private::P256(key) => Public::P256(key.public_key()),
        Private::P384(key) => Public::P384(key.public_key()),
        Private::P521(key) => Public::P521(key.public_key()),
        Private::Secp256k1(key) => Public::Secp256k1(key.public_key()),
    }
}

pub(super) fn pkcs8_der(key: &Private) -> Result<Vec<u8>, JsErrorBox> {
    let document = match key {
        Private::P256(key) => key.to_pkcs8_der(),
        Private::P384(key) => key.to_pkcs8_der(),
        Private::P521(key) => key.to_pkcs8_der(),
        Private::Secp256k1(key) => key.to_pkcs8_der(),
    }
    .map_err(invalid_key)?;
    Ok(document.as_bytes().to_vec())
}

pub(super) fn sec1_der(key: &Private) -> Result<Vec<u8>, JsErrorBox> {
    // `SecretKey::to_sec1_der` writes no curve parameters; OpenSSL (and Node) need the named
    // curve in the ECPrivateKey, so the wrapper is built here.
    let private = secret_bytes(key);
    let public = public_point(&public_key(key));
    sec1::EcPrivateKey {
        private_key: &private,
        parameters: Some(sec1::EcParameters::NamedCurve(curve(key).oid())),
        public_key: Some(&public),
    }
    .to_der()
    .map_err(invalid_key)
}

pub(super) fn spki_der(key: &Public) -> Result<Vec<u8>, JsErrorBox> {
    let document = match key {
        Public::P256(key) => key.to_public_key_der(),
        Public::P384(key) => key.to_public_key_der(),
        Public::P521(key) => key.to_public_key_der(),
        Public::Secp256k1(key) => key.to_public_key_der(),
    }
    .map_err(invalid_key)?;
    Ok(document.as_bytes().to_vec())
}

pub(super) fn secret_bytes(key: &Private) -> Vec<u8> {
    match key {
        Private::P256(key) => key.to_bytes().to_vec(),
        Private::P384(key) => key.to_bytes().to_vec(),
        Private::P521(key) => key.to_bytes().to_vec(),
        Private::Secp256k1(key) => key.to_bytes().to_vec(),
    }
}

pub(super) fn public_point(key: &Public) -> Vec<u8> {
    match key {
        Public::P256(key) => key.to_encoded_point(false).as_bytes().to_vec(),
        Public::P384(key) => key.to_encoded_point(false).as_bytes().to_vec(),
        Public::P521(key) => key.to_encoded_point(false).as_bytes().to_vec(),
        Public::Secp256k1(key) => key.to_encoded_point(false).as_bytes().to_vec(),
    }
}

pub(super) fn from_jwk_private(
    curve: Curve,
    x: &[u8],
    y: &[u8],
    d: &[u8],
) -> Result<Private, JsErrorBox> {
    let point = affine_point(curve, x, y)?;
    let key = match curve {
        Curve::P256 => p256::SecretKey::from_slice(d)
            .map(Private::P256)
            .map_err(|error| invalid_jwk(&error.to_string()))?,
        Curve::P384 => p384::SecretKey::from_slice(d)
            .map(Private::P384)
            .map_err(|error| invalid_jwk(&error.to_string()))?,
        Curve::P521 => p521::SecretKey::from_slice(d)
            .map(Private::P521)
            .map_err(|error| invalid_jwk(&error.to_string()))?,
        Curve::Secp256k1 => k256::SecretKey::from_slice(d)
            .map(Private::Secp256k1)
            .map_err(|error| invalid_jwk(&error.to_string()))?,
    };
    if public_point(&public_key(&key)) != point {
        return Err(invalid_jwk("JWK public key does not match its private key"));
    }
    Ok(key)
}

pub(super) fn from_jwk_public(curve: Curve, x: &[u8], y: &[u8]) -> Result<Public, JsErrorBox> {
    let point = affine_point(curve, x, y)?;
    let key = match curve {
        Curve::P256 => p256::PublicKey::from_sec1_bytes(&point).map(Public::P256),
        Curve::P384 => p384::PublicKey::from_sec1_bytes(&point).map(Public::P384),
        Curve::P521 => p521::PublicKey::from_sec1_bytes(&point).map(Public::P521),
        Curve::Secp256k1 => k256::PublicKey::from_sec1_bytes(&point).map(Public::Secp256k1),
    }
    .map_err(|error| invalid_jwk(&error.to_string()))?;
    Ok(key)
}

/// The uncompressed SEC1 point (`0x04 || X || Y`) for a JWK pair.
fn affine_point(curve: Curve, x: &[u8], y: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    let coordinate = match curve {
        Curve::P256 | Curve::Secp256k1 => 32,
        Curve::P384 => 48,
        Curve::P521 => 66,
    };
    if x.len() != coordinate || y.len() != coordinate {
        return Err(invalid_jwk("JWK EC coordinates have the wrong length"));
    }
    let mut point = Vec::with_capacity(1 + 2 * coordinate);
    point.push(0x04);
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    Ok(point)
}

pub(super) fn diffie_hellman(private: &Private, public: &Public) -> Result<Vec<u8>, JsErrorBox> {
    use p256::elliptic_curve::ecdh::diffie_hellman;

    let secret = match (private, public) {
        (Private::P256(private), Public::P256(public)) => {
            diffie_hellman(private.to_nonzero_scalar(), public.as_affine())
                .raw_secret_bytes()
                .to_vec()
        }
        (Private::P384(private), Public::P384(public)) => {
            diffie_hellman(private.to_nonzero_scalar(), public.as_affine())
                .raw_secret_bytes()
                .to_vec()
        }
        (Private::P521(private), Public::P521(public)) => {
            diffie_hellman(private.to_nonzero_scalar(), public.as_affine())
                .raw_secret_bytes()
                .to_vec()
        }
        (Private::Secp256k1(private), Public::Secp256k1(public)) => {
            diffie_hellman(private.to_nonzero_scalar(), public.as_affine())
                .raw_secret_bytes()
                .to_vec()
        }
        _ => {
            return Err(coded_error(
                "ERR_CRYPTO_INCOMPATIBLE_KEY",
                "Incompatible key types for Diffie-Hellman",
            ));
        }
    };
    Ok(secret)
}

fn invalid_jwk(message: &str) -> JsErrorBox {
    coded_error("ERR_CRYPTO_INVALID_JWK", message)
}
