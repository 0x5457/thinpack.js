use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use thinpack_pack::{Compression, FileRecord, Manifest, Template};

// A packed test normally finishes in under a second, but a debug-build RSA key generation
// can spend several seconds searching for primes.
const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const STARTUP_LIMIT: Duration = Duration::from_secs(1);
const STARTUP_MARKER: &str = "startup complete";

struct PackedTest {
    name: &'static str,
    directory: &'static str,
    entry: &'static str,
    files: &'static [(&'static str, &'static str)],
    capabilities: &'static [&'static str],
    /// An expected failure must exit non-zero and print every listed fragment.
    error: Option<&'static [&'static str]>,
}

const LARGE_ESM_FILES: &[(&str, &str)] = &[
    (
        "/app/main.mjs",
        "import { total } from './esm/registry.mjs';\nif (total !== 120) throw new Error(`unexpected total: ${total}`);\n",
    ),
    (
        "/app/main-error.mjs",
        "import { total } from './esm/registry.mjs';\nimport './esm/missing-export.mjs';\nif (total !== 120) throw new Error(`unexpected total: ${total}`);\n",
    ),
    (
        "/app/esm/registry.mjs",
        "import { groupA } from './groups/a.mjs';\nimport { groupB } from './groups/b.mjs';\nimport { groupC } from './groups/c.mjs';\nimport { groupD } from './groups/d.mjs';\nexport const total = groupA + groupB + groupC + groupD;\n",
    ),
    (
        "/app/esm/groups/a.mjs",
        "import { part00 } from '../parts/part-00.mjs';\nimport { part01 } from '../parts/part-01.mjs';\nimport { part02 } from '../parts/part-02.mjs';\nimport { part03 } from '../parts/part-03.mjs';\nexport const groupA = part00 + part01 + part02 + part03;\n",
    ),
    (
        "/app/esm/groups/b.mjs",
        "import { part04 } from '../parts/part-04.mjs';\nimport { part05 } from '../parts/part-05.mjs';\nimport { part06 } from '../parts/part-06.mjs';\nimport { part07 } from '../parts/part-07.mjs';\nexport const groupB = part04 + part05 + part06 + part07;\n",
    ),
    (
        "/app/esm/groups/c.mjs",
        "import { part08 } from '../parts/part-08.mjs';\nimport { part09 } from '../parts/part-09.mjs';\nimport { part10 } from '../parts/part-10.mjs';\nimport { part11 } from '../parts/part-11.mjs';\nexport const groupC = part08 + part09 + part10 + part11;\n",
    ),
    (
        "/app/esm/groups/d.mjs",
        "import { part12 } from '../parts/part-12.mjs';\nimport { part13 } from '../parts/part-13.mjs';\nimport { part14 } from '../parts/part-14.mjs';\nimport { part15 } from '../parts/part-15.mjs';\nexport const groupD = part12 + part13 + part14 + part15;\n",
    ),
    ("/app/esm/parts/part-00.mjs", "export const part00 = 0;\n"),
    ("/app/esm/parts/part-01.mjs", "export const part01 = 1;\n"),
    ("/app/esm/parts/part-02.mjs", "export const part02 = 2;\n"),
    ("/app/esm/parts/part-03.mjs", "export const part03 = 3;\n"),
    ("/app/esm/parts/part-04.mjs", "export const part04 = 4;\n"),
    ("/app/esm/parts/part-05.mjs", "export const part05 = 5;\n"),
    ("/app/esm/parts/part-06.mjs", "export const part06 = 6;\n"),
    ("/app/esm/parts/part-07.mjs", "export const part07 = 7;\n"),
    ("/app/esm/parts/part-08.mjs", "export const part08 = 8;\n"),
    ("/app/esm/parts/part-09.mjs", "export const part09 = 9;\n"),
    ("/app/esm/parts/part-10.mjs", "export const part10 = 10;\n"),
    ("/app/esm/parts/part-11.mjs", "export const part11 = 11;\n"),
    ("/app/esm/parts/part-12.mjs", "export const part12 = 12;\n"),
    ("/app/esm/parts/part-13.mjs", "export const part13 = 13;\n"),
    ("/app/esm/parts/part-14.mjs", "export const part14 = 14;\n"),
    ("/app/esm/parts/part-15.mjs", "export const part15 = 15;\n"),
    (
        "/app/esm/missing-export.mjs",
        "// The error variant reaches the full graph before this deliberate link error.\nimport { missingPart } from './parts/part-15.mjs';\nexport const value = missingPart;\n",
    ),
];

