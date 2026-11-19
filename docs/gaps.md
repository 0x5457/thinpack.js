# Node compatibility gaps

Where a built-in deviates from Node.js, this file is the record. The rule every
entry follows: a feature either behaves like Node or it fails loudly
(`ERR_NOT_IMPLEMENTED`); it never returns a fabricated value. Each entry names
the divergence, why it exists, and where it lives.

## Globals

- `fetch`, `Headers`, `Request`, `Response` and `FormData` are Node's own undici, vendored
  byte for byte from `deps/undici/undici.js` at the Node tag this repo tracks
  (`crates/node/http/src/vendor/undici.js`). They are the same code Node runs, so request
  construction, redirects, `AbortSignal`, multipart encoding and the error shapes are Node's,
  and the module needs no seams: the bundle requires only `node:` builtins.
  - That bundle is the reason a `fetch` call pulls sixteen modules into the payload
    (`assert`, `async_hooks`, `buffer`, `crypto`, `diagnostics_channel`, `events`, `http`,
    `net`, `perf_hooks`, `querystring`, `stream`, `tls`, `url`, `util`, `worker_threads`,
    `zlib`), which closes over 21 capabilities once the ambient modules are added. `http2`
    and `sqlite` stay out of both sets: undici probes them and handles their absence, so the
    analyzer does not pull them and `--without http2`/`--without sqlite` keep working.
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
