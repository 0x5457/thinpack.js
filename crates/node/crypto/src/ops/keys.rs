//! Node's `KeyObject` surface over `RustCrypto` key material.
//!
//! Private keys are stored as PKCS#8 DER and public keys as SPKI DER, the same contract the
//! previous OpenSSL implementation used: the signing ops hand those bytes to ring, and the JS
//! side only ever sees the normalized DER plus the asymmetric-key metadata.
//!
//! RSA, EC, Ed25519 and X25519 are implemented; DSA, DH, Ed448 and X448 are not available in
//! this build and report `ERR_CRYPTO_OPERATION_FAILED` instead of pulling a second crypto
//! library in.

use std::fmt::Display;

use deno_core::serde::{Deserialize, Serialize};
use deno_core::{JsBuffer, ToJsBuffer, op2};
use deno_error::JsErrorBox;
use der::Decode as _;
use der::Encode as _;
use rand_core::RngCore as _;

use super::join_error;

mod ec;
mod jwk;
mod okp;
mod rsa;

#[derive(Deserialize)]
#[serde(crate = "deno_core::serde", tag = "type", rename_all = "kebab-case")]
enum KeyGeneration {
    Rsa {
        bits: u32,
        exponent: u32,
    },
    RsaPss {
        bits: u32,
        exponent: u32,
    },
    Dsa {
        bits: u32,
    },
    Ec {
        curve: String,
        explicit: bool,
    },
    Ed25519,
    Ed448,
    X25519,
    X448,
    Dh {
        group: Option<String>,
        bits: Option<u32>,
        prime: Option<Vec<u8>>,
        generator: u32,
    },
}

#[derive(Clone, Copy)]
enum KeyKind {
    Rsa,
    RsaPss,
    Ec,
    Ed25519,
    X25519,
}

impl KeyKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Rsa => "rsa",
            Self::RsaPss => "rsa-pss",
            Self::Ec => "ec",
            Self::Ed25519 => "ed25519",
            Self::X25519 => "x25519",
        }
    }
}

enum PrivateKey {
    Rsa { key: rsa::Private, pss: bool },
    Ec(ec::Private),
    Ed25519(ed25519_dalek::SigningKey),
    X25519(x25519_dalek::StaticSecret),
}

enum PublicKey {
    Rsa { key: rsa::Public, pss: bool },
    Ec(ec::Public),
    Ed25519(ed25519_dalek::VerifyingKey),
    X25519(x25519_dalek::PublicKey),
}

impl PrivateKey {
    const fn kind(&self) -> KeyKind {
        match self {
            Self::Rsa { pss, .. } if *pss => KeyKind::RsaPss,
            Self::Rsa { .. } => KeyKind::Rsa,
            Self::Ec(_) => KeyKind::Ec,
            Self::Ed25519(_) => KeyKind::Ed25519,
            Self::X25519(_) => KeyKind::X25519,
        }
    }

    fn details(&self) -> Result<KeyDetails, JsErrorBox> {
        match self {
            Self::Rsa { key, .. } => KeyDetails::rsa(&key.key.to_public_key()),
            Self::Ec(key) => Ok(KeyDetails::ec(ec::curve(key))),
            Self::Ed25519(_) | Self::X25519(_) => Ok(KeyDetails::default()),
        }
    }

    fn to_pkcs8_der(&self) -> Result<Vec<u8>, JsErrorBox> {
        match self {
            Self::Rsa { key, .. } => rsa::pkcs8_der(&key.key),
            Self::Ec(key) => ec::pkcs8_der(key),
            Self::Ed25519(key) => okp::ed25519_private_der(key),
            Self::X25519(key) => okp::x25519_private_der(key),
        }
    }

    fn to_public(&self) -> PublicKey {
        match self {
            Self::Rsa { key, pss } => PublicKey::Rsa {
                key: rsa::Public {
                    key: key.key.to_public_key(),
                    pss: *pss,
                },
                pss: *pss,
            },
            Self::Ec(key) => PublicKey::Ec(ec::public_key(key)),
            Self::Ed25519(key) => PublicKey::Ed25519(key.verifying_key()),
            Self::X25519(key) => PublicKey::X25519(x25519_dalek::PublicKey::from(key)),
        }
    }
}