const TESTS: &[PackedTest] = &[
    PackedTest {
        name: "packed::stat-mode-birthtime",
        directory: "stat-mode-birthtime",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
const fs = require('node:fs');

const directoryStats = fs.statSync('/app');
const fileStats = fs.statSync('/app/main.js');
assert.deepEqual(
  {
    directoryMode: directoryStats.mode,
    birthtimeValid:
      fileStats.birthtime instanceof Date &&
      !Number.isNaN(fileStats.birthtime.getTime()),
  },
  { directoryMode: 0o40755, birthtimeValid: true },
);
",
        )],
        capabilities: &["assert", "fs"],
        error: None,
    },
    PackedTest {
        name: "packed::error-paths",
        directory: "error-paths",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                r"
const assert = require('node:assert/strict');

// Resolver errors name the path the payload records, not a host rendering of it.
assert.throws(
  () => require('/app/native.node'),
  (error) => {
    assert.match(error.message, /\/app\/native\.node/);
    assert.doesNotMatch(error.message, /\\/);
    return true;
  },
);
",
            ),
            ("/app/native.node", ""),
        ],
        capabilities: &["assert"],
        error: None,
    },
    PackedTest {
        name: "packed::module-paths",
        directory: "module-paths",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');

// Payload paths are POSIX paths on every platform, so the ids JS reports must
// not pick up the host separators of a Windows build.
assert.deepEqual(
  {
    dirname: __dirname,
    filename: module.filename,
    relative: require.resolve('./main.js'),
    absolute: require.resolve('/app/main.js'),
  },
  {
    dirname: '/app',
    filename: '/app/main.js',
    relative: '/app/main.js',
    absolute: '/app/main.js',
  },
);
",
        )],
        capabilities: &["assert"],
        error: None,
    },
    PackedTest {
        name: "packed::lexical-scope-lookup-semantics",
        directory: "lexical-scope-lookup-semantics",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const captured = (() => {
  const value = 'outer';
  return () => value;
})();
if (captured() !== 'outer') throw new Error('closure lookup failed');

function blockShadowing() {
  const value = 'function';
  {
    const value = 'block';
    if (value !== 'block') throw new Error('block lookup failed');
  }
  return value;
}
if (blockShadowing() !== 'function') throw new Error('shadowing failed');

function defaultParameter(value = 5) {
  const result = value + 2;
  return () => result;
}
if (defaultParameter()() !== 7) throw new Error('parameter scope lookup failed');

const environment = { value: 'with' };
let withClosure;
with (environment) {
  withClosure = () => value;
}
if (withClosure() !== 'with') throw new Error('with scope lookup failed');
",
        )],
        capabilities: &[],
        error: None,
    },
    PackedTest {
        name: "packed::worker-vfs",
        directory: "worker-vfs",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                r"
const assert = require('node:assert/strict');
const { Worker } = require('node:worker_threads');

const worker = new Worker('/app/worker.js');
worker.once('error', (error) => {
  throw error;
});
worker.once('message', (message) => {
  assert.equal(message, 'worker loaded from the packed VFS');
});
worker.once('exit', (code) => {
  assert.equal(code, 0);
});
",
            ),
            (
                "/app/worker.js",
                r"
const { parentPort } = require('node:worker_threads');
const assert = require('node:assert/strict');

assert.equal(__dirname, '/app');
assert.equal(require.resolve('./worker.js'), '/app/worker.js');
parentPort.postMessage('worker loaded from the packed VFS');
",
            ),
        ],
        capabilities: &["assert", "worker_threads"],
        error: None,
    },
    // The analyzer's format check rests on this: a packed `package.json` makes the
    // runtime read `.js` files as ES modules.
    PackedTest {
        name: "packed::esm-js-in-module-package",
        directory: "esm-js-in-module-package",
        entry: "/app/main.js",
        files: &[
            ("/app/package.json", r#"{"type": "module"}"#),
            (
                "/app/main.js",
                "import { value } from './value.js';\n\
                 if (value !== 42) throw new Error(`unexpected value: ${value}`);\n",
            ),
            ("/app/value.js", "export const value = 42;\n"),
        ],
        capabilities: &[],
        error: None,
    },
    // A directory manifest may point outside its own directory; the payload Vfs
    // knows only normalized paths, so the resolver has to normalize the target.
    PackedTest {
        name: "packed::directory-main-outside-directory",
        directory: "directory-main-outside-directory",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                "const value = require('./lib');\n\
                 if (value !== 1) throw new Error(`unexpected value: ${value}`);\n",
            ),
            ("/app/lib/package.json", r#"{"main": "../shared/entry.js"}"#),
            ("/app/shared/entry.js", "module.exports = 1;\n"),
        ],
        capabilities: &[],
        error: None,
    },
    PackedTest {
        name: "packed::commonjs-esm-diagnostic",
        directory: "commonjs-esm-diagnostic",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
import { readFileSync } from 'node:fs';
console.log(readFileSync);
",
        )],
        capabilities: &[],
        error: Some(&[
            "/app/main.js is loaded as CommonJS; if it is an ES module, rename it to .mjs \
             or add \"type\": \"module\" to the nearest package.json.",
        ]),
    },
    PackedTest {
        name: "packed::esm-link-error",
        directory: "esm-link-error",
        entry: "/app/main.mjs",
        files: &[
            ("/app/main.mjs", "import './importer.mjs';"),
            (
                "/app/importer.mjs",
                "import { missing } from './dependency.mjs';",
            ),
            ("/app/dependency.mjs", "export const present = 1;"),
        ],
        capabilities: &[],
        error: Some(&[
            "SyntaxError: The requested module './dependency.mjs' does not provide an export named 'missing'",
            "file:///app/importer.mjs:1",
        ]),
    },
    PackedTest {
        name: "packed::esm-large-bundle",
        directory: "esm-large-bundle",
        entry: "/app/main.mjs",
        files: LARGE_ESM_FILES,
        capabilities: &[],
        error: None,
    },
    PackedTest {
        name: "packed::esm-large-link-error",
        directory: "esm-large-link-error",
        entry: "/app/main-error.mjs",
        files: LARGE_ESM_FILES,
        capabilities: &[],
        error: Some(&[
            "The requested module './parts/part-15.mjs' does not provide an export named 'missingPart'",
            "file:///app/esm/missing-export.mjs:2",
        ]),
    },
    PackedTest {
        name: "packed::esm-resolution-error",
        directory: "esm-resolution-error",
        entry: "/app/main.mjs",
        files: &[
            ("/app/main.mjs", "import './importer.mjs';"),
            ("/app/importer.mjs", "import './missing.mjs';"),
        ],
        capabilities: &[],
        error: Some(&[
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/app/missing.mjs' imported from /app/importer.mjs",
        ]),
    },
    PackedTest {
        name: "packed::esm-invalid-specifier-raw-nul",
        directory: "esm-invalid-specifier-raw-nul",
        entry: "/app/main.mjs",
        files: &[
            ("/app/main.mjs", "import './importer.mjs';"),
            (
                "/app/importer.mjs",
                "// Keep the invalid import on line two.\nimport './dependency.mjs\0';",
            ),
            (
                "/app/node_modules/undefined/index.js",
                "module.exports = {};",
            ),
        ],
        capabilities: &[],
        error: Some(&["Invalid ESM import specifier", "file:///app/importer.mjs:2"]),
    },
    PackedTest {
        name: "packed::esm-invalid-specifier-escaped-nul",
        directory: "esm-invalid-specifier-escaped-nul",
        entry: "/app/main.mjs",
        files: &[
            ("/app/main.mjs", "import './importer.mjs';"),
            (
                "/app/importer.mjs",
                r"// Keep the invalid import on line two.
import './dependency.mjs\u0000';",
            ),
            (
                "/app/node_modules/undefined/index.js",
                "module.exports = {};",
            ),
        ],
        capabilities: &[],
        error: Some(&["Invalid ESM import specifier", "file:///app/importer.mjs:2"]),
    },
    PackedTest {
        name: "packed::esm-valid-undefined-package",
        directory: "esm-valid-undefined-package",
        entry: "/app/main.mjs",
        files: &[
            (
                "/app/main.mjs",
                "import value from 'undefined';\nif (value !== 'resolved package') throw new Error('wrong package value');\n",
            ),
            (
                "/app/node_modules/undefined/index.js",
                "module.exports = 'resolved package';",
            ),
        ],
        capabilities: &[],
        error: None,
    },
    PackedTest {
        name: "packed::esm-dynamic-resolution-error",
        directory: "esm-dynamic-resolution-error",
        entry: "/app/main.mjs",
        files: &[(
            "/app/main.mjs",
            r#"
import assert from 'node:assert/strict';

await assert.rejects(import('./missing.mjs'), {
  name: 'Error',
  code: 'ERR_MODULE_NOT_FOUND',
  message: "Cannot find module '/app/missing.mjs' imported from /app/main.mjs",
  url: 'file:///app/missing.mjs',
});
await assert.rejects(import('node:thinpack-missing'), {
  name: 'Error',
  code: 'ERR_UNKNOWN_BUILTIN_MODULE',
  message: 'No such built-in module: node:thinpack-missing',
});
"#,
        )],
        capabilities: &["assert"],
        error: None,
    },
    PackedTest {
        name: "packed::esm-dynamic-link-error",
        directory: "esm-dynamic-link-error",
        entry: "/app/main.mjs",
        files: &[
            (
                "/app/main.mjs",
                r#"
import assert from 'node:assert/strict';

for (let i = 0; i < 2; i++) {
  await assert.rejects(import('./root.mjs'), (error) => {
    assert.ok(error instanceof SyntaxError);
    assert.equal(error.message,
      "The requested module './dependency.mjs' does not provide an export named 'missing'");
    assert.equal(error.code, undefined);
    assert.match(error.stack, /file:\/\/\/app\/importer\.mjs:1/);
    return true;
  });
}
assert.equal(globalThis.dependencyEvaluated, undefined);
"#,
            ),
            ("/app/root.mjs", "import './importer.mjs';"),
            (
                "/app/importer.mjs",
                "import { missing } from './dependency.mjs';",
            ),
            (
                "/app/dependency.mjs",
                "globalThis.dependencyEvaluated = true; export const present = 1;",
            ),
        ],
        capabilities: &["assert"],
        error: None,
    },
    PackedTest {
        name: "packed::crypto-keypair-diffie-hellman",
        directory: "crypto-keypair-diffie-hellman",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
(async () => {
  const assert = require('node:assert/strict');
  const crypto = require('node:crypto');
  const { promisify } = require('node:util');
  const generateKeyPair = promisify(crypto.generateKeyPair);

  function callbackKeyPair(type, options) {
    return new Promise((resolve, reject) => {
      crypto.generateKeyPair(type, options, (error, publicKey, privateKey) => {
        if (error) reject(error);
        else resolve({ publicKey, privateKey });
      });
    });
  }

  function importPair(pair) {
    assert.equal(pair.publicKey.type, 'public');
    assert.equal(pair.privateKey.type, 'private');
    const publicPem = pair.publicKey.export({ type: 'spki', format: 'pem' });
    const privatePem = pair.privateKey.export({ type: 'pkcs8', format: 'pem' });
    return {
      publicKey: crypto.createPublicKey(publicPem),
      privateKey: crypto.createPrivateKey(privatePem),
    };
  }

  function assertSharedSecret(left, right) {
    const leftSecret = crypto.diffieHellman({
      privateKey: left.privateKey,
      publicKey: right.publicKey,
    });
    const rightSecret = crypto.diffieHellman({
      privateKey: right.privateKey,
      publicKey: left.publicKey,
    });
    assert.ok(leftSecret.equals(rightSecret));
    assert.ok(leftSecret.length > 0);
  }

  const ecOptions = { namedCurve: 'prime256v1' };
  const ecSync = crypto.generateKeyPairSync('ec', ecOptions);
  const ecCallback = await callbackKeyPair('ec', ecOptions);
  const ecLeft = importPair(ecSync);
  const ecRight = importPair(ecCallback);
  assertSharedSecret(ecLeft, ecRight);

  const message = Buffer.from('packed EC signing compatibility');
  const signature = crypto.createSign('SHA256').update(message).sign(ecLeft.privateKey);
  assert.equal(
    crypto.createVerify('SHA256').update(message).verify(ecLeft.publicKey, signature),
    true,
  );

  const x25519Sync = crypto.generateKeyPairSync('x25519');
  const x25519Promisified = await generateKeyPair('x25519');
  const x25519Left = importPair(x25519Sync);
  const x25519Right = importPair(x25519Promisified);
  assertSharedSecret(x25519Left, x25519Right);
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
",
        )],
        capabilities: &["assert", "crypto", "util"],
        error: None,
    },
    PackedTest {
        name: "packed::crypto-key-formats",
        directory: "crypto-key-formats",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
const crypto = require('node:crypto');

const keyTypes = [
  ['rsa', { modulusLength: 2048 }],
  ['ec', { namedCurve: 'prime256v1' }],
  ['ec', { namedCurve: 'secp384r1' }],
  ['ec', { namedCurve: 'secp521r1' }],
  ['ec', { namedCurve: 'secp256k1' }],
  ['ed25519', {}],
  ['x25519', {}],
];

let rsaPair;
for (const [type, options] of keyTypes) {
  const pair = crypto.generateKeyPairSync(type, options);
  if (type === 'rsa') rsaPair = pair;
  const privateDer = pair.privateKey.export({ format: 'der', type: 'pkcs8' });
  const publicDer = pair.publicKey.export({ format: 'der', type: 'spki' });
  assert.ok(privateDer.length > 0);
  assert.ok(publicDer.length > 0);
  assert.ok(crypto.createPrivateKey({ key: privateDer, format: 'der', type: 'pkcs8' })
    .export({ format: 'der', type: 'pkcs8' }).equals(privateDer));
  assert.ok(crypto.createPublicKey({ key: publicDer, format: 'der', type: 'spki' })
    .export({ format: 'der', type: 'spki' }).equals(publicDer));
  assert.ok(crypto.createPublicKey(pair.privateKey).export({ format: 'der', type: 'spki' }).equals(publicDer));

  const jwk = pair.privateKey.export({ format: 'jwk' });
  const imported = crypto.createPrivateKey({ key: jwk, format: 'jwk' });
  assert.equal(imported.asymmetricKeyType, pair.privateKey.asymmetricKeyType);
  assert.deepEqual(imported.export({ format: 'jwk' }), jwk);
  assert.ok(crypto.createPublicKey({ key: pair.publicKey.export({ format: 'jwk' }), format: 'jwk' })
    .export({ format: 'der', type: 'spki' }).equals(publicDer));

  if (type === 'ec') {
    const sec1 = pair.privateKey.export({ format: 'pem', type: 'sec1' });
    assert.ok(sec1.includes('BEGIN EC PRIVATE KEY'));
    const sec1Key = crypto.createPrivateKey(sec1);
    assert.equal(sec1Key.asymmetricKeyDetails.namedCurve, pair.privateKey.asymmetricKeyDetails.namedCurve);
    assert.ok(sec1Key.export({ format: 'der', type: 'pkcs8' }).equals(privateDer));
  }

  const encrypted = pair.privateKey.export({
    format: 'pem', type: 'pkcs8', cipher: 'aes-256-cbc', passphrase: 'secret',
  });
  assert.ok(crypto.createPrivateKey({ key: encrypted, passphrase: 'secret' })
    .export({ format: 'der', type: 'pkcs8' }).equals(privateDer));
}

const rsa = rsaPair;
const rsaSignature = crypto.sign('RSA-SHA256', Buffer.from('hello'), rsa.privateKey);
assert.equal(crypto.verify('RSA-SHA256', Buffer.from('hello'), rsa.publicKey, rsaSignature), true);
assert.equal(rsa.privateKey.asymmetricKeyDetails.modulusLength, 2048);
assert.equal(rsa.privateKey.asymmetricKeyDetails.publicExponent, 65537n);

const ec = crypto.generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const ecSignature = crypto.sign('SHA256', Buffer.from('hello'), ec.privateKey);
assert.equal(crypto.verify('SHA256', Buffer.from('hello'), ec.publicKey, ecSignature), true);
assert.equal(ec.privateKey.asymmetricKeyDetails.namedCurve, 'prime256v1');
assert.ok(ec.privateKey.export({ format: 'pem', type: 'sec1' }).includes('BEGIN EC PRIVATE KEY'));

const other = crypto.generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
const otherJwk = other.publicKey.export({ format: 'jwk' });
assert.throws(() => crypto.createPrivateKey({
  key: { ...ec.privateKey.export({ format: 'jwk' }), x: otherJwk.x, y: otherJwk.y },
  format: 'jwk',
}));

const alice = crypto.generateKeyPairSync('x25519');
const bob = crypto.generateKeyPairSync('x25519');
assert.ok(crypto.diffieHellman({ privateKey: alice.privateKey, publicKey: bob.publicKey })
  .equals(crypto.diffieHellman({ privateKey: bob.privateKey, publicKey: alice.publicKey })));

const lowOrder = Buffer.from(bob.publicKey.export({ format: 'der', type: 'spki' }));
lowOrder.fill(0, lowOrder.length - 32);
const lowOrderKey = crypto.createPublicKey({ key: lowOrder, format: 'der', type: 'spki' });
assert.throws(() => crypto.diffieHellman({ privateKey: alice.privateKey, publicKey: lowOrderKey }));

for (const [type, options] of [
  ['dsa', { modulusLength: 2048 }],
  ['dh', { group: 'modp14' }],
  ['ed448', {}],
  ['x448', {}],
]) {
  let thrown;
  try {
    crypto.generateKeyPairSync(type, options);
  } catch (error) {
    thrown = error;
  }
  assert.ok(thrown, `${type} key generation must fail`);
  assert.match(thrown.message, /not supported/);
  assert.equal(thrown.code, 'ERR_CRYPTO_OPERATION_FAILED');
}
",
        )],
        capabilities: &["assert", "crypto"],
        error: None,
    },
    PackedTest {
        name: "packed::zlib-constructors",
        directory: "zlib-constructors",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
const zlib = require('node:zlib');

// Dependencies such as crc32-stream subclass the named compression streams:
// they override `_transform`/`push`, call the inherited implementations, and
// run their own bookkeeping alongside the compressed output.
class DeflateCRC32Stream extends zlib.DeflateRaw {
  constructor(options) {
    super(options);
    this.checksum = 0;
    this.rawSize = 0;
    this.compressedSize = 0;
  }

  push(chunk, encoding) {
    if (chunk) this.compressedSize += chunk.length;
    return super.push(chunk, encoding);
  }

  _transform(chunk, encoding, callback) {
    this.checksum = zlib.crc32(chunk, this.checksum);
    this.rawSize += chunk.length;
    super._transform(chunk, encoding, callback);
  }
}

const input = Buffer.from('thinpack zlib constructor compatibility');
const stream = new DeflateCRC32Stream();
const chunks = [];
stream.on('data', (chunk) => chunks.push(chunk));
stream.on('end', () => {
  const compressed = Buffer.concat(chunks);
  assert.equal(stream.rawSize, input.length);
  assert.equal(stream.compressedSize, compressed.length);
  assert.equal(stream.checksum, zlib.crc32(input));
  assert.ok(zlib.inflateRawSync(compressed).equals(input));

  const pairs = [
    ['Gzip', 'createGzip'],
    ['Gunzip', 'createGunzip'],
    ['Deflate', 'createDeflate'],
    ['Inflate', 'createInflate'],
    ['DeflateRaw', 'createDeflateRaw'],
    ['InflateRaw', 'createInflateRaw'],
    ['Unzip', 'createUnzip'],
    ['BrotliCompress', 'createBrotliCompress'],
    ['BrotliDecompress', 'createBrotliDecompress'],
    ['ZstdCompress', 'createZstdCompress'],
    ['ZstdDecompress', 'createZstdDecompress'],
  ];
  for (const [name, factory] of pairs) {
    assert.equal(typeof zlib[name], 'function', name);
    assert.ok(zlib[factory]() instanceof zlib[name], factory);
  }
});
stream.end(input);
",
        )],
        capabilities: &["assert", "zlib"],
        error: None,
    },
    PackedTest {
        name: "packed::util-diff-without-assert",
        directory: "util-diff-without-assert",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
// `util.diff` uses the private myers_diff helper. The helper is registered by
// the always-linked internals extension, so a template selected with `util`
// alone can load it without pulling in the public `assert` capability.
const { diff } = require('node:util');

const check = (actual, expected) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`unexpected diff: ${JSON.stringify(actual)}`);
  }
};

