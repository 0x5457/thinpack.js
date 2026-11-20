use deno_core::serde::{Deserialize, Serialize};
use deno_core::{JsBuffer, ToJsBuffer, op2};
use deno_error::JsErrorBox;
use openssl::asn1::Asn1Object;
use openssl::bn::BigNum;
use openssl::derive::Deriver;
use openssl::dh::Dh;
use openssl::dsa::Dsa;
use openssl::ec::{Asn1Flag, EcGroup, EcKey};
use openssl::error::ErrorStack;
use openssl::pkey::{HasPublic, Id, PKey, PKeyRef, Private, Public};
use openssl::pkey_ctx::PkeyCtx;
use openssl::rsa::Rsa;
use openssl::symm::Cipher;
use openssl::x509::X509;

use super::join_error;

mod jwk;

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

impl KeyGeneration {
    fn generate(self) -> Result<PKey<Private>, JsErrorBox> {
        let key = match self {
            Self::Rsa { bits, exponent } => (|| {
                let exponent = BigNum::from_u32(exponent)?;
                PKey::from_rsa(Rsa::generate_with_e(bits, &exponent)?)
            })(),
            Self::RsaPss { bits, exponent } => rsa_pss_key(bits, exponent),
            Self::Dsa { bits } => Dsa::generate(bits).and_then(PKey::from_dsa),
            Self::Ec { curve, explicit } => {
                let mut group = named_group(&curve)?;
                if explicit {
                    group.set_asn1_flag(Asn1Flag::EXPLICIT_CURVE);
                }
                EcKey::generate(&group).and_then(PKey::from_ec_key)
            }
            Self::Ed25519 => PKey::generate_ed25519(),
            Self::Ed448 => PKey::generate_ed448(),
            Self::X25519 => PKey::generate_x25519(),
            Self::X448 => PKey::generate_x448(),
            Self::Dh {
                group,
                bits,
                prime,
                generator,
            } => group
                .as_deref()
                .map_or_else(|| dh_key(bits, prime.as_deref(), generator), dh_group)?
                .generate_key()
                .and_then(PKey::from_dh),
        };
        key.map_err(|error| crypto_error("ERR_CRYPTO_KEYGEN_FAILED", &error))
    }
}

fn rsa_pss_key(bits: u32, exponent: u32) -> Result<PKey<Private>, ErrorStack> {
    let mut context = PkeyCtx::new_id(Id::RSA_PSS)?;
    context.keygen_init()?;
    context.set_rsa_keygen_bits(bits)?;
    let exponent = BigNum::from_u32(exponent)?;
    context.set_rsa_keygen_pubexp(&exponent)?;
    context.keygen()
}

fn named_group(curve: &str) -> Result<EcGroup, JsErrorBox> {
    let alias = match curve {
        "P-192" | "secp192r1" => "prime192v1",
        "P-224" => "secp224r1",
        "P-256" | "secp256r1" => "prime256v1",
        "P-384" => "secp384r1",
        "P-521" => "secp521r1",
        name => name,
    };
    let group = (!alias.contains('\0'))
        .then(|| Asn1Object::from_str(alias))
        .and_then(Result::ok)
        .and_then(|object| EcGroup::from_curve_name(object.nid()).ok());
    group.ok_or_else(|| {
        coded_error(
            "ERR_CRYPTO_INVALID_CURVE",
            &format!("Invalid EC curve name: {curve}"),
        )
    })
}

fn dh_group(group: &str) -> Result<Dh<openssl::pkey::Params>, JsErrorBox> {
    let prime = match group.to_ascii_lowercase().as_str() {
        "modp1" => BigNum::get_rfc2409_prime_768(),
        "modp2" => BigNum::get_rfc2409_prime_1024(),
        "modp5" => BigNum::get_rfc3526_prime_1536(),
        "modp14" => BigNum::get_rfc3526_prime_2048(),
        "modp15" => BigNum::get_rfc3526_prime_3072(),
        "modp16" => BigNum::get_rfc3526_prime_4096(),
        "modp17" => BigNum::get_rfc3526_prime_6144(),
        "modp18" => BigNum::get_rfc3526_prime_8192(),
        _ => {
            return Err(coded_error(
                "ERR_CRYPTO_UNKNOWN_DH_GROUP",
                &format!("Unknown DH group: {group}"),
            ));
        }
    };
    prime
        .and_then(|prime| Dh::from_pqg(prime, None, BigNum::from_u32(2)?))
        .map_err(|error| crypto_error("ERR_CRYPTO_KEYGEN_FAILED", &error))
}