impl PublicKey {
    const fn kind(&self) -> KeyKind {
        match self {
            Self::Rsa { pss, .. } if *pss => KeyKind::RsaPss,
            Self::Rsa { .. } => KeyKind::Rsa,
            Self::Ec(_) => KeyKind::Ec,
            Self::Ed25519(_) => KeyKind::Ed25519,
            Self::X25519(_) => KeyKind::X25519,
        }
    }

    fn details(&self) -> Result<KeyDetails, JsErrorBox> {
        match self {
            Self::Rsa { key, .. } => KeyDetails::rsa(&key.key),
            Self::Ec(key) => Ok(KeyDetails::ec(ec::public_curve(key))),
            Self::Ed25519(_) | Self::X25519(_) => Ok(KeyDetails::default()),
        }
    }

    fn to_spki_der(&self) -> Result<Vec<u8>, JsErrorBox> {
        match self {
            Self::Rsa { key, .. } => rsa::spki_der(&key.key),
            Self::Ec(key) => ec::spki_der(key),
            Self::Ed25519(key) => okp::ed25519_public_der(key),
            Self::X25519(key) => okp::x25519_public_der(key),
        }
    }
}

#[derive(Serialize)]
#[serde(crate = "deno_core::serde", rename_all = "camelCase")]
struct KeyData {
    der: ToJsBuffer,
    kind: &'static str,
    #[serde(flatten)]
    details: KeyDetails,
}

#[derive(Serialize, Default)]
#[serde(crate = "deno_core::serde", rename_all = "camelCase")]
struct KeyDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    named_curve: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modulus_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    divisor_length: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_exponent: Option<ToJsBuffer>,
}

impl KeyDetails {
    fn rsa(key: &::rsa::RsaPublicKey) -> Result<Self, JsErrorBox> {
        let (_, exponent) = rsa::public_numbers(key);
        Ok(Self {
            modulus_length: Some(rsa::modulus_length(key)?),
            public_exponent: Some(exponent.into()),
            ..Self::default()
        })
    }

    const fn ec(curve: ec::Curve) -> Self {
        Self {
            named_curve: Some(curve.key_name()),
            modulus_length: None,
            divisor_length: None,
            public_exponent: None,
        }
    }
}

impl KeyData {
    fn from_private(key: &PrivateKey) -> Result<Self, JsErrorBox> {
        Ok(Self {
            der: key.to_pkcs8_der()?.into(),
            kind: key.kind().as_str(),
            details: key.details()?,
        })
    }

    fn from_public(key: &PublicKey) -> Result<Self, JsErrorBox> {
        Ok(Self {
            der: key.to_spki_der()?.into(),
            kind: key.kind().as_str(),
            details: key.details()?,
        })
    }
}

#[derive(Serialize)]
#[serde(crate = "deno_core::serde", rename_all = "camelCase")]
struct KeyPair {
    public_key: KeyData,
    private_key: KeyData,
}

fn generate(generation: KeyGeneration) -> Result<PrivateKey, JsErrorBox> {
    match generation {
        KeyGeneration::Rsa { bits, exponent } => Ok(PrivateKey::Rsa {
            key: rsa::generate(bits, exponent, false)?,
            pss: false,
        }),
        KeyGeneration::RsaPss { bits, exponent } => Ok(PrivateKey::Rsa {
            key: rsa::generate(bits, exponent, false)?,
            pss: true,
        }),
        KeyGeneration::Ec { curve, explicit } => {
            // RustCrypto only emits named-curve parameters; `explicit` is accepted for API
            // compatibility and ignored.
            let _ = explicit;
            let curve = ec::Curve::from_name(&curve).ok_or_else(|| {
                coded_error(
                    "ERR_CRYPTO_INVALID_CURVE",
                    &format!("Invalid EC curve name: {curve}"),
                )
            })?;
            Ok(PrivateKey::Ec(ec::generate(curve)))
        }
        KeyGeneration::Ed25519 => Ok(PrivateKey::Ed25519(okp::generate_ed25519())),
        KeyGeneration::X25519 => Ok(PrivateKey::X25519(okp::generate_x25519())),
        // The RustCrypto curve/key crates that would cover these are either pre-release or
        // missing the ASN.1 wrappers Node needs; linking OpenSSL for them would give back the
        // 3.8 MiB this module just removed. Record them as gaps instead.
        KeyGeneration::Dsa { bits } => {
            let _ = bits;
            Err(unsupported("dsa"))
        }
        KeyGeneration::Ed448 => Err(unsupported("ed448")),
        KeyGeneration::X448 => Err(unsupported("x448")),
        KeyGeneration::Dh {
            group,
            bits,
            prime,
            generator,
        } => {
            let _ = (group, bits, prime, generator);
            Err(unsupported("dh"))
        }
    }
}

