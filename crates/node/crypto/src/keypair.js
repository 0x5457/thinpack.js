(() => {
  const { Buffer } = require("node:buffer");
  const { codes, opError } = require("node:thinpack/internal/errors");
  const {
    validateBuffer,
    validateFunction,
    validateInt32,
    validateObject,
    validateString,
    validateUint32,
  } = require("node:thinpack/internal/validators");

  const ops = Deno.core.ops;
  const keyData = new WeakMap();
  const keyToken = Symbol();
  const empty = Buffer.alloc(0);

  function errorWithCode(code, message, ErrorType = Error) {
    const error = new ErrorType(message);
    error.code = code;
    return error;
  }

  function call(operation, ...args) {
    try {
      return operation(...args);
    } catch (error) {
      throw opError(error);
    }
  }

  function privateKeyTypeError(actual) {
    return errorWithCode(
      "ERR_CRYPTO_INVALID_KEY_OBJECT_TYPE",
      `Invalid key object type ${actual}, expected private.`,
      TypeError,
    );
  }

  function isKeyBytes(value) {
    return typeof value === "string" || ArrayBuffer.isView(value) || value instanceof ArrayBuffer;
  }

  function keyBytesTypeError(name, value) {
    return new codes.ERR_INVALID_ARG_TYPE(name, ["string", "ArrayBuffer", "Buffer", "TypedArray", "DataView"], value);
  }

  function toKeyBytes(value, name, encoding) {
    if (typeof value === "string") return Buffer.from(value, encoding);
    if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) {
      return ArrayBuffer.isView(value)
        ? Buffer.from(value.buffer, value.byteOffset, value.byteLength)
        : Buffer.from(value);
    }
    throw keyBytesTypeError(name, value);
  }

  function incompatibleKeyOptions(message) {
    return errorWithCode("ERR_CRYPTO_INCOMPATIBLE_KEY_OPTIONS", message, TypeError);
  }

  function parseEncoding(options, kind, keyType, name) {
    validateObject(options, name);
    const { format, type, cipher, passphrase, encoding } = options;
    if (format === "jwk") {
      if (kind === "private" && (cipher != null || passphrase !== undefined)) {
        throw incompatibleKeyOptions("jwk does not support encryption");
      }
      return { format };
    }
    if (format !== "pem" && format !== "der") {
      throw new codes.ERR_INVALID_ARG_VALUE(`${name}.format`, format);
    }
    if (type === "pkcs1" && keyType !== "rsa") {
      throw incompatibleKeyOptions("pkcs1 can only be used for RSA keys");
    }
    if (type === "sec1" && kind === "private" && keyType !== "ec") {
      throw incompatibleKeyOptions("sec1 can only be used for EC keys");
    }
    const allowed = kind === "private" ? ["pkcs8", "pkcs1", "sec1"] : ["spki", "pkcs1"];
    if (!allowed.includes(type)) {
      throw new codes.ERR_INVALID_ARG_VALUE(`${name}.type`, type);
    }
    if (kind === "private") {
      if (cipher != null) {
        if (typeof cipher !== "string") throw new codes.ERR_INVALID_ARG_VALUE(`${name}.cipher`, cipher);
        if (cipher === "") throw errorWithCode("ERR_CRYPTO_UNKNOWN_CIPHER", "Unknown cipher");
        if (format === "der" && type !== "pkcs8") {
          throw incompatibleKeyOptions(`${type} does not support encryption in DER format`);
        }
        if (!isKeyBytes(passphrase)) throw new codes.ERR_INVALID_ARG_VALUE(`${name}.passphrase`, passphrase);
        return { format, type, cipher, passphrase: toKeyBytes(passphrase, `${name}.passphrase`, encoding) };
      }
      if (passphrase !== undefined) {
        throw new codes.ERR_INVALID_ARG_VALUE(`${name}.cipher`, cipher, "is required when a passphrase is specified");
      }
    }
    return { format, type, cipher: "", passphrase: empty };
  }

  function exportKey(data, type, encoding) {
    if (encoding.format === "jwk") return call(ops.op_crypto_export_jwk, type, data.der);
    const bytes = Buffer.from(call(
      ops.op_crypto_export_key,
      type,
      encoding.format,
      encoding.type,
      data.der,
      encoding.cipher,
      encoding.passphrase,
    ));
    return encoding.format === "pem" ? bytes.toString() : bytes;
  }

  class KeyObject {
    constructor(token, type, data) {
      if (token !== keyToken) throw new TypeError("Illegal constructor");
      keyData.set(this, { ...data, type });
    }

    get type() { return keyData.get(this).type; }

    get asymmetricKeyType() { return keyData.get(this).kind || undefined; }

    get asymmetricKeyDetails() {
      const { namedCurve, modulusLength, divisorLength, publicExponent } = keyData.get(this);
      const details = {};
      if (namedCurve !== undefined) details.namedCurve = namedCurve;
      if (modulusLength !== undefined) details.modulusLength = modulusLength;
      if (divisorLength !== undefined) details.divisorLength = divisorLength;
      if (publicExponent !== undefined) {
        details.publicExponent = BigInt(`0x${Buffer.from(publicExponent).toString("hex")}`);
      }
      return details;
    }

    export(options) {
      const data = keyData.get(this);
      return exportKey(data, data.type, parseEncoding(options, data.type, data.kind, "options"));
    }

    equals(other) {
      const data = keyData.get(this);
      const otherData = keyData.get(other);
      if (otherData === undefined) {
        throw new codes.ERR_INVALID_ARG_TYPE("otherKeyObject", "KeyObject", other);
      }
      if (data.type !== otherData.type || data.kind !== otherData.kind || data.der.length !== otherData.der.length) {
        return false;
      }
      return ops.op_crypto_timing_safe_equal(data.der, otherData.der);
    }
  }

  function keyDer(key) {
    return Buffer.from(keyData.get(key).der);
  }

  function importKey(type, value) {
    const data = keyData.get(value);
    if (data !== undefined) {
      if (type === "private") throw keyBytesTypeError("key", value);
      if (data.type !== "private") throw privateKeyTypeError(data.type);
      return new KeyObject(keyToken, "public", call(ops.op_crypto_import_key, "public", "der", "pkcs8", data.der, empty));
    }
    const options = value !== null && typeof value === "object" && "key" in value ? value : {};
    if ("key" in options && keyData.has(options.key)) return importKey(type, options.key);
    const raw = "key" in options ? options.key : value;
    if (options.format === "jwk") {
      validateObject(raw, "key.key");
      if (!["RSA", "EC", "OKP"].includes(raw.kty)) {
        throw new codes.ERR_INVALID_ARG_VALUE("key.key.kty", raw.kty);
      }
      return new KeyObject(keyToken, type, call(ops.op_crypto_import_jwk, type, raw));
    }
    const bytes = toKeyBytes(raw, "key", options.encoding);
    const format = options.format ?? (typeof raw === "string" || bytes.includes("-----BEGIN ") ? "pem" : "der");
    if (format !== "pem" && format !== "der") {
      throw new codes.ERR_INVALID_ARG_VALUE("key.format", format);
    }
    if (options.type !== undefined || options.format === "der") {
      const types = type === "public" ? ["spki", "pkcs1", "pkcs8", "sec1"] : ["pkcs1", "pkcs8", "sec1"];
      if (!types.includes(options.type)) throw new codes.ERR_INVALID_ARG_VALUE("key.type", options.type);
    }
    const passphrase = options.passphrase === undefined ? empty : toKeyBytes(options.passphrase, "key.passphrase", options.encoding);
    return new KeyObject(keyToken, type, call(ops.op_crypto_import_key, type, format, options.type ?? "", bytes, passphrase));
  }

  function createPrivateKey(key) { return importKey("private", key); }
  function createPublicKey(key) { return importKey("public", key); }

  function dhOptions(options) {
    const { group, prime, primeLength, generator } = options;
    if (group != null) {
      for (const name of ["prime", "primeLength", "generator"]) {
        if (options[name] != null) throw new codes.ERR_INCOMPATIBLE_OPTION_PAIR("group", name);
      }
      validateString(group, "options.group");
      return { group, generator: 2 };
    }
    if (generator != null) validateInt32(generator, "options.generator", 0);
    if (prime != null) {
      if (primeLength != null) throw new codes.ERR_INCOMPATIBLE_OPTION_PAIR("prime", "primeLength");
      validateBuffer(prime, "options.prime");
      return {
        prime: Array.from(new Uint8Array(prime.buffer, prime.byteOffset, prime.byteLength)),
        generator: generator ?? 2,
      };
    }
    if (primeLength == null) {
      throw new codes.ERR_MISSING_OPTION("At least one of the group, prime, or primeLength options");
    }
    validateInt32(primeLength, "options.primeLength", 0);
    return { bits: primeLength, generator: generator ?? 2 };
  }

  function generationOptions(type, options) {
    validateString(type, "type");
    if (options !== undefined) validateObject(options, "options");
    switch (type) {
      case "rsa":
      case "rsa-pss": {
        validateObject(options, "options");
        const { modulusLength: bits, publicExponent: exponent = 65537 } = options;
        validateUint32(bits, "options.modulusLength");
        validateUint32(exponent ?? 65537, "options.publicExponent");
        if (type === "rsa-pss" && ["hashAlgorithm", "mgf1HashAlgorithm", "saltLength", "hash", "mgf1Hash"].some(
          (name) => options[name] !== undefined
        )) {
          throw new codes.ERR_NOT_IMPLEMENTED("RSA-PSS key restrictions");
        }
        return { type, bits, exponent: exponent ?? 65537 };
      }
      case "dsa": {
        validateObject(options, "options");
        const { modulusLength: bits, divisorLength } = options;
        validateUint32(bits, "options.modulusLength");
        if (divisorLength != null) {
          validateInt32(divisorLength, "options.divisorLength", 0);
          throw new codes.ERR_NOT_IMPLEMENTED("DSA divisor length selection");
        }
        return { type, bits };
      }
      case "ec": {
        validateObject(options, "options");
        const { namedCurve: curve, paramEncoding } = options;
        validateString(curve, "options.namedCurve");
        if (paramEncoding != null && paramEncoding !== "named" && paramEncoding !== "explicit") {
          throw new codes.ERR_INVALID_ARG_VALUE("options.paramEncoding", paramEncoding);
        }
        return { type, curve, explicit: paramEncoding === "explicit" };
      }
      case "dh":
        validateObject(options, "options");
        return { type, ...dhOptions(options) };
      case "ed25519":
      case "ed448":
      case "x25519":
      case "x448":
        return { type };
      default:
        throw new codes.ERR_INVALID_ARG_VALUE("type", type, "must be a supported key type");
    }
  }

  function keyPairEncodings(type, options) {
    const { publicKeyEncoding, privateKeyEncoding } = options ?? {};
    return {
      publicKey: publicKeyEncoding == null
        ? null
        : parseEncoding(publicKeyEncoding, "public", type, "options.publicKeyEncoding"),
      privateKey: privateKeyEncoding == null
        ? null
        : parseEncoding(privateKeyEncoding, "private", type, "options.privateKeyEncoding"),
    };
  }

  function encodePair(keys, encodings) {
    const encode = (kind, key, encoding) => encoding === null
      ? new KeyObject(keyToken, kind, key)
      : exportKey(key, kind, encoding);
    return {
      publicKey: encode("public", keys.publicKey, encodings.publicKey),
      privateKey: encode("private", keys.privateKey, encodings.privateKey),
    };
  }

  function generateKeyPairSync(type, options) {
    const generation = generationOptions(type, options);
    const encodings = keyPairEncodings(type, options);
    return encodePair(call(ops.op_crypto_generate_key_pair, generation), encodings);
  }

  function generateKeyPair(type, options, callback) {
    if (typeof options === "function") {
      callback = options;
      options = undefined;
    }
    validateFunction(callback, "callback");
    const generation = generationOptions(type, options);
    const encodings = keyPairEncodings(type, options);
    call(ops.op_crypto_generate_key_pair_async, generation)
      .then((keys) => encodePair(keys, encodings))
      .then(
        ({ publicKey, privateKey }) => callback(null, publicKey, privateKey),
        (error) => callback(opError(error)),
      );
  }

  Object.defineProperty(generateKeyPair, Symbol.for("nodejs.util.promisify.custom"), {
    configurable: true,
    value: (type, options) => new Promise((resolve, reject) => {
      generateKeyPair(type, options, (error, publicKey, privateKey) => {
        if (error) reject(error);
        else resolve({ publicKey, privateKey });
      });
    }),
  });

  function diffieHellman(options, callback) {
    validateObject(options, "options");
    if (callback !== undefined) validateFunction(callback, "callback");

    const { privateKey, publicKey } = options;
    if (privateKey === undefined) throw new codes.ERR_INVALID_ARG_VALUE("options.privateKey", privateKey);
    if (publicKey === undefined) throw new codes.ERR_INVALID_ARG_VALUE("options.publicKey", publicKey);
    const secretKey = keyData.get(privateKey) ?? keyData.get(createPrivateKey(privateKey));
    if (secretKey.type !== "private") throw privateKeyTypeError(secretKey.type);

    const peerKey = keyData.get(publicKey);
    const peer = peerKey?.type === "public" ? peerKey : keyData.get(createPublicKey(publicKey));
    if (secretKey.kind !== peer.kind || !["dh", "ec", "x25519", "x448"].includes(secretKey.kind)) {
      throw errorWithCode(
        "ERR_CRYPTO_INCOMPATIBLE_KEY",
        `Incompatible key types for Diffie-Hellman: ${secretKey.kind} and ${peer.kind}`,
      );
    }
    if (callback === undefined) {
      return Buffer.from(call(ops.op_crypto_diffie_hellman, secretKey.der, peer.der));
    }
    call(ops.op_crypto_diffie_hellman_async, secretKey.der, peer.der).then(
      (bytes) => callback(null, Buffer.from(bytes)),
      (error) => callback(opError(error)),
    );
  }

  module.exports = {
    KeyObject,
    createPrivateKey,
    createPublicKey,
    diffieHellman,
    generateKeyPair,
    generateKeyPairSync,
    keyDer,
  };
})();