check(diff('12345678', '12!!5!7!'), [
  [0, '1'],
  [0, '2'],
  [1, '3'],
  [1, '4'],
  [-1, '!'],
  [-1, '!'],
  [0, '5'],
  [1, '6'],
  [-1, '!'],
  [0, '7'],
  [1, '8'],
  [-1, '!'],
]);
check(diff(['1', '2', '3'], ['1', '3', '4']), [
  [0, '1'],
  [1, '2'],
  [0, '3'],
  [-1, '4'],
]);
check(diff('foo', 'foo'), []);
",
        )],
        capabilities: &["util"],
        error: None,
    },
    PackedTest {
        name: "packed::node-test-late-registration",
        directory: "node-test-late-registration",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const test = require('node:test');

test('before the timer', () => {});

// The runner drains on the next tick, so this registration lands after the
// first drain finished. It must still run: a dropped test lets a failing
// suite exit 0. A timer rather than a top-level await keeps the entry a
// CommonJS module, which packed ESM entries do not support on Windows.
setTimeout(() => {
  test('late', () => {
    throw new Error('late boom');
  });
}, 20);
",
        )],
        capabilities: &["test"],
        error: Some(&["late boom"]),
    },
    PackedTest {
        name: "packed::signals-shared-table",
        directory: "signals-shared-table",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const os = require('node:os');
const signals = os.constants.signals;
if (process.platform === 'win32') {
  if (signals.SIGBREAK !== 21) throw new Error('SIGBREAK');
} else {
  const sigbus = process.platform === 'darwin' ? 10 : 7;
  if (signals.SIGBUS !== sigbus) throw new Error('SIGBUS ' + signals.SIGBUS);
  if (typeof signals.SIGURG !== 'number') throw new Error('SIGURG');
  process.kill(process.pid, 'SIGURG');
  process.kill(process.pid, 'SIGWINCH');
}
console.log('signals shared');
",
        )],
        capabilities: &["os"],
        error: None,
    },
    PackedTest {
        name: "packed::websocket-undici",
        directory: "websocket-undici",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"const http = require('node:http');
const crypto = require('node:crypto');
const assert = require('node:assert/strict');

let connections = 0;
const server = http.createServer();
server.on('upgrade', (request, socket) => {
  const invalid = connections++ === 1;
  const key = request.headers['sec-websocket-key'];
  const accept = crypto
    .createHash('sha1')
    .update(key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
    .digest('base64');
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n' +
      `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  let buffer = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 2) {
      const opcode = buffer[0] & 0x0f;
      const masked = (buffer[1] & 0x80) !== 0;
      let length = buffer[1] & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buffer.length < 4) return;
        length = buffer.readUInt16BE(2);
        offset = 4;
      }
      const maskOffset = offset;
      if (masked) offset += 4;
      if (buffer.length < offset + length) return;
      const payload = Buffer.from(buffer.subarray(offset, offset + length));
      if (masked) {
        for (let i = 0; i < payload.length; i += 1) payload[i] ^= buffer[maskOffset + (i % 4)];
      }
      buffer = buffer.subarray(offset + length);
      if (opcode === 0x8) {
        socket.write(Buffer.from([0x88, 0x00]));
        socket.end();
        return;
      }
      if (opcode === 0x1 || opcode === 0x2) {
        if (invalid) {
          socket.write(Buffer.from([0x81, 0x02, 0xc3, 0x28]));
        } else {
          socket.write(Buffer.concat([Buffer.from([0x80 | opcode, length]), payload]));
        }
      }
    }
  });
});

const connect = (port, invalid) =>
  new Promise((resolve) => {
    const socket = new WebSocket(`ws://127.0.0.1:${port}`);
    let message = null;
    socket.onopen = () => socket.send('ping');
    socket.onmessage = (event) => {
      message = event.data;
      socket.close();
    };
    socket.onerror = () => {};
    socket.onclose = () => resolve(message);
  });

server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  assert.equal(await connect(port, false), 'ping');
  assert.equal(await connect(port, true), null);
  server.close();
  console.log('websocket parity ok');
});
",
        )],
        capabilities: &["assert", "crypto"],
        error: None,
    },
    PackedTest {
        name: "packed::require-resolve-errors",
        directory: "require-resolve-errors",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const assert = require('node:assert/strict');
assert.throws(() => require.resolve('./missing'), (error) => {
  assert.equal(error.code, 'MODULE_NOT_FOUND');
  assert.match(error.message, /^Cannot find module '\.\/missing'/);
  return true;
});
console.log('require.resolve errors ok');
",
        )],
        capabilities: &["assert"],
        error: None,
    },
    PackedTest {
        name: "packed::node-test-only",
        directory: "node-test-only",
        entry: "/app/main.js",
        files: &[(
            "/app/main.js",
            r"
const test = require('node:test');

test.only('runs', () => {});
// If `only` is ignored this throws and the packed process exits 1.
test('skipped sibling', () => {
  throw new Error('sibling ran');
});
",
        )],
        capabilities: &["test"],
        error: None,
    },
    PackedTest {
        name: "packed::module-parent",
        directory: "module-parent",
        entry: "/app/main.js",
        files: &[
            (
                "/app/main.js",
                r"
const assert = require('node:assert/strict');
assert.equal(module.parent, null);
const child = require('./child.cjs');
assert.equal(child.parent, module);
assert.equal(require('./child.cjs').parent, module);
console.log('module.parent ok');
",
            ),
            ("/app/child.cjs", "exports.parent = module.parent;\n"),
        ],
        capabilities: &["assert"],
        error: None,
    },
];

