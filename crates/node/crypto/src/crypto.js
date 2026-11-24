(() => {
  const { Buffer } = require("node:buffer");
  const { Transform } = require("node:stream");
  const { codes, opError, opErrorParts } = require("node:thinpack/internal/errors");

  const {
    KeyObject,
    createPrivateKey,
    createPublicKey,
    diffieHellman,
    generateKeyPair,
    generateKeyPairSync,
    keyDer,
  } = require("node:thinpack/internal/crypto/keypair");

  const notImplemented = (name) => new codes.ERR_NOT_IMPLEMENTED(`crypto.${name}`);

  class CryptoKey {
    constructor(properties) {
      Object.assign(this, properties);
    }
  }

  const hashNameOf = (algorithm) => {
    if (typeof algorithm === "string") return algorithm;
    const hash = algorithm?.hash;
    return typeof hash === "string" ? hash : hash?.name ?? "SHA-256";
  };

  const base64url = (value) =>
    Buffer.from(String(value).replace(/-/g, "+").replace(/_/g, "/"), "base64");

  const derLength = (length) => {
    if (length < 0x80) return [length];
    const bytes = [];
    for (let remaining = length; remaining > 0; remaining >>= 8) {
      bytes.unshift(remaining & 0xff);
    }
    return [0x80 | bytes.length, ...bytes];
  };

  const derInteger = (bytes) => {
    let start = 0;
    while (start < bytes.length - 1 && bytes[start] === 0) start += 1;
    let value = Buffer.from(bytes.subarray(start));
    if (value[0] & 0x80) value = Buffer.concat([Buffer.from([0]), value]);
    return Buffer.concat([Buffer.from([0x02, ...derLength(value.length)]), value]);
  };
  const spkiToRing = (der) => {
    const read = (offset) => {
      const tag = der[offset];
      let length = der[offset + 1];
      let header = 2;
      if (length & 0x80) {
        const count = length & 0x7f;
        length = 0;
        for (let index = 0; index < count; index += 1) {
          length = (length << 8) | der[offset + 2 + index];
        }
        header += count;
      }
      return { tag, start: offset + header, end: offset + header + length };
    };
    const outer = read(0);
    const algorithm = read(outer.start);
    const bits = read(algorithm.end);
    return der.subarray(bits.start + 1, bits.end);
  };

  const bitLength = (bytes) => {
    let index = 0;
    while (index < bytes.length - 1 && bytes[index] === 0) index += 1;
    return (bytes.length - index) * 8 - (Math.clz32(bytes[index]) - 24);
  };
  const rsaModulusBits = (pkcs1) => {
    const read = (offset) => {
      let length = pkcs1[offset + 1];
      let header = 2;
      if (length & 0x80) {
        const count = length & 0x7f;
        length = 0;
        for (let index = 0; index < count; index += 1) {
          length = (length << 8) | pkcs1[offset + 2 + index];
        }
        header += count;
      }
      return { start: offset + header, end: offset + header + length };
    };
    const sequence = read(0);
    const modulus = read(sequence.start);
    return bitLength(pkcs1.subarray(modulus.start, modulus.end));
  };

  const derRead = (bytes, offset) => {
    let length = bytes[offset + 1];
    let header = 2;
    if (length & 0x80) {
      const count = length & 0x7f;
      length = 0;
      for (let index = 0; index < count; index += 1) {
        length = (length << 8) | bytes[offset + 2 + index];
      }
      header += count;
    }
    return { start: offset + header, end: offset + header + length };
  };

  const base64urlEncode = (bytes) =>
    Buffer.from(bytes).toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  const spkiToJwk = (spki, isEc) => {
    const ringKey = spkiToRing(spki);
    if (!isEc) {
      const sequence = derRead(ringKey, 0);
      const modulus = derRead(ringKey, sequence.start);
      const exponent = derRead(ringKey, modulus.end);
      const unsigned = (bytes) => {
        let start = 0;
        while (start < bytes.length - 1 && bytes[start] === 0) start += 1;
        return bytes.subarray(start);
      };
      return {
        kty: "RSA",
        n: base64urlEncode(unsigned(ringKey.subarray(modulus.start, modulus.end))),
        e: base64urlEncode(unsigned(ringKey.subarray(exponent.start, exponent.end))),
      };
    }
    const size = (ringKey.length - 1) / 2;
    return {
      kty: "EC",
      crv: size === 48 ? "P-384" : "P-256",
      x: base64urlEncode(ringKey.subarray(1, 1 + size)),
      y: base64urlEncode(ringKey.subarray(1 + size)),
    };
  };

  const jwkToRing = (jwk) => {
    const decode = (value) => Buffer.from(base64url(value));
    if (jwk.kty === "RSA") {
      const body = Buffer.concat([derInteger(decode(jwk.n)), derInteger(decode(jwk.e))]);
      return { kind: "rsa", key: Buffer.concat([Buffer.from([0x30, ...derLength(body.length)]), body]) };
    }
    if (jwk.kty === "EC" && (jwk.crv === "P-256" || jwk.crv === "P-384")) {
      return {
        kind: jwk.crv === "P-384" ? "ec384" : "ec",
        key: Buffer.concat([Buffer.from([0x04]), decode(jwk.x), decode(jwk.y)]),
      };
    }
    throw notImplemented(`crypto.subtle.importKey(jwk.kty ?? "JWK")`);
  };

  const verifyAlgorithm = (algorithm, key) => {
    const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
    const digest = normalizeAlgorithm(hashNameOf(algorithm));
    if (key._kind === "rsa" && name !== "ECDSA") {
      return `${name === "RSA-PSS" ? "rsa-pss" : "rsa"}-${digest}`;
    }
    if (key._kind === "ec") {
      return `ecdsa-p256-${digest}`;
    }
    if (key._kind === "ec384") {
      return `ecdsa-p384-${digest}`;
    }
    return undefined;
  };

  const toBytes = (data) => {
    if (ArrayBuffer.isView(data)) {
      return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    }
    return new Uint8Array(data);
  };

  const normalizeAlgorithm = (name) => {
    if (name === undefined) return undefined;
    return String(name).replace(/-/g, "").toLowerCase();
  };

  const decode = (data, inputEncoding) => {
    if (typeof data === "string") {
      return Buffer.from(data, inputEncoding || "utf8");
    }
    if (ArrayBuffer.isView(data)) {
      return Buffer.from(data.buffer, data.byteOffset, data.byteLength);
    }
    return Buffer.from(data);
  };

  const output = (bytes, encoding) =>
    encoding === undefined || encoding === "buffer"
      ? Buffer.from(bytes)
      : Buffer.from(bytes).toString(encoding);

  class Hash {
    #algorithm;
    #chunks = [];
    #digested = false;

    constructor(algorithm) {
      this.#algorithm = String(algorithm);
    }

    update(data, inputEncoding) {
      if (this.#digested) throw new Error("Digest already called");
      this.#chunks.push(decode(data, inputEncoding));
      return this;
    }

    digest(encoding) {
      if (this.#digested) throw new Error("Digest already called");
      this.#digested = true;
      const data = Buffer.concat(this.#chunks);
      const bytes = Deno.core.ops.op_crypto_hash(this.#algorithm, data);
      return output(bytes, encoding);
    }

    copy() {
      const copy = new Hash(this.#algorithm);
      copy.update(Buffer.concat(this.#chunks));
      return copy;
    }
  }

  class Hmac {
    #algorithm;
    #key;
    #chunks = [];
    #digested = false;

    constructor(algorithm, key) {
      this.#algorithm = String(algorithm);
      this.#key = Buffer.from(key);
    }

    update(data, inputEncoding) {
      if (this.#digested) throw new Error("Digest already called");
      this.#chunks.push(decode(data, inputEncoding));
      return this;
    }

    digest(encoding) {
      if (this.#digested) throw new Error("Digest already called");
      this.#digested = true;
      const bytes = Deno.core.ops.op_crypto_hmac(
        this.#algorithm,
        this.#key,
        Buffer.concat(this.#chunks),
      );
      return output(bytes, encoding);
    }
  }

  const createHash = (algorithm) => new Hash(algorithm);

  // Node's one-shot hash. undici's WebSocket authenticates the handshake with it.
  const hash = (algorithm, data, outputEncoding) => {
    const hasher = createHash(algorithm).update(data);
    return outputEncoding === undefined ? hasher.digest() : hasher.digest(outputEncoding);
  };
  const createHmac = (algorithm, key) => new Hmac(algorithm, key);
  const getHashes = () => ["md5", "sha1", "sha256", "sha384", "sha512"];

  const randomBytes = (size, callback) => {
    if (typeof callback === "function") {
      queueMicrotask(() => {
        try {
          callback(null, Buffer.from(Deno.core.ops.op_crypto_random(size)));
        } catch (error) {
          callback(opError(error));
        }
      });
      return undefined;
    }
    return Buffer.from(Deno.core.ops.op_crypto_random(size));
  };

  const randomFillSync = (buffer, offset = 0, size = buffer.byteLength - offset) => {
    const bytes = Deno.core.ops.op_crypto_random(size);
    buffer.set(bytes, offset);
    return buffer;
  };

  const randomUUID = () => {
    const bytes = Deno.core.ops.op_crypto_random(16);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = Buffer.from(bytes).toString("hex");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  };

  const timingSafeEqual = (left, right) => {
    if (left.byteLength !== right.byteLength) {
      throw new codes.ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH();
    }
    return Deno.core.ops.op_crypto_timing_safe_equal(left, right);
  };

  const cipherSpec = (algorithm) => {
    const match = /^aes-(128|192|256)-(gcm|cbc|ctr)$/.exec(String(algorithm).toLowerCase());
    if (match === null) {
      const error = new Error(`Unknown cipher: ${algorithm}`);
      error.code = "ERR_CRYPTO_UNKNOWN_CIPHER";
      throw error;
    }
    return { bits: Number(match[1]), mode: match[2] };
  };

  const cipherFailure = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };

  const checkCipherParams = (spec, key, iv) => {
    if (key.length !== spec.bits / 8) {
      throw cipherFailure("ERR_CRYPTO_INVALID_KEYLEN",
        `Invalid key length. Received ${key.length}, expected ${spec.bits / 8}`);
    }
    const expectedIv = spec.mode === "gcm" ? 12 : 16;
    if (iv.length !== expectedIv) {
      throw cipherFailure("ERR_CRYPTO_INVALID_IVLEN",
        `Invalid initialization vector length. Received ${iv.length}, expected ${expectedIv}`);
    }
  };

  const CIPHERS = [];
  for (const bits of [128, 192, 256]) {
    for (const mode of ["gcm", "cbc", "ctr"]) CIPHERS.push(`aes-${bits}-${mode}`);
  }

  // Deviation from Node: Cipheriv/Decipheriv buffer every update() and emit
  // all output (plus the GCM tag) at final(), where the one-shot
  // op_crypto_cipher runs over the accumulated data; the ops layer exposes
  // no incremental cipher state for true streaming.
  class Cipheriv extends Transform {
    #spec;
    #key;
    #iv;
    #aad = [];
    #chunks = [];
    #tag = null;
    #finalized = false;
    #autoPadding = true;

    constructor(algorithm, key, iv, options = {}) {
      super();
      this.#spec = cipherSpec(algorithm);
      this.#key = decode(key);
      this.#iv = decode(iv);
      checkCipherParams(this.#spec, this.#key, this.#iv);
      if (options?.authTagLength !== undefined && options.authTagLength !== 16) {
        throw notImplemented("crypto.createCipheriv with authTagLength != 16");
      }
    }

    _transform(chunk, encoding, callback) {
      this.#chunks.push(Buffer.from(chunk));
      callback();
    }

    _flush(callback) {
      try {
        callback(null, this.final());
      } catch (error) {
        callback(error);
      }
    }

    update(data, inputEncoding) {
      this.#chunks.push(decode(data, inputEncoding));
      return Buffer.alloc(0);
    }

    setAAD(data) {
      this.#aad.push(decode(data));
      return this;
    }

    setAutoPadding(value = true) {
      this.#autoPadding = Boolean(value);
      return this;
    }

    final(outputEncoding) {
      if (this.#finalized) throw new Error("Cipher already finalized");
      this.#finalized = true;
      const result = Buffer.from(Deno.core.ops.op_crypto_cipher(
        `aes-${this.#spec.bits}-${this.#spec.mode}`,
        this.#key,
        this.#iv,
        Buffer.concat(this.#aad),
        Buffer.concat(this.#chunks),
        1,
        this.#autoPadding ? 1 : 0,
      ));
      let ciphertext = result;
      if (this.#spec.mode === "gcm") {
        this.#tag = result.subarray(result.length - 16);
        ciphertext = result.subarray(0, result.length - 16);
      }
      return output(ciphertext, outputEncoding);
    }

    getAuthTag() {
      if (this.#tag === null) throw new Error("Auth tag is not available");
      return Buffer.from(this.#tag);
    }
  }

  class Decipheriv extends Transform {
    #spec;
    #key;
    #iv;
    #aad = [];
    #chunks = [];
    #tag = null;
    #finalized = false;
    #autoPadding = true;

    constructor(algorithm, key, iv, options = {}) {
      super();
      this.#spec = cipherSpec(algorithm);
      this.#key = decode(key);
      this.#iv = decode(iv);
      checkCipherParams(this.#spec, this.#key, this.#iv);
      if (options?.authTagLength !== undefined && options.authTagLength !== 16) {
        throw notImplemented("crypto.createDecipheriv with authTagLength != 16");
      }
    }

    _transform(chunk, encoding, callback) {
      this.#chunks.push(Buffer.from(chunk));
      callback();
    }

    _flush(callback) {
      try {
        callback(null, this.final());
      } catch (error) {
        callback(error);
      }
    }

    update(data, inputEncoding) {
      this.#chunks.push(decode(data, inputEncoding));
      return Buffer.alloc(0);
    }

    setAAD(data) {
      this.#aad.push(decode(data));
      return this;
    }

    setAuthTag(tag) {
      this.#tag = decode(tag);
      return this;
    }

    setAutoPadding(value = true) {
      this.#autoPadding = Boolean(value);
      return this;
    }

    final(outputEncoding) {
      if (this.#finalized) throw new Error("Decipher already finalized");
      this.#finalized = true;
      let data = Buffer.concat(this.#chunks);
      if (this.#spec.mode === "gcm") {
        if (this.#tag === null) {
          throw cipherFailure("ERR_CRYPTO_INVALID_AUTH_TAG", "Auth tag is required");
        }
        data = Buffer.concat([data, this.#tag]);
      }
      const result = Buffer.from(Deno.core.ops.op_crypto_cipher(
        `aes-${this.#spec.bits}-${this.#spec.mode}`,
        this.#key,
        this.#iv,
        Buffer.concat(this.#aad),
        data,
        0,
        this.#autoPadding ? 1 : 0,
      ));
      return output(result, outputEncoding);
    }
  }

  const toDer = (key) => {
    if (key instanceof KeyObject) return keyDer(key);
    if (typeof key !== "string") return Buffer.from(key);
    const match = /-----BEGIN [^-]+-----([\s\S]*?)-----END [^-]+-----/.exec(key);
    if (match === null) return Buffer.from(key);
    return Buffer.from(match[1].replace(/\s+/g, ""), "base64");
  };

  const readDerElement = (der, offset) => {
    let length = der[offset + 1];
    let header = 2;
    if (length & 0x80) {
      const count = length & 0x7f;
      length = 0;
      for (let index = 0; index < count; index += 1) {
        length = (length << 8) | der[offset + 2 + index];
      }
      header += count;
    }
    return { tag: der[offset], start: offset + header, end: offset + header + length };
  };
  const toVerifyKey = (key) => {
    const der = toDer(key);
    if (der[0] !== 0x30) return der;
    const outer = readDerElement(der, 0);
    const first = readDerElement(der, outer.start);
    return first.tag === 0x30 ? spkiToRing(der) : der;
  };

  const ecdsaRawToDer = (raw) => {
    const half = raw.length / 2;
    const r = derInteger(raw.subarray(0, half));
    const s = derInteger(raw.subarray(half));
    const body = Buffer.concat([r, s]);
    return Buffer.concat([Buffer.from([0x30, ...derLength(body.length)]), body]);
  };

  const ecdsaDerToRaw = (der, size) => {
    const sequence = readDerElement(der, 0);
    const r = readDerElement(der, sequence.start);
    const s = readDerElement(der, r.end);
    const pad = (value, start, end) => {
      let begin = start;
      while (begin < end - 1 && der[begin] === 0) begin += 1;
      const bytes = der.subarray(begin, end);
      if (bytes.length > size) return null;
      return Buffer.concat([Buffer.alloc(size - bytes.length), bytes]);
    };
    const left = pad(der, r.start, r.end);
    const right = pad(der, s.start, s.end);
    return left === null || right === null ? null : Buffer.concat([left, right]);
  };

  const opFailure = (error) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    return cipherFailure(code, detail);
  };
  const signAlgorithms = (algorithm) => {
    const name = String(algorithm).toUpperCase().replace(/_/g, "-");
    if (name.startsWith("RSA-PSS")) {
      throw notImplemented('crypto.createSign("RSA-PSS")');
    }
    if (name.startsWith("RSA-")) {
      const digest = name.slice(4).toLowerCase();
      if (!["sha256", "sha384", "sha512"].includes(digest)) {
        throw notImplemented(`crypto.createSign("${algorithm}")`);
      }
      return [`rsa-${digest}`];
    }
    const match = /^SHA-?(256|384)$/.exec(name);
    if (match === null) {
      throw notImplemented(`crypto.createSign("${algorithm}")`);
    }
    return [`ecdsa-p256-sha${match[1]}`, `ecdsa-p384-sha${match[1]}`];
  };

  class Sign {
    #algorithms;
    #chunks = [];

    constructor(algorithm) {
      this.#algorithms = signAlgorithms(algorithm);
    }

    update(data, inputEncoding) {
      this.#chunks.push(decode(data, inputEncoding));
      return this;
    }

    sign(key, outputEncoding) {
      const der = toDer(key);
      const data = Buffer.concat(this.#chunks);
      let failure;
      for (const algorithm of this.#algorithms) {
        try {
          let signature = Buffer.from(Deno.core.ops.op_crypto_sign(algorithm, der, data));
          if (algorithm.startsWith("ecdsa")) signature = ecdsaRawToDer(signature);
          return output(signature, outputEncoding);
        } catch (error) {
          failure = error;
        }
      }
      throw opFailure(failure);
    }
  }

  class Verify {
    #algorithms;
    #chunks = [];

    constructor(algorithm) {
      this.#algorithms = signAlgorithms(algorithm);
    }

    update(data, inputEncoding) {
      this.#chunks.push(decode(data, inputEncoding));
      return this;
    }

    verify(key, signature, signatureEncoding) {
      const der = toVerifyKey(key);
      const data = Buffer.concat(this.#chunks);
      const bytes = typeof signature === "string" && signatureEncoding !== undefined
        ? Buffer.from(signature, signatureEncoding)
        : decode(signature, signatureEncoding);
      const ecCurve = der[0] === 0x04 && der.length === 97 ? "p384" : "p256";
      for (const algorithm of this.#algorithms) {
        if (algorithm.startsWith("ecdsa") && !algorithm.includes(ecCurve)) continue;
        const candidates = [bytes];
        if (algorithm.startsWith("ecdsa") && bytes[0] === 0x30) {
          const size = ecCurve === "p384" ? 48 : 32;
          const raw = ecdsaDerToRaw(bytes, size);
          if (raw !== null) candidates.push(raw);
        }
        for (const candidate of candidates) {
          if (Deno.core.ops.op_crypto_verify(algorithm, der, candidate, data)) return true;
        }
      }
      return false;
    }
  }

  const pbkdf2Sync = (password, salt, iterations, keylen, digest) => {
    const bytes = Deno.core.ops.op_crypto_pbkdf2(
      String(digest),
      decode(password),
      decode(salt),
      iterations,
      keylen,
    );
    return Buffer.from(bytes);
  };

  // The async KDF forms keep their callback contract: argument errors reach
  // the callback rather than the caller, while the derivation itself runs in
  // a blocking task off the event loop.
  const kdfAsync = (start, callback) => {
    queueMicrotask(() => {
      let task;
      try {
        task = start();
      } catch (error) {
        callback(opFailure(error));
        return;
      }
      task.then(
        (bytes) => callback(null, Buffer.from(bytes)),
        (error) => callback(opFailure(error)),
      );
    });
  };

  const pbkdf2 = (password, salt, iterations, keylen, digest, callback) => {
    if (typeof digest === "function") {
      callback = digest;
      digest = "sha1";
    }
    kdfAsync(() => Deno.core.ops.op_crypto_pbkdf2_async(
      String(digest), decode(password), decode(salt), iterations, keylen,
    ), callback);
  };

  const scryptParams = (options = {}) => {
    const N = options.N ?? 16384;
    const r = options.r ?? 8;
    const p = options.p ?? 1;
    const logN = Math.log2(N);
    if (!Number.isInteger(logN) || logN < 1) {
      throw cipherFailure("ERR_CRYPTO_INVALID_SCRYPT_PARAMS", "Invalid scrypt N");
    }
    return [logN, r, p];
  };

  const scryptSync = (password, salt, keylen, options = {}) => {
    const [logN, r, p] = scryptParams(options);
    const bytes = Deno.core.ops.op_crypto_scrypt(
      decode(password),
      decode(salt),
      logN,
      r,
      p,
      keylen,
    );
    return Buffer.from(bytes);
  };

  const scrypt = (password, salt, keylen, options, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    kdfAsync(() => {
      const [logN, r, p] = scryptParams(options);
      return Deno.core.ops.op_crypto_scrypt_async(
        decode(password), decode(salt), logN, r, p, keylen,
      );
    }, callback);
  };

  const hkdfSync = (digest, ikm, salt, info, keylen) => {
    const bytes = Deno.core.ops.op_crypto_hkdf(
      String(digest),
      decode(ikm),
      decode(salt),
      decode(info),
      keylen,
    );
    return Buffer.from(bytes);
  };

  const hkdf = (digest, ikm, salt, info, keylen, callback) => {
    kdfAsync(() => Deno.core.ops.op_crypto_hkdf_async(
      String(digest), decode(ikm), decode(salt), decode(info), keylen,
    ), callback);
  };

  const randomFill = (buffer, offset, size, callback) => {
    if (typeof offset === "function") {
      callback = offset;
      offset = 0;
      size = buffer.byteLength;
    } else if (typeof size === "function") {
      callback = size;
      size = buffer.byteLength - offset;
    }
    queueMicrotask(() => {
      try {
        randomFillSync(buffer, offset, size);
        callback(null, buffer);
      } catch (error) {
        callback(error);
      }
    });
  };

  const randomIntSync = (min, max) => {
    if (max === undefined) {
      max = min;
      min = 0;
    }
    if (!Number.isSafeInteger(min) || !Number.isSafeInteger(max) || min >= max) {
      throw cipherFailure("ERR_OUT_OF_RANGE", "randomInt range is invalid");
    }
    const range = max - min;
    if (range > 2 ** 48) {
      throw cipherFailure("ERR_OUT_OF_RANGE", "randomInt range is too large");
    }
    const limit = Math.floor(2 ** 48 / range) * range;
    for (;;) {
      const bytes = randomFillSync(Buffer.alloc(6));
      let value = 0;
      for (const byte of bytes) value = value * 256 + byte;
      if (value < limit) return min + (value % range);
    }
  };

  const randomInt = (min, max, callback) => {
    if (typeof max === "function") {
      callback = max;
      max = min;
      min = 0;
    }
    if (typeof callback === "function") {
      queueMicrotask(() => {
        try {
          callback(null, randomIntSync(min, max));
        } catch (error) {
          callback(error);
        }
      });
      return undefined;
    }
    return randomIntSync(min, max);
  };

  class X509Certificate {
    constructor(buffer) {
      if (buffer === undefined || buffer === null) {
        throw new codes.ERR_INVALID_ARG_TYPE("buffer", ["Buffer", "string"], buffer);
      }
      this.raw = decode(buffer);
    }

    toString() {
      const body = this.raw.toString("base64").match(/.{1,64}/g).join("\n");
      return `-----BEGIN CERTIFICATE-----\n${body}\n-----END CERTIFICATE-----\n`;
    }

    get subject() { throw notImplemented("X509Certificate.subject"); }
    get issuer() { throw notImplemented("X509Certificate.issuer"); }
    get validFrom() { throw notImplemented("X509Certificate.validFrom"); }
    get validTo() { throw notImplemented("X509Certificate.validTo"); }
    get serialNumber() { throw notImplemented("X509Certificate.serialNumber"); }
    get fingerprint() { throw notImplemented("X509Certificate.fingerprint"); }
    get fingerprint256() { throw notImplemented("X509Certificate.fingerprint256"); }
    get publicKey() { throw notImplemented("X509Certificate.publicKey"); }

    checkHost() { throw notImplemented("X509Certificate.checkHost"); }
    checkIP() { throw notImplemented("X509Certificate.checkIP"); }
    checkEmail() { throw notImplemented("X509Certificate.checkEmail"); }
    verify() { throw notImplemented("X509Certificate.verify"); }
    toLegacyObject() { throw notImplemented("X509Certificate.toLegacyObject"); }
  }

  module.exports = {
    CryptoKey,
    Hash,
    Hmac,
    createHash,
    createHmac,
    hash,
    getHashes,
    randomBytes,
    randomFill,
    KeyObject,
    X509Certificate,
    createPrivateKey,
    createPublicKey,
    diffieHellman,
    generateKeyPair,
    generateKeyPairSync,
    getRandomValues: (array) => {
      randomFillSync(new Uint8Array(array.buffer, array.byteOffset, array.byteLength));
      return array;
    },
    randomFillSync,
    randomInt,
    hkdf,
    hkdfSync,
    randomUUID,
    timingSafeEqual,
    getCurves: () => ["prime256v1", "secp384r1", "secp521r1", "secp256k1"],
    getCiphers: () => CIPHERS.slice(),
    createCipheriv: (algorithm, key, iv, options) => new Cipheriv(algorithm, key, iv, options),
    createDecipheriv: (algorithm, key, iv, options) => new Decipheriv(algorithm, key, iv, options),
    Sign,
    Verify,
    createSign: (algorithm) => new Sign(algorithm),
    createVerify: (algorithm) => new Verify(algorithm),
    sign: (algorithm, data, key) => new Sign(algorithm).update(data).sign(key),
    verify: (algorithm, data, key, signature) =>
      new Verify(algorithm).update(data).verify(key, signature),
    pbkdf2,
    pbkdf2Sync,
    scrypt,
    scryptSync,
    constants: {},
    webcrypto: {
      CryptoKey,
      randomUUID,
      getRandomValues(array) {
        randomFillSync(new Uint8Array(array.buffer, array.byteOffset, array.byteLength));
        return array;
      },
      subtle: {
        async digest(algorithm, data) {
          const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
          const bytes = Deno.core.ops.op_crypto_hash(normalizeAlgorithm(name), toBytes(data));
          return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
        },

        async importKey(format, keyData, algorithm, extractable, usages) {
          if (format === "raw") {
            const rawName = typeof algorithm === "string" ? algorithm : algorithm?.name;
            const normalized = normalizeAlgorithm(rawName);
            if (normalized === "aesgcm" || normalized === "aescbc") {
              const key = toBytes(keyData);
              if (![16, 24, 32].includes(key.length)) {
                throw cipherFailure("ERR_CRYPTO_INVALID_KEYLEN",
                  `Invalid AES key length: ${key.length}`);
              }
              return new CryptoKey({
                type: "secret",
                extractable: Boolean(extractable),
                algorithm: { name: rawName, length: key.length * 8 },
                usages: Array.from(usages ?? []),
                _kind: "aes",
                _key: key,
              });
            }
            return new CryptoKey({
              type: "secret",
              extractable: Boolean(extractable),
              algorithm: { name: typeof algorithm === "string" ? algorithm : algorithm?.name },
              usages: Array.from(usages ?? []),
              _key: toBytes(keyData),
            });
          }
          const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
          const hashName = hashNameOf(algorithm);
          if (format === "spki") {
            const spki = toBytes(keyData);
            const ringKey = spkiToRing(spki);
            const isEc = name === "ECDSA" || name === "ECDSA_P384";
            const curve = isEc && (algorithm?.namedCurve === "P-384" || name === "ECDSA_P384")
              ? "P-384"
              : "P-256";
            const kind = isEc ? (curve === "P-384" ? "ec384" : "ec") : "rsa";
            const algorithm_ = { name: isEc ? "ECDSA" : name, hash: { name: hashName } };
            if (isEc) algorithm_.namedCurve = curve;
            else algorithm_.modulusLength = rsaModulusBits(ringKey);
            return new CryptoKey({
              type: "public",
              extractable: Boolean(extractable),
              algorithm: algorithm_,
              usages: Array.from(usages ?? []),
              _kind: kind,
              _key: Buffer.from(ringKey),
              _spki: Buffer.from(spki),
              _jwk: spkiToJwk(spki, isEc),
            });
          }
          if (format === "pkcs8") {
            const isEc = name === "ECDSA" || name === "ECDSA_P384";
            const curve = isEc && (algorithm?.namedCurve === "P-384" || name === "ECDSA_P384")
              ? "P-384"
              : "P-256";
            const kind = isEc ? (curve === "P-384" ? "ec384" : "ec") : "rsa";
            const algorithm_ = { name: isEc ? "ECDSA" : name, hash: { name: hashName } };
            if (isEc) algorithm_.namedCurve = curve;
            return new CryptoKey({
              type: "private",
              extractable: Boolean(extractable),
              algorithm: algorithm_,
              usages: Array.from(usages ?? []),
              _kind: kind,
              _pkcs8: toBytes(keyData),
              _key: toBytes(keyData),
            });
          }
          if (format === "jwk") {
            const { kind, key } = jwkToRing(keyData);
            const isEc = kind === "ec" || kind === "ec384";
            const algorithm_ = {
              name: isEc ? "ECDSA" : (name === "RSA-PSS" ? "RSA-PSS" : "RSASSA-PKCS1-v1_5"),
              hash: { name: hashName },
            };
            if (isEc) algorithm_.namedCurve = kind === "ec384" ? "P-384" : "P-256";
            else algorithm_.modulusLength = rsaModulusBits(key);
            return new CryptoKey({
              type: "public",
              extractable: Boolean(extractable),
              algorithm: algorithm_,
              usages: Array.from(usages ?? []),
              _kind: kind,
              _key: key,
              _jwk: { ...keyData },
            });
          }
          throw notImplemented(`crypto.subtle.importKey(${format})`);
        },

        async generateKey(algorithm, extractable, usages) {
          const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
          const normalized = normalizeAlgorithm(name);
          if (normalized === "aesgcm" || normalized === "aescbc") {
            const length = algorithm?.length ?? 256;
            if (![128, 192, 256].includes(length)) {
              throw cipherFailure("ERR_CRYPTO_INVALID_KEYLEN", `Invalid AES key length: ${length}`);
            }
            return new CryptoKey({
              type: "secret",
              extractable: Boolean(extractable),
              algorithm: { name, length },
              usages: Array.from(usages ?? []),
              _kind: "aes",
              _key: Buffer.from(Deno.core.ops.op_crypto_random(length / 8)),
            });
          }
          if (normalized !== "hmac") {
            throw notImplemented(`crypto.subtle.generateKey(${name})`);
          }
          const length = algorithm?.length ?? 256;
          const key = Buffer.from(Deno.core.ops.op_crypto_random(Math.ceil(length / 8)));
          return new CryptoKey({
            type: "secret",
            extractable: Boolean(extractable),
            algorithm: { name: "HMAC", length },
            usages: Array.from(usages ?? []),
            _key: key,
          });
        },

        async exportKey(format, key) {
          if (key.type === "secret" && format === "raw") {
            return key._key.buffer.slice(key._key.byteOffset, key._key.byteOffset + key._key.byteLength);
          }
          if (key.type !== "public") {
            throw notImplemented(`crypto.subtle.exportKey(${format})`);
          }
          if (format === "pkcs8" && key._pkcs8) {
            return key._pkcs8.buffer.slice(key._pkcs8.byteOffset, key._pkcs8.byteOffset + key._pkcs8.byteLength);
          }
          if (format === "spki" && key._spki) {
            return key._spki.buffer.slice(key._spki.byteOffset, key._spki.byteOffset + key._spki.byteLength);
          }
          if (format === "jwk" && key._jwk) {
            return { ...key._jwk };
          }
          throw notImplemented(`crypto.subtle.exportKey(${format})`);
        },

        async sign(algorithm, key, data) {
          const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
          if (key.type === "private") {
            const digest = normalizeAlgorithm(hashNameOf(algorithm));
            const op = key._kind === "rsa"
              ? `rsa-${digest}`
              : key._kind === "ec384"
                ? `ecdsa-p384-${digest}`
                : `ecdsa-p256-${digest}`;
            const signature = Deno.core.ops.op_crypto_sign(op, key._pkcs8, toBytes(data));
            return signature.buffer.slice(signature.byteOffset, signature.byteOffset + signature.byteLength);
          }
          if (key.type === "public") {
            throw notImplemented(`crypto.subtle.sign(${name})`);
          }
          if (normalizeAlgorithm(name) !== "hmac") {
            throw notImplemented(`crypto.subtle.sign(${name})`);
          }
          const signature = Deno.core.ops.op_crypto_hmac(
            normalizeAlgorithm(hashNameOf(algorithm)),
            key._key,
            toBytes(data),
          );
          return signature.buffer.slice(signature.byteOffset, signature.byteOffset + signature.byteLength);
        },

        async verify(algorithm, key, signature, data) {
          const name = typeof algorithm === "string" ? algorithm : algorithm?.name;
          if (key.type === "public") {
            const op = verifyAlgorithm(algorithm, key);
            if (op === undefined) {
              throw notImplemented(`crypto.subtle.verify(${name})`);
            }
            return Deno.core.ops.op_crypto_verify(
              op,
              key._key,
              toBytes(signature),
              toBytes(data),
            );
          }
          if (normalizeAlgorithm(name) !== "hmac") {
            throw notImplemented(`crypto.subtle.verify(${name})`);
          }
          const hash = typeof algorithm === "string" ? "SHA-256" : algorithm.hash?.name ?? "SHA-256";
          const expected = Deno.core.ops.op_crypto_hmac(
            normalizeAlgorithm(hashNameOf(algorithm)),
            key._key,
            toBytes(data),
          );
          return timingSafeEqual(expected, new Uint8Array(signature));
        },

        async encrypt(algorithm, key, data) {
          const name = normalizeAlgorithm(
            typeof algorithm === "string" ? algorithm : algorithm?.name,
          );
          if (key._kind !== "aes") {
            throw notImplemented(`crypto.subtle.encrypt(${name})`);
          }
          const bits = key._key.length * 8;
          const iv = toBytes(algorithm.iv);
          if (name === "aesgcm") {
            if ((algorithm.tagLength ?? 128) !== 128) {
              throw notImplemented("crypto.subtle.encrypt(AES-GCM tagLength != 128)");
            }
            const aad = algorithm.additionalData ? toBytes(algorithm.additionalData) : new Uint8Array();
            const out = Deno.core.ops.op_crypto_cipher(
              `aes-${bits}-gcm`, key._key, iv, aad, toBytes(data), 1, 0,
            );
            return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
          }
          if (name === "aescbc") {
            const out = Deno.core.ops.op_crypto_cipher(
              `aes-${bits}-cbc`, key._key, iv, new Uint8Array(), toBytes(data), 1, 1,
            );
            return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
          }
          throw notImplemented(`crypto.subtle.encrypt(${name})`);
        },

        async decrypt(algorithm, key, data) {
          const name = normalizeAlgorithm(
            typeof algorithm === "string" ? algorithm : algorithm?.name,
          );
          if (key._kind !== "aes") {
            throw notImplemented(`crypto.subtle.decrypt(${name})`);
          }
          const bits = key._key.length * 8;
          const iv = toBytes(algorithm.iv);
          try {
            if (name === "aesgcm") {
              if ((algorithm.tagLength ?? 128) !== 128) {
                throw notImplemented("crypto.subtle.decrypt(AES-GCM tagLength != 128)");
              }
              const aad = algorithm.additionalData ? toBytes(algorithm.additionalData) : new Uint8Array();
              const out = Deno.core.ops.op_crypto_cipher(
                `aes-${bits}-gcm`, key._key, iv, aad, toBytes(data), 0, 0,
              );
              return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
            }
            if (name === "aescbc") {
              const out = Deno.core.ops.op_crypto_cipher(
                `aes-${bits}-cbc`, key._key, iv, new Uint8Array(), toBytes(data), 0, 1,
              );
              return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
            }
          } catch (error) {
            throw opFailure(error);
          }
          throw notImplemented(`crypto.subtle.decrypt(${name})`);
        },
      },
    },
    getCrypto: () => {
      throw notImplemented("getCrypto");
    },
  };
})();