fn unsupported(name: &str) -> JsErrorBox {
    coded_error(
        "ERR_CRYPTO_OPERATION_FAILED",
        &format!("{name} keys are not supported by this runtime"),
    )
}

fn generate_key_pair(generation: KeyGeneration) -> Result<KeyPair, JsErrorBox> {
    let key = generate(generation)?;
    let public = key.to_public();
    Ok(KeyPair {
        public_key: KeyData::from_public(&public)?,
        private_key: KeyData::from_private(&key)?,
    })
}

#[op2]
#[serde]
pub(super) fn op_crypto_generate_key_pair(
    #[serde] generation: KeyGeneration,
) -> Result<KeyPair, JsErrorBox> {
    generate_key_pair(generation)
}

#[op2(async(lazy))]
#[serde]
pub(super) async fn op_crypto_generate_key_pair_async(
    #[serde] generation: KeyGeneration,
) -> Result<KeyPair, JsErrorBox> {
    tokio::task::spawn_blocking(move || generate_key_pair(generation))
        .await
        .map_err(|error| join_error(&error))?
}

fn parse_private_pkcs8(der: &[u8]) -> Result<PrivateKey, JsErrorBox> {
    if let Some(key) = rsa::parse_pkcs8_private(der)? {
        let pss = key.pss;
        return Ok(PrivateKey::Rsa { key, pss });
    }
    if let Some(key) = ec::parse_pkcs8(der) {
        return Ok(PrivateKey::Ec(key));
    }
    if let Some(key) = okp::ed25519_parse_private(der)? {
        return Ok(PrivateKey::Ed25519(key));
    }
    if let Some(key) = okp::x25519_parse_private(der)? {
        return Ok(PrivateKey::X25519(key));
    }
    Err(invalid_key("unsupported private key"))
}

fn parse_private_der(der: &[u8], passphrase: &[u8]) -> Result<PrivateKey, JsErrorBox> {
    if let Ok(key) = parse_private_pkcs8(der) {
        return Ok(key);
    }
    if let Some(key) = rsa::parse_pkcs1_private(der) {
        let pss = key.pss;
        return Ok(PrivateKey::Rsa { key, pss });
    }
    if let Some(key) = ec::parse_sec1(der) {
        return Ok(PrivateKey::Ec(key));
    }
    parse_private_pkcs8(&decrypt_pkcs8(der, passphrase)?)
}

fn read_private(
    format: &str,
    encoding: &str,
    key: &[u8],
    passphrase: &[u8],
) -> Result<PrivateKey, JsErrorBox> {
    if format == "pem" {
        let (label, der) = pem_block(key)?;
        return match label {
            "PRIVATE KEY" => parse_private_pkcs8(&der),
            "ENCRYPTED PRIVATE KEY" => parse_private_pkcs8(&decrypt_pkcs8(&der, passphrase)?),
            "RSA PRIVATE KEY" => rsa::parse_pkcs1_private(&der)
                .map(|key| PrivateKey::Rsa { key, pss: false })
                .ok_or_else(|| invalid_key("invalid RSA private key")),
            "EC PRIVATE KEY" => ec::parse_sec1(&der)
                .map(PrivateKey::Ec)
                .ok_or_else(|| invalid_key("invalid EC private key")),
            other => Err(invalid_key(format!(
                "unsupported private key PEM label `{other}`"
            ))),
        };
    }

    match encoding {
        "pkcs8" => {
            if let Ok(key) = parse_private_pkcs8(key) {
                Ok(key)
            } else {
                parse_private_pkcs8(&decrypt_pkcs8(key, passphrase)?)
            }
        }
        "pkcs1" => rsa::parse_pkcs1_private(key)
            .map(|key| PrivateKey::Rsa { key, pss: false })
            .ok_or_else(|| invalid_key("invalid RSA private key")),
        "sec1" => ec::parse_sec1(key)
            .map(PrivateKey::Ec)
            .ok_or_else(|| invalid_key("invalid EC private key")),
        _ => parse_private_der(key, passphrase),
    }
}