/// Packed startup cases whose source is generated because the payload is large.
struct StartupTest {
    name: &'static str,
    directory: &'static str,
    source: fn() -> String,
}

const STARTUP_TESTS: &[StartupTest] = &[
    StartupTest {
        name: "packed::large-lexical-scope-startup",
        directory: "large-lexical-scope-startup",
        source: large_scope_source,
    },
    StartupTest {
        name: "packed::minified-cold-startup",
        directory: "minified-cold-startup",
        source: minified_source,
    },
];

pub(super) fn print_list(cli: &super::Cli) {
    for test in TESTS {
        if cli.matches(test.name) {
            println!("{}", test.name);
        }
    }
    for test in STARTUP_TESTS {
        if cli.matches(test.name) {
            println!("{}", test.name);
        }
    }
}

pub(super) fn execute(cli: &super::Cli) -> bool {
    if cli.ignored == super::Ignored::Only {
        return true;
    }

    let selected: Vec<&PackedTest> = TESTS.iter().filter(|test| cli.matches(test.name)).collect();
    let startup: Vec<&StartupTest> = STARTUP_TESTS
        .iter()
        .filter(|test| cli.matches(test.name))
        .collect();
    let test_count = selected.len() + startup.len();
    if test_count == 0 {
        return true;
    }

    println!(
        "running {test_count} packed test{}",
        if test_count == 1 { "" } else { "s" }
    );
    let mut passed = 0;
    let mut failed = 0;
    for test in selected {
        let started = Instant::now();
        report(test.name, run_test(test), started, &mut passed, &mut failed);
    }
    for test in startup {
        let started = Instant::now();
        report(
            test.name,
            run_startup_test(test),
            started,
            &mut passed,
            &mut failed,
        );
    }
    println!("packed test result: {passed} passed; {failed} failed");
    failed == 0
}