fn dh_key(
    bits: Option<u32>,
    prime: Option<&[u8]>,
    generator: u32,
) -> Result<Dh<openssl::pkey::Params>, JsErrorBox> {
    let result = match (prime, bits) {
        (Some(prime), None) => (|| {
            Dh::from_pqg(
                BigNum::from_slice(prime)?,
                None,
                BigNum::from_u32(generator)?,
            )
        })(),
        (None, Some(bits)) => Dh::generate_params(bits, generator),
        _ => {
            return Err(coded_error(
                "ERR_INVALID_ARG_VALUE",
                "Supply a prime or a prime length",
            ));
        }
    };
    result.map_err(|error| crypto_error("ERR_CRYPTO_KEYGEN_FAILED", &error))
}

const fn key_kind(id: Id) -> &'static str {
    match id {
        Id::RSA => "rsa",
        Id::RSA_PSS => "rsa-pss",
        Id::DSA => "dsa",
        Id::DH | Id::DHX => "dh",
        Id::EC | Id::SM2 => "ec",
        Id::ED25519 => "ed25519",
        Id::ED448 => "ed448",
        Id::X25519 => "x25519",
        Id::X448 => "x448",
        _ => "",
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
    fn from_key(key: &PKeyRef<impl HasPublic>) -> Result<Self, ErrorStack> {
        let mut details = Self::default();
        match key.id() {
            Id::EC | Id::SM2 => {
                details.named_curve = key
                    .ec_key()?
                    .group()
                    .curve_name()
                    .map(|nid| nid.short_name())
                    .transpose()?;
            }
            Id::RSA | Id::RSA_PSS => {
                details.modulus_length = Some(key.bits());
                details.public_exponent = Some(key.rsa()?.e().to_vec().into());
            }
            Id::DSA => {
                details.modulus_length = Some(key.bits());
                details.divisor_length = Some(key.dsa()?.q().num_bits());
            }
            _ => {}
        }
        Ok(details)
    }
}

impl KeyData {
    fn new(key: &PKeyRef<impl HasPublic>, der: Vec<u8>) -> Result<Self, ErrorStack> {
        Ok(Self {
            der: der.into(),
            kind: key_kind(key.id()),
            details: KeyDetails::from_key(key)?,
        })
    }
}

#[derive(Serialize)]
#[serde(crate = "deno_core::serde", rename_all = "camelCase")]
struct KeyPair {
    public_key: KeyData,
    private_key: KeyData,
}

