use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use deno_core::serde::{Deserialize, Serialize};
use deno_error::JsErrorBox;
use openssl::bn::{BigNum, BigNumContext};
use openssl::ec::{EcKey, PointConversionForm};
use openssl::nid::Nid;
use openssl::pkey::{HasPublic, Id, PKey, PKeyRef, Private, Public};
use openssl::rsa::Rsa;

use super::{KeyData, coded_error, crypto_error, named_group};

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

impl RsaPrivate {
    fn from_key(key: &PKeyRef<Private>) -> Result<Self, JsErrorBox> {
        let key = key.rsa().map_err(invalid_key)?;
        let (Some(p), Some(q), Some(dp), Some(dq), Some(qi)) =
            (key.p(), key.q(), key.dmp1(), key.dmq1(), key.iqmp())
        else {
            return Err(invalid_jwk("RSA key is missing private parameters"));
        };
        Ok(Self {
            d: encode(key.d().to_vec()),
            p: encode(p.to_vec()),
            q: encode(q.to_vec()),
            dp: encode(dp.to_vec()),
            dq: encode(dq.to_vec()),
            qi: encode(qi.to_vec()),
        })
    }
}

impl Jwk {
    pub(super) fn export(
        key: &PKeyRef<impl HasPublic>,
        private: Option<&PKeyRef<Private>>,
    ) -> Result<Self, JsErrorBox> {
        Ok(match key.id() {
            Id::RSA => {
                let rsa = key.rsa().map_err(invalid_key)?;
                Self::Rsa {
                    n: encode(rsa.n().to_vec()),
                    e: encode(rsa.e().to_vec()),
                    private: private.map(RsaPrivate::from_key).transpose()?,
                }
            }
            Id::EC => {
                let ec = key.ec_key().map_err(invalid_key)?;
                let group = ec.group();
                let curve = match group.curve_name() {
                    Some(Nid::X9_62_PRIME256V1) => "P-256",
                    Some(Nid::SECP384R1) => "P-384",
                    Some(Nid::SECP521R1) => "P-521",
                    Some(Nid::SECP256K1) => "secp256k1",
                    _ => {
                        return Err(coded_error(
                            "ERR_CRYPTO_JWK_UNSUPPORTED_CURVE",
                            "Unsupported JWK EC curve",
                        ));
                    }
                };
                let mut context = BigNumContext::new().map_err(invalid_key)?;
                let point = ec
                    .public_key()
                    .to_bytes(group, PointConversionForm::UNCOMPRESSED, &mut context)
                    .map_err(invalid_key)?;
                let (x, y) = point[1..].split_at((point.len() - 1) / 2);
                let d = private
                    .map(|key| {
                        let size =
                            i32::try_from(group.degree().div_ceil(8)).expect("known curve size");
                        let scalar = key.ec_key()?.private_key().to_vec_padded(size)?;
                        Ok(encode(scalar))
                    })
                    .transpose()
                    .map_err(invalid_key)?;
                Self::Ec {
                    crv: curve.into(),
                    x: encode(x),
                    y: encode(y),
                    d,
                }
            }
            id => {
                let curve = match id {
                    Id::ED25519 => "Ed25519",
                    Id::ED448 => "Ed448",
                    Id::X25519 => "X25519",
                    Id::X448 => "X448",
                    _ => {
                        return Err(coded_error(
                            "ERR_CRYPTO_JWK_UNSUPPORTED_KEY_TYPE",
                            "Unsupported JWK Key Type.",
                        ));
                    }
                };
                let d = private
                    .map(PKeyRef::raw_private_key)
                    .transpose()
                    .map_err(invalid_key)?
                    .map(encode);
                Self::Okp {
                    crv: curve.into(),
                    x: encode(key.raw_public_key().map_err(invalid_key)?),
                    d,
                }
            }
        })
    }

    pub(super) fn import(self, as_private: bool) -> Result<KeyData, JsErrorBox> {
        if as_private {
            let key = self.private_key()?;
            let der = key.private_key_to_pkcs8().map_err(invalid_key)?;
            KeyData::new(&key, der).map_err(invalid_key)
        } else {
            let key = self.public_key()?;
            let der = key.public_key_to_der().map_err(invalid_key)?;
            KeyData::new(&key, der).map_err(invalid_key)
        }
    }

