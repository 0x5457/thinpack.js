//! JSON Web Key import and export for the key types this module supports.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use deno_core::serde::{Deserialize, Serialize};
use deno_error::JsErrorBox;

use super::ec;
use super::{KeyData, PrivateKey, PublicKey, coded_error};

#[derive(Deserialize, Serialize)]
#[serde(crate = "deno_core::serde", tag = "kty")]
pub(super) enum Jwk {
    #[serde(rename = "RSA")]
    Rsa {
        n: String,
        e: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        private: Option<RsaPrivate>,
    },
    #[serde(rename = "EC")]
    Ec {
        crv: String,
        x: String,
        y: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        d: Option<String>,
    },
    #[serde(rename = "OKP")]
    Okp {
        crv: String,
        x: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        d: Option<String>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(crate = "deno_core::serde")]
pub(super) struct RsaPrivate {
    d: String,
    p: String,
    q: String,
    dp: String,
    dq: String,
    qi: String,
}

pub(super) fn export_private(key: &PrivateKey) -> Result<Jwk, JsErrorBox> {
    match key {
        PrivateKey::Rsa { key, .. } => {
            let numbers = super::rsa::private_numbers(&key.key)?;
            Ok(Jwk::Rsa {
                n: encode(numbers.n),
                e: encode(numbers.e),
                private: Some(RsaPrivate {
                    d: encode(numbers.d),
                    p: encode(numbers.p),
                    q: encode(numbers.q),
                    dp: encode(numbers.dp),
                    dq: encode(numbers.dq),
                    qi: encode(numbers.qi),
                }),
            })
        }
        PrivateKey::Ec(key) => {
            let (x, y) = coordinates(&ec::public_key(key));
            Ok(Jwk::Ec {
                crv: ec::curve(key).jwk_name().to_owned(),
                x: encode(x),
                y: encode(y),
                d: Some(encode(ec::secret_bytes(key))),
            })
        }
        PrivateKey::Ed25519(key) => Ok(Jwk::Okp {
            crv: "Ed25519".to_owned(),
            x: encode(key.verifying_key().as_bytes()),
            d: Some(encode(key.to_bytes())),
        }),
        PrivateKey::X25519(key) => Ok(Jwk::Okp {
            crv: "X25519".to_owned(),
            x: encode(x25519_dalek::PublicKey::from(key).as_bytes()),
            d: Some(encode(key.to_bytes())),
        }),
    }
}

pub(super) fn export_public(key: &PublicKey) -> Jwk {
    match key {
        PublicKey::Rsa { key, .. } => {
            let (n, e) = super::rsa::public_numbers(&key.key);
            Jwk::Rsa {
                n: encode(n),
                e: encode(e),
                private: None,
            }
        }
        PublicKey::Ec(key) => {
            let (x, y) = coordinates(key);
            Jwk::Ec {
                crv: ec::public_curve(key).jwk_name().to_owned(),
                x: encode(x),
                y: encode(y),
                d: None,
            }
        }
        PublicKey::Ed25519(key) => Jwk::Okp {
            crv: "Ed25519".to_owned(),
            x: encode(key.as_bytes()),
            d: None,
        },
        PublicKey::X25519(key) => Jwk::Okp {
            crv: "X25519".to_owned(),
            x: encode(key.as_bytes()),
            d: None,
        },
    }
}

pub(super) fn import(jwk: Jwk, as_private: bool) -> Result<KeyData, JsErrorBox> {
    if as_private {
        KeyData::from_private(&private_key(jwk)?)
    } else {
        KeyData::from_public(&public_key(jwk)?)
    }
}

fn private_key(jwk: Jwk) -> Result<PrivateKey, JsErrorBox> {
    match jwk {
        Jwk::Rsa {
            n,
            e,
            private: Some(private),
        } => Ok(PrivateKey::Rsa {
            key: super::rsa::from_jwk_private(
                &decode(&n)?,
                &decode(&e)?,
                &decode(&private.d)?,
                &decode(&private.p)?,
                &decode(&private.q)?,
            )?,
            pss: false,
        }),
        Jwk::Rsa { private: None, .. } | Jwk::Ec { d: None, .. } | Jwk::Okp { d: None, .. } => {
            Err(invalid_jwk("JWK is missing private parameters"))
        }
        Jwk::Ec {
            crv,
            x,
            y,
            d: Some(d),
        } => {
            let curve = curve(&crv)?;
            Ok(PrivateKey::Ec(ec::from_jwk_private(
                curve,
                &decode(&x)?,
                &decode(&y)?,
                &decode(&d)?,
            )?))
        }
        Jwk::Okp { crv, x, d: Some(d) } => okp_private(&crv, &x, &d),
    }
}

fn public_key(jwk: Jwk) -> Result<PublicKey, JsErrorBox> {
    match jwk {
        Jwk::Rsa { n, e, .. } => Ok(PublicKey::Rsa {
            key: super::rsa::from_jwk_public(&decode(&n)?, &decode(&e)?)?,
            pss: false,
        }),
        Jwk::Ec { crv, x, y, .. } => {
            let curve = curve(&crv)?;
            Ok(PublicKey::Ec(ec::from_jwk_public(
                curve,
                &decode(&x)?,
                &decode(&y)?,
            )?))
        }
        Jwk::Okp { crv, x, .. } => okp_public(&crv, &x),
    }
}

fn okp_private(curve: &str, x: &str, d: &str) -> Result<PrivateKey, JsErrorBox> {
    let seed = decode(d)?;
    let x = decode(x)?;
    match curve {
        "Ed25519" => {
            let seed: [u8; 32] = seed
                .try_into()
                .map_err(|_| invalid_jwk("Ed25519 private key has the wrong length"))?;
            let key = ed25519_dalek::SigningKey::from_bytes(&seed);
            if key.verifying_key().as_bytes() != x.as_slice() {
                return Err(invalid_jwk("JWK public key does not match its private key"));
            }
            Ok(PrivateKey::Ed25519(key))
        }
        "X25519" => {
            let seed: [u8; 32] = seed
                .try_into()
                .map_err(|_| invalid_jwk("X25519 private key has the wrong length"))?;
            let key = x25519_dalek::StaticSecret::from(seed);
            if x25519_dalek::PublicKey::from(&key).as_bytes() != x.as_slice() {
                return Err(invalid_jwk("JWK public key does not match its private key"));
            }
            Ok(PrivateKey::X25519(key))
        }
        other => Err(unsupported_curve(other)),
    }
}

fn okp_public(curve: &str, x: &str) -> Result<PublicKey, JsErrorBox> {
    let bytes = decode(x)?;
    match curve {
        "Ed25519" => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| invalid_jwk("Ed25519 public key has the wrong length"))?;
            Ok(PublicKey::Ed25519(
                ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                    .map_err(|error| invalid_jwk(&error.to_string()))?,
            ))
        }
        "X25519" => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| invalid_jwk("X25519 public key has the wrong length"))?;
            Ok(PublicKey::X25519(x25519_dalek::PublicKey::from(bytes)))
        }
        other => Err(unsupported_curve(other)),
    }
}

fn curve(name: &str) -> Result<ec::Curve, JsErrorBox> {
    ec::Curve::from_jwk_name(name).ok_or_else(|| unsupported_curve(name))
}

fn coordinates(key: &ec::Public) -> (Vec<u8>, Vec<u8>) {
    let point = ec::public_point(key);
    let (x, y) = point[1..].split_at((point.len() - 1) / 2);
    (x.to_vec(), y.to_vec())
}

fn unsupported_curve(name: &str) -> JsErrorBox {
    coded_error(
        "ERR_CRYPTO_JWK_UNSUPPORTED_CURVE",
        &format!("Unsupported JWK curve `{name}`"),
    )
}

fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode(value: &str) -> Result<Vec<u8>, JsErrorBox> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_jwk("Invalid base64url encoded JWK parameter"))
}

fn invalid_jwk(message: &str) -> JsErrorBox {
    coded_error("ERR_CRYPTO_INVALID_JWK", message)
}