fn parse_public_spki(der: &[u8]) -> Result<PublicKey, JsErrorBox> {
    if let Some(key) = rsa::parse_spki_public(der)? {
        let pss = key.pss;
        return Ok(PublicKey::Rsa { key, pss });
    }
    if let Some(key) = ec::parse_spki(der) {
        return Ok(PublicKey::Ec(key));
    }
    if let Some(key) = okp::ed25519_parse_public(der)? {
        return Ok(PublicKey::Ed25519(key));
    }
    if let Some(key) = okp::x25519_parse_public(der)? {
        return Ok(PublicKey::X25519(key));
    }
    Err(invalid_key("unsupported public key"))
}

fn parse_public_der(der: &[u8]) -> Result<PublicKey, JsErrorBox> {
    if let Ok(key) = parse_public_spki(der) {
        return Ok(key);
    }
    if let Some(key) = rsa::parse_pkcs1_public(der) {
        let pss = key.pss;
        return Ok(PublicKey::Rsa { key, pss });
    }
    parse_certificate(der)
}

fn read_public(format: &str, encoding: &str, key: &[u8]) -> Result<PublicKey, JsErrorBox> {
    if format == "pem" {
        let (label, der) = pem_block(key)?;
        return match label {
            "PUBLIC KEY" => parse_public_spki(&der),
            "RSA PUBLIC KEY" => rsa::parse_pkcs1_public(&der)
                .map(|key| PublicKey::Rsa { key, pss: false })
                .ok_or_else(|| invalid_key("invalid RSA public key")),
            "CERTIFICATE" => parse_certificate(&der),
            other => Err(invalid_key(format!(
                "unsupported public key PEM label `{other}`"
            ))),
        };
    }

    match encoding {
        "pkcs1" => rsa::parse_pkcs1_public(key)
            .map(|key| PublicKey::Rsa { key, pss: false })
            .ok_or_else(|| invalid_key("invalid RSA public key")),
        "spki" => parse_public_spki(key),
        _ => parse_public_der(key),
    }
}

fn parse_certificate(der: &[u8]) -> Result<PublicKey, JsErrorBox> {
    let certificate = x509_cert::Certificate::from_der(der).map_err(invalid_key)?;
    let spki = certificate
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .map_err(invalid_key)?;
    parse_public_spki(&spki)
}

#[op2]
#[serde]
pub(super) fn op_crypto_import_key(
    #[string] kind: &str,
    #[string] format: &str,
    #[string] encoding: &str,
    #[buffer] key: &[u8],
    #[buffer] passphrase: &[u8],
) -> Result<KeyData, JsErrorBox> {
    if kind == "private" {
        return KeyData::from_private(&read_private(format, encoding, key, passphrase)?);
    }

    if format == "pem" || !matches!(encoding, "pkcs8" | "sec1") {
        match read_public(format, encoding, key) {
            Ok(public) => return KeyData::from_public(&public),
            Err(error) if format == "der" && encoding == "spki" => return Err(error),
            Err(_) => {}
        }
    }
    let private = read_private(format, encoding, key, passphrase)?;
    KeyData::from_public(&private.to_public())
}

#[op2]
#[serde]
pub(super) fn op_crypto_import_jwk(
    #[string] kind: &str,
    #[serde] jwk: jwk::Jwk,
) -> Result<KeyData, JsErrorBox> {
    jwk::import(jwk, kind == "private")
}