fn generate_key_pair(generation: KeyGeneration) -> Result<KeyPair, JsErrorBox> {
    let key = generation.generate()?;
    let pair = || {
        Ok(KeyPair {
            public_key: KeyData::new(&key, key.public_key_to_der()?)?,
            private_key: KeyData::new(&key, key.private_key_to_pkcs8()?)?,
        })
    };
    pair().map_err(|error| crypto_error("ERR_CRYPTO_KEYGEN_FAILED", &error))
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

fn read_private(
    format: &str,
    encoding: &str,
    key: &[u8],
    passphrase: &[u8],
) -> Result<PKey<Private>, ErrorStack> {
    let password = |output: &mut [u8]| {
        output
            .get_mut(..passphrase.len())
            .ok_or_else(ErrorStack::get)?
            .copy_from_slice(passphrase);
        Ok(passphrase.len())
    };
    if format == "pem" {
        PKey::private_key_from_pem_callback(key, password)
    } else {
        let unencrypted = if encoding == "pkcs8" {
            PKey::private_key_from_pkcs8(key)
        } else {
            PKey::private_key_from_der(key)
        };
        unencrypted.or_else(|_| PKey::private_key_from_pkcs8_callback(key, password))
    }
}

fn read_public(format: &str, encoding: &str, key: &[u8]) -> Result<PKey<Public>, ErrorStack> {
    if format == "pem" {
        PKey::public_key_from_pem(key)
            .or_else(|_| Rsa::public_key_from_pem_pkcs1(key).and_then(PKey::from_rsa))
            .or_else(|_| X509::from_pem(key).and_then(|cert| cert.public_key()))
    } else {
        match encoding {
            "pkcs1" => Rsa::public_key_from_der_pkcs1(key).and_then(PKey::from_rsa),
            "spki" => PKey::public_key_from_der(key),
            _ => PKey::public_key_from_der(key)
                .or_else(|_| Rsa::public_key_from_der_pkcs1(key).and_then(PKey::from_rsa)),
        }
    }
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
    let import = || {
        if kind == "private" {
            let key = read_private(format, encoding, key, passphrase)?;
            KeyData::new(&key, key.private_key_to_pkcs8()?)
        } else {
            if format == "pem" || !matches!(encoding, "pkcs8" | "sec1") {
                match read_public(format, encoding, key) {
                    Ok(key) => return KeyData::new(&key, key.public_key_to_der()?),
                    Err(error) if format == "der" && encoding == "spki" => return Err(error),
                    Err(_) => {}
                }
            }
            let private = read_private(format, encoding, key, passphrase)?;
            KeyData::new(&private, private.public_key_to_der()?)
        }
    };
    import().map_err(|error| crypto_error("ERR_CRYPTO_INVALID_KEY", &error))
}

#[op2]
#[serde]
pub(super) fn op_crypto_import_jwk(
    #[string] kind: &str,
    #[serde] jwk: jwk::Jwk,
) -> Result<KeyData, JsErrorBox> {
    jwk.import(kind == "private")
}

#[op2]
#[serde]
pub(super) fn op_crypto_export_jwk(
    #[string] kind: &str,
    #[buffer] der: &[u8],
) -> Result<jwk::Jwk, JsErrorBox> {
    let invalid = |error| crypto_error("ERR_CRYPTO_INVALID_KEY", &error);
    if kind == "private" {
        let key = PKey::private_key_from_pkcs8(der).map_err(invalid)?;
        jwk::Jwk::export(&key, Some(&key))
    } else {
        let key = PKey::public_key_from_der(der).map_err(invalid)?;
        jwk::Jwk::export(&key, None)
    }
}

fn export_private(
    key: &PKeyRef<Private>,
    format: &str,
    encoding: &str,
    cipher: Option<Cipher>,
    passphrase: &[u8],
) -> Result<Vec<u8>, ErrorStack> {
    match (encoding, format, cipher) {
        ("pkcs1", "pem", Some(cipher)) => {
            key.rsa()?.private_key_to_pem_passphrase(cipher, passphrase)
        }
        ("pkcs1", "pem", None) => key.rsa()?.private_key_to_pem(),
        ("pkcs1", _, _) => key.rsa()?.private_key_to_der(),
        ("sec1", "pem", Some(cipher)) => key
            .ec_key()?
            .private_key_to_pem_passphrase(cipher, passphrase),
        ("sec1", "pem", None) => key.ec_key()?.private_key_to_pem(),
        ("sec1", _, _) => key.ec_key()?.private_key_to_der(),
        (_, "pem", Some(cipher)) => key.private_key_to_pem_pkcs8_passphrase(cipher, passphrase),
        (_, "pem", None) => key.private_key_to_pem_pkcs8(),
        (_, _, Some(cipher)) => key.private_key_to_pkcs8_passphrase(cipher, passphrase),
        (_, _, None) => key.private_key_to_pkcs8(),
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
    let cipher = if cipher.is_empty() {
        None
    } else {
        Some(
            (!cipher.contains('\0'))
                .then(|| Asn1Object::from_str(cipher))
                .and_then(Result::ok)
                .and_then(|object| Cipher::from_nid(object.nid()))
                .ok_or_else(|| {
                    coded_error(
                        "ERR_CRYPTO_UNKNOWN_CIPHER",
                        &format!("Unknown cipher: {cipher}"),
                    )
                })?,
        )
    };
    let export = || {
        if kind == "private" {
            let key = PKey::private_key_from_pkcs8(der)?;
            export_private(&key, format, encoding, cipher, passphrase)
        } else {
            let key = PKey::public_key_from_der(der)?;
            match (encoding, format) {
                ("pkcs1", "pem") => key.rsa()?.public_key_to_pem_pkcs1(),
                ("pkcs1", _) => key.rsa()?.public_key_to_der_pkcs1(),
                (_, "pem") => key.public_key_to_pem(),
                _ => key.public_key_to_der(),
            }
        }
    };
    export()
        .map(Vec::into_boxed_slice)
        .map_err(|error| crypto_error("ERR_CRYPTO_INVALID_KEY", &error))
}

fn derive_secret(private: &[u8], public: &[u8]) -> Result<Box<[u8]>, JsErrorBox> {
    let derive = || {
        let private = PKey::private_key_from_pkcs8(private)?;
        let public = PKey::public_key_from_der(public)?;
        let mut deriver = Deriver::new(&private)?;
        deriver.set_peer(&public)?;
        deriver.derive_to_vec().map(Vec::into_boxed_slice)
    };
    derive().map_err(|error| crypto_error("ERR_CRYPTO_OPERATION_FAILED", &error))
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

fn crypto_error(code: &str, error: &ErrorStack) -> JsErrorBox {
    coded_error(code, &error.to_string())
}
