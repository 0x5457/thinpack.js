# Node compatibility gaps

Where a built-in deviates from Node.js, this file is the record. The rule every
entry follows: a feature either behaves like Node or it fails loudly
(`ERR_NOT_IMPLEMENTED`); it never returns a fabricated value. Each entry names
the divergence, why it exists, and where it lives.

## Backends

- The `jsc` template runs on macOS's `JavaScriptCore.framework` instead of bundling an engine.
  Its ES module support is a source rewriter aimed at flattened bundles
  (`vendor/v8x/src/jsc/module.rs`);
  packed apps keep their file boundaries, so live bindings, cycles and namespace edge cases can
  differ from the QuickJS and V8 templates. A missing named export evaluates as `undefined`
  instead of failing instantiation, and Apple's framework does not parse import attributes
  (`import ... assert { ... }`).
- That backend cannot create a `SharedArrayBuffer` over a transferred backing store through
  JSC's public C API, and v8x's serializer copies shared bytes into an ordinary `ArrayBuffer`.
  `SharedArrayBuffer` and `Atomics` work within one isolate, but a cross-isolate worker message
  does not share the buffer: the receiver gets a copy, and `Atomics.wait` on it throws. This is
  a v8x limitation; the shared-store tests are skipped there
  (`crates/thinpack-core/tests/sab_store.rs`).

## TypeScript

- Packing strips `.ts`/`.mts`/`.cts` with Node's strip-only pass, so a packed app never needs a
  TypeScript parser. The accepted syntax is Node's: `enum`, `namespace`, parameter properties,
  `import x = require()`, `export =` and `.tsx` fail at pack time, matching Node without
  `--experimental-transform-types`.
- TypeScript under `node_modules` is stripped too. Node rejects it
  (`ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING`) because its runtime pays the cost; packing is a
  build step, so refusing there would only reject a module the artifact can carry as JavaScript.
- Type-only imports are erased before dependency analysis, so a module named only by `import type`
  does not have to resolve. Node resolves static specifiers before stripping and can fail there.
- The runtime does not strip TypeScript outside the payload: a packed app that requires a host
  `.ts` file gets `ERR_UNKNOWN_FILE_EXTENSION` instead of a parse error from the engine.

## Globals

- `fetch`, `Headers`, `Request`, `Response` and `FormData` are Node's own undici, vendored
  byte for byte from `deps/undici/undici.js` at the Node tag this repo tracks
  (`crates/node/http/src/vendor/undici.js`). They are the same code Node runs, so request
  construction, redirects, `AbortSignal`, multipart encoding and the error shapes are Node's,
  and the module needs no seams: the bundle requires only `node:` builtins.
  - That bundle is the reason a `fetch` or `WebSocket` call pulls sixteen modules into the payload
    (`assert`, `async_hooks`, `buffer`, `crypto`, `diagnostics_channel`, `events`, `http`,
    `net`, `perf_hooks`, `querystring`, `stream`, `tls`, `url`, `util`, `worker_threads`,
    `zlib`), which closes over 21 capabilities once the ambient modules are added. `http2`
    and `sqlite` stay out of both sets: undici probes them and handles their absence, so the
    analyzer does not pull them and `--without http2`/`--without sqlite` keep working.
  - `WebSocket` is the bundle's implementation, not a separate stack, so it shares the same
    closure and the same protocol behavior as Node.
  - Loading is lazy, the payload is not: the bundle is a lazy source of the `http` extension,
    and every lazy source is decompressed and retained as a string per realm when the runtime
    is created. An app that enables `http` — which includes every app that only uses
    `new WebSocket(…)` — therefore pays its 132 KiB of template, a brotli decode per realm and
    its 812 KiB of JavaScript kept for the life of the process, whether or not `fetch` is ever
    evaluated; only evaluation is lazy.
  - `EventSource` is exported by that bundle but absent here: Node gates the global behind
    `--experimental-eventsource` and deletes it otherwise, and this runtime has no equivalent
    switch, so the global is never installed.
  - `MessageEvent`, `CloseEvent` and `ErrorEvent` are likewise exported by the bundle and
    exposed by Node by default, but thinpack defines none of them, so `new MessageEvent('x')`
    is a `ReferenceError`.
  - `reportError` throws its argument from a microtask, which reaches the same
    uncaught-exception path; Node first dispatches a cancelable `ErrorEvent` on `globalThis`,
    and that global does not exist here
    (`crates/thinpack-rt/src/bootstrap/fetch_globals.js`).
  - Everything undici builds on is still this runtime's implementation, so the entries below
    for `net`, `tls`, `zlib` and the streams apply to it too.