#[op2]
#[serde]
pub(super) fn op_crypto_export_jwk(
    #[string] kind: &str,
    #[buffer] der: &[u8],
) -> Result<jwk::Jwk, JsErrorBox> {
    if kind == "private" {
        jwk::export_private(&parse_private_pkcs8(der)?)
    } else {
        Ok(jwk::export_public(&parse_public_spki(der)?))
    }
}

#[op2]
#[buffer]
pub(super) fn op_crypto_export_key(
    #[string] kind: &str,
    #[string] format: &str,
    #[string] encoding: &str,
    #[buffer] der: &[u8],
    #[string] cipher: &str,
    #[buffer] passphrase: &[u8],
) -> Result<Box<[u8]>, JsErrorBox> {
    let cipher = parse_cipher(cipher)?;
    let exported = if kind == "private" {
        export_private(
            &parse_private_pkcs8(der)?,
            format,
            encoding,
            cipher,
            passphrase,
        )?
    } else {
        export_public(&parse_public_spki(der)?, format, encoding)?
    };
    Ok(exported.into_boxed_slice())
}

fn export_private(
    key: &PrivateKey,
    format: &str,
    encoding: &str,
    cipher: Option<Cipher>,
    passphrase: &[u8],
) -> Result<Vec<u8>, JsErrorBox> {
    match encoding {
        "" | "pkcs8" => {
            let der = key.to_pkcs8_der()?;
            let (label, der) = match cipher {
                Some(cipher) => (
                    "ENCRYPTED PRIVATE KEY",
                    encrypted_pkcs8(&der, cipher, passphrase)?,
                ),
                None => ("PRIVATE KEY", der),
            };
            encode(format, label, &der)
        }
        "pkcs1" => {
            let PrivateKey::Rsa { key, .. } = key else {
                return Err(invalid_key("pkcs1 is only valid for RSA private keys"));
            };
            if cipher.is_some() {
                return Err(legacy_cipher());
            }
            encode(
                format,
                "RSA PRIVATE KEY",
                &rsa::pkcs1_private_der(&key.key)?,
            )
        }
        "sec1" => {
            let PrivateKey::Ec(key) = key else {
                return Err(invalid_key("sec1 is only valid for EC private keys"));
            };
            if cipher.is_some() {
                return Err(legacy_cipher());
            }
            encode(format, "EC PRIVATE KEY", &ec::sec1_der(key)?)
        }
        other => Err(invalid_key(format!(
            "unsupported private key encoding `{other}`"
        ))),
    }
}

fn export_public(key: &PublicKey, format: &str, encoding: &str) -> Result<Vec<u8>, JsErrorBox> {
    match encoding {
        "pkcs1" => {
            let PublicKey::Rsa { key, .. } = key else {
                return Err(invalid_key("pkcs1 is only valid for RSA public keys"));
            };
            encode(format, "RSA PUBLIC KEY", &rsa::pkcs1_public_der(&key.key)?)
        }
        _ => encode(format, "PUBLIC KEY", &key.to_spki_der()?),
    }
}

fn encode(format: &str, label: &str, der: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    if format == "pem" {
        der::pem::encode_string(label, der::pem::LineEnding::LF, der)
            .map(String::into_bytes)
            .map_err(invalid_key)
    } else {
        Ok(der.to_vec())
    }
}

fn pem_block(bytes: &[u8]) -> Result<(&str, Vec<u8>), JsErrorBox> {
    der::pem::decode_vec(bytes).map_err(|error| invalid_key(format!("invalid PEM: {error}")))
}

/// The cipher names Node maps to OpenSSL ciphers. Only the modern PKCS#8 path is supported;
/// the legacy PKCS#1/SEC1 PEM encryption needs `DEK-Info` handling that `RustCrypto` does not
/// offer.
#[derive(Clone, Copy)]
enum Cipher {
    Aes128,
    Aes192,
    Aes256,
}