fn report(
    name: &str,
    outcome: super::Outcome,
    started: Instant,
    passed: &mut usize,
    failed: &mut usize,
) {
    match outcome {
        super::Outcome::Passed => {
            *passed += 1;
            println!("ok {name} ({:.2}s)", started.elapsed().as_secs_f64());
        }
        super::Outcome::Failed(report) => {
            *failed += 1;
            println!(
                "FAILED {name} ({:.2}s)\n{report}",
                started.elapsed().as_secs_f64()
            );
        }
    }
}

fn run_test(test: &PackedTest) -> super::Outcome {
    let directory = match TempDir::new(test.directory) {
        Ok(directory) => directory,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot create test directory: {error}"));
        }
    };
    let binary =
        match build_packed_payload(test.entry, test.files, test.capabilities, directory.path()) {
            Ok(binary) => binary,
            Err(error) => {
                return super::Outcome::Failed(format!(
                    "cannot build packed test executable: {error}"
                ));
            }
        };
    run_binary(&binary, directory.path(), test.error, None)
}

fn run_startup_test(test: &StartupTest) -> super::Outcome {
    let directory = match TempDir::new(test.directory) {
        Ok(directory) => directory,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot create test directory: {error}"));
        }
    };
    let source = (test.source)();
    let files = [("/app/main.js", source.as_str())];
    let binary = match build_packed_payload("/app/main.js", &files, &[], directory.path()) {
        Ok(binary) => binary,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot build packed test executable: {error}"));
        }
    };

    let started = Instant::now();
    match run_binary(&binary, directory.path(), None, Some(STARTUP_MARKER)) {
        super::Outcome::Passed if started.elapsed() <= STARTUP_LIMIT => super::Outcome::Passed,
        super::Outcome::Passed => super::Outcome::Failed(format!(
            "startup took {:.2}s, exceeding {} ms",
            started.elapsed().as_secs_f64(),
            STARTUP_LIMIT.as_millis()
        )),
        outcome @ super::Outcome::Failed(_) => outcome,
    }
}