- `Blob` parts are read through a JS reader (`crates/node/buffer/src/binding_blob.js`). Node's
  reader is C++ and can answer `STATUS_BLOCK`; these parts are already in memory and always
  answer synchronously, so `setWakeup` accepts Node's callback and never invokes it.

## Event loop

- `setImmediate` shares the timer queue as a 0 ms timer instead of running in a
  separate check phase after I/O; ordering versus `setTimeout(…, 1)` and I/O
  callbacks differs, and the handle lacks Node's `Immediate` shape
  (`crates/node/timers/src/timers.js`).

## Async hooks

- `destroy` hooks run synchronously where a resource is released:
  `AsyncResource.emitDestroy()` before it returns, and timers/immediates before
  their queued microtasks. Node defers them to a later tick; the runtime has no
  deferred destroy queue (`crates/thinpack-node-internals/src/async_hooks.js`).
- The JSC backend exposes no promise hooks and does not preserve the async
  context across promise continuations, so `AsyncLocalStorage` does not follow
  promises there; the local async-local case is skipped on JSC
  (`vendor/v8x/src/jsc/isolate.rs`).

## TLS

- rustls performs real hostname verification, but the peer certificate never
  crosses into JavaScript: `getPeerCertificate()`, `getSession()` and
  `getCACertificates()` throw `ERR_NOT_IMPLEMENTED` instead of inventing
  objects, and a custom `checkServerIdentity` fails the connect with
  `ERR_NOT_IMPLEMENTED` rather than being skipped silently. Surfacing the
  certificate and invoking the hook is a tracked follow-up
  (`crates/node/tls/src/tls.js`).

## Streams and codecs

- `Cipheriv`/`Decipheriv` buffer all output until `final()`: the one-shot
  cipher op has no incremental state machine, so `update()` returns empty
  buffers and `final()` returns everything (`crates/node/crypto/src/crypto.js`).
- `zlib` streams compress in one shot per stream; `fs.cp`/`fs.rm` callbacks run
  their work inside a microtask, so a large tree blocks the event loop for the
  duration (`crates/node/zlib/src/zlib.js`, `crates/node/fs/src/fs_rimraf.js`).

## Crypto

- Key material is handled by RustCrypto key crates, not OpenSSL.
  `generateKeyPair`/`generateKeyPairSync`, `createPrivateKey`/`createPublicKey`,
  `KeyObject.export`, JWK import/export and `diffieHellman` support RSA/RSA-PSS, EC
  (P-256/P-384/P-521/secp256k1), Ed25519 and X25519. DSA, DH, Ed448 and X448 throw
  `ERR_CRYPTO_OPERATION_FAILED` instead of being fabricated; post-quantum key types are
  unsupported. Selecting the RSA-PSS hash, MGF1 hash or salt length, or selecting a DSA
  divisor length, throws `ERR_NOT_IMPLEMENTED` (`crates/node/crypto/src/keypair.js`).
- An `rsa-pss` key is stored as RSA key material: a PSS-parameterized PKCS#8 import is
  accepted and normalized, and export writes `rsaEncryption`. The JS signing surface still
  rejects `RSA-PSS` (`crates/node/crypto/src/ops/keys.rs`).
- EC key generation accepts `paramEncoding: 'explicit'` but always writes named-curve
  parameters, and only P-256, P-384, P-521 and secp256k1 are available (P-192 and P-224
  throw `ERR_CRYPTO_INVALID_CURVE`); `crates/node/crypto/src/ops/keys/ec.rs` carries the
  curve table.
- PKCS#8 encryption supports `aes-128-cbc`, `aes-192-cbc` and `aes-256-cbc`. Encrypting
  PKCS#1 or SEC1 PEM needs OpenSSL's `DEK-Info` scheme and fails loudly instead
  (`crates/node/crypto/src/ops/keys.rs`).

## File system

- `readdir` returns entries sorted by byte order rather than the directory
  order Node reports; deterministic, but observable
  (`crates/node/fs/src/ops.rs`).
- `fs.openAsBlob` reads the file eagerly into memory; Node keeps the blob
  file-backed and reads lazily (`crates/node/fs/src/node-fs.js`).
- `fs.watch` has no retry loop: `maxRetries`/`retryDelay` are accepted and
  ignored, a rename of the watched directory does not re-arm the watcher, and
  closing a watcher that never started (missing path with
  `throwIfNoEntry: false`) still emits `close` where Node stays silent
  (`crates/node/fs/src/fs_watchers.js`).
- `rmSync` ignores `maxRetries`/`retryDelay`; the retryable failures Node loops
  over (EBUSY, EMFILE, …) surface directly
  (`crates/node/fs/src/binding_fs.js`).