fn parse_cipher(name: &str) -> Result<Option<Cipher>, JsErrorBox> {
    if name.is_empty() {
        return Ok(None);
    }
    match name.to_ascii_lowercase().as_str() {
        "aes-128-cbc" => Ok(Some(Cipher::Aes128)),
        "aes-192-cbc" => Ok(Some(Cipher::Aes192)),
        "aes-256-cbc" => Ok(Some(Cipher::Aes256)),
        _ => Err(coded_error(
            "ERR_CRYPTO_UNKNOWN_CIPHER",
            &format!("Unknown cipher: {name}"),
        )),
    }
}

fn encrypted_pkcs8(der: &[u8], cipher: Cipher, passphrase: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    let mut random = rand_core::OsRng;
    let mut salt = [0u8; 16];
    random.fill_bytes(&mut salt);
    let mut iv = [0u8; 16];
    random.fill_bytes(&mut iv);

    let kdf = pkcs5::pbes2::Pbkdf2Params::hmac_with_sha256(2048, &salt).map_err(invalid_key)?;
    let encryption = match cipher {
        Cipher::Aes128 => pkcs5::pbes2::EncryptionScheme::Aes128Cbc { iv: &iv },
        Cipher::Aes192 => pkcs5::pbes2::EncryptionScheme::Aes192Cbc { iv: &iv },
        Cipher::Aes256 => pkcs5::pbes2::EncryptionScheme::Aes256Cbc { iv: &iv },
    };
    let parameters = pkcs5::pbes2::Parameters {
        kdf: pkcs5::pbes2::Kdf::Pbkdf2(kdf),
        encryption,
    };

    pkcs8::PrivateKeyInfo::try_from(der)
        .map_err(invalid_key)?
        .encrypt_with_params(parameters, passphrase)
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

fn decrypt_pkcs8(der: &[u8], passphrase: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    let encrypted = pkcs8::EncryptedPrivateKeyInfo::try_from(der).map_err(invalid_key)?;
    encrypted
        .decrypt(passphrase)
        .map(|document| document.as_bytes().to_vec())
        .map_err(invalid_key)
}

fn legacy_cipher() -> JsErrorBox {
    coded_error(
        "ERR_CRYPTO_OPERATION_FAILED",
        "encrypted PKCS#1/SEC1 PEM is not supported; export pkcs8 instead",
    )
}

fn derive_secret(private: &[u8], public: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    let private = parse_private_pkcs8(private)?;
    let public = parse_public_spki(public)?;
    let secret = match (&private, &public) {
        (PrivateKey::X25519(private), PublicKey::X25519(public)) => {
            let shared = private.diffie_hellman(public);
            if !shared.was_contributory() {
                return Err(coded_error(
                    "ERR_CRYPTO_OPERATION_FAILED",
                    "Invalid X25519 public key",
                ));
            }
            shared.to_bytes().to_vec()
        }
        (PrivateKey::Ec(private), PublicKey::Ec(public)) => ec::diffie_hellman(private, public)?,
        _ => {
            return Err(coded_error(
                "ERR_CRYPTO_INCOMPATIBLE_KEY",
                "Incompatible key types for Diffie-Hellman",
            ));
        }
    };
    Ok(secret.into_boxed_slice())
}

#[op2]
#[buffer]
pub(super) fn op_crypto_diffie_hellman(
    #[buffer] private: &[u8],
    #[buffer] public: &[u8],
) -> Result<Box<[u8]>, JsErrorBox> {
    derive_secret(private, public)
}

#[op2(async(lazy))]
#[buffer]
pub(super) async fn op_crypto_diffie_hellman_async(
    #[buffer] private: JsBuffer,
    #[buffer] public: JsBuffer,
) -> Result<Box<[u8]>, JsErrorBox> {
    let private = private.to_vec();
    let public = public.to_vec();
    tokio::task::spawn_blocking(move || derive_secret(&private, &public))
        .await
        .map_err(|error| join_error(&error))?
}

fn coded_error(code: &str, message: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0{message}"))
}

fn crypto_error(code: &str, error: &impl Display) -> JsErrorBox {
    coded_error(code, &error.to_string())
}

fn invalid_key(error: impl Display) -> JsErrorBox {
    coded_error("ERR_CRYPTO_INVALID_KEY", &error.to_string())
}