fn large_scope_source() -> String {
    let mut source = String::with_capacity(1_100_000);
    source.push_str("function require_coreV1Api(){if(false){\n");
    for index in 0..50_000 {
        writeln!(source, "const value_{index}=0;").expect("writing to a String cannot fail");
    }
    source.push_str("}}\nconsole.log('startup complete');\n");
    source
}

fn minified_source() -> String {
    let mut source = String::with_capacity(700_000);
    source.push_str("function require_coreV1Api(){if(false){");
    for index in 0..50_000 {
        write!(source, "value_{}({index});", index % 16).expect("writing to a String cannot fail");
    }
    source.push_str("}}\nconsole.log('startup complete');\n");
    source
}

fn build_packed_payload(
    entry: &str,
    source_files: &[(&str, &str)],
    capability_roots: &[&str],
    directory: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let runtime = Path::new(env!("CARGO_BIN_EXE_thinpack-rt"));
    let template = inspect_template(runtime)?;
    let mut blob = Vec::new();
    let mut files = Vec::with_capacity(source_files.len());
    for &(path, source) in source_files {
        let offset = u64::try_from(blob.len())?;
        let contents = source.as_bytes();
        let length = u32::try_from(contents.len())?;
        files.push(FileRecord {
            path: path.to_owned(),
            offset,
            length,
        });
        blob.extend_from_slice(contents);
    }

    let capabilities = capabilities(capability_roots);
    let manifest = Manifest {
        entry: entry.to_owned(),
        capabilities,
        template,
        files,
        assets: Vec::new(),
    };
    let payload = thinpack_pack::assemble(&manifest, &blob, Compression::None)?;
    let runtime_bytes = fs::read(runtime)?;
    let executable = thinpack_pack::append_to_binary(&runtime_bytes, &payload)?;
    #[cfg(windows)]
    let name = "packed-test.exe";
    #[cfg(not(windows))]
    let name = "packed-test";
    let path = directory.join(name);
    fs::write(&path, executable)?;
    fs::set_permissions(&path, fs::metadata(runtime)?.permissions())?;
    Ok(path)
}