## Module resolution

- The resolver keeps lexical paths (Node's `--preserve-symlinks` behavior) instead of
  canonicalizing symlinks. `__dirname`, `require.resolve` and a linked package's own
  dependency lookup therefore see the link location, and two specifiers that reach the
  same real file through different links remain distinct modules
  (`crates/thinpack-resolve/src/lib.rs`).

## Process and platform

- On unix platforms that are neither Linux nor macOS, `process.memoryUsage()`
  reports `rss` from `getrusage`'s high-water mark instead of a live value, and
  the heap fields are zero (QuickJS has no V8 heap accounting)
  (`crates/node/process/src/ops.rs`).
- `v8.getHeapStatistics()` returns real usage where the engine reports it, but
  `heap_size_limit` stays zero: neither engine backend exposes a limit
  (`crates/node/v8/src/v8.js`).
- `os.getPriority`/`os.setPriority` throw `ERR_NOT_IMPLEMENTED`; a real
  implementation needs the platform priority mapping plus
  `os.constants.priority` (`crates/node/os/src/os.js`).
- `process.versions.undici` reports the vendored bundle's version even in a build without
  `http`, where that bundle is not compiled in
  (`crates/node/process/src/process.js`).
- `domain` implements the synchronous `create/run/bind/intercept/add/remove/enter/exit`
  subset. The active domain is not carried through timers or promises (no
  `AsyncLocalStorage`-backed store), so an async error is not routed to its domain
  (`crates/node/domain/src/domain.js`).
- HTTP/2 over UNIX-domain sockets and TLS over non-TCP transports fail with
  `ERR_NOT_IMPLEMENTED` (`crates/node/http2/src/ops.rs`,
  `crates/node/net/src/ops.rs`).

## Worker threads

- Workers in a packed executable see the `/app` virtual file system, but a
  relative worker path still resolves against the host working directory, like
  the unpacked runtime; load virtual files by absolute `/app/...` path, which
  stays in the payload's path space on every platform
  (`crates/node/worker-threads/src/worker_threads.js`).
- `markAsUncloneable` marks the object in a `WeakSet` that the JS `structuredClone`
  consults, so cloning a marked value throws `DataCloneError`. `worker_threads`
  `postMessage` and `core.serialize` do not consult it: they go through the engine
  serializer, so a marked value still transfers there
  (`crates/thinpack-rt/src/bootstrap/structured_clone.js`,
  `crates/node/worker-threads/src/worker_threads.js`).
- `getEnvironmentData`/`setEnvironmentData` still throw `ERR_NOT_IMPLEMENTED`;
  real environment data needs cross-thread state
  (`crates/node/worker-threads/src/worker_threads.js`).

## Inspector

- `inspector` exposes `Session`, but `connect`/`post` throw
  `ERR_NOT_IMPLEMENTED`; there is no inspector protocol implementation
  (`crates/node/inspector/src/inspector.js`).

## Test runner

- `node:test` implements registration (`test`/`it`/`describe`/`suite` with the
  `skip`/`todo`/`only` variants), hooks, nested subtests, per-test mock trackers
  with `mock.fn`/`mock.method`, TAP output on stdout and `process.exitCode` on
  failure. Everything else fails loudly rather than returning a fabricated
  value: `run()` throws `ERR_NOT_IMPLEMENTED`, because its `TestsStream` result,
  the reporters and the `--test` CLI are not implemented; a test context exposes
  no `assert`, `plan`, `snapshot`, `waitFor` or `setTimeout`; and a mock tracker
  has no `timers`, `getter`, `setter` or `property`
  (`crates/node/test/src/node-test.js`,
  `crates/node/test/src/node-test-runner.js`).
- Mocks follow Node's scoping: `t.mock` is reset when that test finishes, while
  the module-level `mock` tracker is only reset when the test calls `reset()` or
  `restoreAll()`, so a mock installed through it in `before()` survives the
  suite's tests.
- A test registered after the run has reported still runs: registrations are
  drained until stable and a later registration starts another drain, so a late
  test fails the process instead of being dropped. Root `before`/`after` hooks
  run once, around the first drain.
- Options that only make sense with the missing parts are accepted and ignored:
  a suite's `timeout` does not apply to its children (a hung child falls through
  to the caller's timeout) and `concurrency` has no effect, because tests run
  one at a time.
- A test that drives workers and so depends on cross-thread cloning
  (`structuredClone`, `markAsUncloneable`, `getEnvironmentData`) inherits the
  limits recorded under Worker threads: those helpers throw
  `ERR_NOT_IMPLEMENTED` (`crates/node/worker-threads/src/worker_threads.js`).