    fn public_key(self) -> Result<PKey<Public>, JsErrorBox> {
        Ok(match self {
            Self::Rsa { n, e, .. } => {
                let key = Rsa::from_public_components(decode_int(&n)?, decode_int(&e)?)
                    .map_err(invalid_key)?;
                PKey::from_rsa(key).map_err(invalid_key)?
            }
            Self::Ec { crv, x, y, .. } => {
                PKey::from_ec_key(ec_public_key(&crv, &x, &y)?).map_err(invalid_key)?
            }
            Self::Okp { crv, x, .. } => {
                PKey::public_key_from_raw_bytes(&decode(&x)?, okp_id(&crv)?).map_err(invalid_key)?
            }
        })
    }

    fn private_key(self) -> Result<PKey<Private>, JsErrorBox> {
        Ok(match self {
            Self::Rsa {
                n,
                e,
                private: Some(private),
            } => {
                let key = Rsa::from_private_components(
                    decode_int(&n)?,
                    decode_int(&e)?,
                    decode_int(&private.d)?,
                    decode_int(&private.p)?,
                    decode_int(&private.q)?,
                    decode_int(&private.dp)?,
                    decode_int(&private.dq)?,
                    decode_int(&private.qi)?,
                )
                .map_err(invalid_key)?;
                if !key.check_key().map_err(invalid_key)? {
                    return Err(invalid_jwk("Invalid RSA private key"));
                }
                PKey::from_rsa(key).map_err(invalid_key)?
            }
            Self::Ec {
                crv,
                x,
                y,
                d: Some(d),
            } => {
                let public = ec_public_key(&crv, &x, &y)?;
                let scalar = decode_int(&d)?;
                let key =
                    EcKey::from_private_components(public.group(), &scalar, public.public_key())
                        .map_err(invalid_key)?;
                key.check_key().map_err(invalid_key)?;
                PKey::from_ec_key(key).map_err(invalid_key)?
            }
            Self::Okp { crv, x, d: Some(d) } => {
                let key = PKey::private_key_from_raw_bytes(&decode(&d)?, okp_id(&crv)?)
                    .map_err(invalid_key)?;
                if key.raw_public_key().map_err(invalid_key)? != decode(&x)? {
                    return Err(invalid_jwk("JWK public key does not match its private key"));
                }
                key
            }
            _ => return Err(invalid_jwk("JWK is missing private parameters")),
        })
    }
}

fn ec_public_key(crv: &str, x: &str, y: &str) -> Result<EcKey<Public>, JsErrorBox> {
    if !matches!(crv, "P-256" | "P-384" | "P-521" | "secp256k1") {
        return Err(invalid_jwk("Unsupported JWK EC curve"));
    }
    let group = named_group(crv)?;
    let key = EcKey::from_public_key_affine_coordinates(&group, &*decode_int(x)?, &*decode_int(y)?)
        .map_err(invalid_key)?;
    key.check_key().map_err(invalid_key)?;
    Ok(key)
}

fn okp_id(curve: &str) -> Result<Id, JsErrorBox> {
    match curve {
        "Ed25519" => Ok(Id::ED25519),
        "Ed448" => Ok(Id::ED448),
        "X25519" => Ok(Id::X25519),
        "X448" => Ok(Id::X448),
        _ => Err(invalid_jwk("Unsupported JWK OKP curve")),
    }
}

fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode(value: &str) -> Result<Vec<u8>, JsErrorBox> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| invalid_jwk("Invalid base64url encoded JWK parameter"))
}

fn decode_int(value: &str) -> Result<BigNum, JsErrorBox> {
    BigNum::from_slice(&decode(value)?).map_err(invalid_key)
}

fn invalid_jwk(message: &str) -> JsErrorBox {
    coded_error("ERR_CRYPTO_INVALID_JWK", message)
}

#[expect(clippy::needless_pass_by_value, reason = "used as a map_err function")]
fn invalid_key(error: openssl::error::ErrorStack) -> JsErrorBox {
    crypto_error("ERR_CRYPTO_INVALID_JWK", &error)
}