fn inspect_template(runtime: &Path) -> Result<Template, Box<dyn std::error::Error>> {
    let output = Command::new(runtime).arg("--template-info").output()?;
    if !output.status.success() {
        return Err(format!(
            "{} --template-info failed: {}",
            runtime.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let output = String::from_utf8(output.stdout)?;
    let mut version = None;
    let mut backend = None;
    let mut capabilities_hash = None;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "version" => version = Some(value.to_owned()),
            "backend" => backend = Some(value.to_owned()),
            "capabilities-hash" => {
                capabilities_hash = Some(u64::from_str_radix(value.trim_start_matches("0x"), 16)?);
            }
            _ => {}
        }
    }

    Ok(Template {
        version: version.ok_or("runtime reported no version")?,
        backend: thinpack_pack::Backend::parse(&backend.ok_or("runtime reported no backend")?)
            .ok_or("runtime reported an unknown backend")?,
        capabilities_hash: capabilities_hash.ok_or("runtime reported no capabilities hash")?,
    })
}

fn capabilities(roots: &[&str]) -> Vec<String> {
    let mut names: BTreeSet<String> = thinpack_caps::baseline()
        .map(|module| module.name.to_owned())
        .collect();
    let roots: Vec<String> = roots.iter().map(|name| (*name).to_owned()).collect();
    names.extend(
        thinpack_caps::closure(&roots)
            .expect("packed test roots are known capabilities")
            .into_iter()
            .map(|module| module.name.to_owned()),
    );
    names.into_iter().collect()
}

fn run_binary(
    binary: &Path,
    directory: &Path,
    expected: Option<&[&str]>,
    expected_stdout: Option<&str>,
) -> super::Outcome {
    let mut command = Command::new(binary);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::configure_process_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return super::Outcome::Failed(format!("cannot spawn packed executable: {error}"));
        }
    };
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        return super::Outcome::Failed("packed executable stdout is not piped".to_owned());
    };
    let Some(stderr) = child.stderr.take() else {
        return super::Outcome::Failed("packed executable stderr is not piped".to_owned());
    };
    let stdout = thread::spawn(move || super::read_all(stdout));
    let stderr = thread::spawn(move || super::read_all(stderr));

    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return super::Outcome::Failed(format!(
                    "cannot wait for packed executable: {error}"
                ));
            }
        }
        if Instant::now() >= deadline {
            timed_out = true;
            super::kill_process_group(pid);
            if let Err(error) = child.kill() {
                return super::Outcome::Failed(format!(
                    "cannot kill timed-out packed test: {error}"
                ));
            }
            match child.wait() {
                Ok(status) => break status,
                Err(error) => {
                    return super::Outcome::Failed(format!(
                        "cannot reap timed-out packed test: {error}"
                    ));
                }
            }
        }
        thread::sleep(Duration::from_millis(25));
    };

    super::kill_process_group(pid);
    let stdout = super::read(stdout);
    let stderr = super::read(stderr);
    let output = format!("{stdout}{stderr}");
    if !timed_out {
        match expected {
            Some(fragments)
                if !status.success()
                    && fragments.iter().all(|fragment| output.contains(fragment)) =>
            {
                return super::Outcome::Passed;
            }
            None if status.success()
                && expected_stdout.is_none_or(|fragment| stdout.contains(fragment)) =>
            {
                return super::Outcome::Passed;
            }
            _ => {}
        }
    }

    let reason = if timed_out {
        format!("timed out after {} ms", TEST_TIMEOUT.as_millis())
    } else if let Some(fragments) = expected {
        format!(
            "expected a failure whose output contains {fragments:?}, got {}",
            super::describe_status(status)
        )
    } else if let Some(fragment) = expected_stdout.filter(|_| status.success()) {
        format!("expected stdout to contain {fragment:?}")
    } else {
        format!("exited with {}", super::describe_status(status))
    };
    let mut report = format!("{reason}\n");
    if !stdout.is_empty() {
        report.push_str("\n----- stdout -----\n");
        report.push_str(&super::truncate(&stdout));
    }
    if !stderr.is_empty() {
        report.push_str("\n----- stderr -----\n");
        report.push_str(&super::truncate(&stderr));
    }
    super::Outcome::Failed(report)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "thinpack-packed-{name}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
