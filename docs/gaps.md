# Node compatibility gaps

Where a built-in deviates from Node.js, this file is the record. The rule every
entry follows: a feature either behaves like Node or it fails loudly
(`ERR_NOT_IMPLEMENTED`); it never returns a fabricated value. Each entry names
the divergence, why it exists, and where it lives.

## Globals

- The globals Node gets from undici are absent: `fetch` and its types
  (`Headers`, `Request`, `Response`, `FormData`). Node defines them from a
  dependency bundled into its binary; this project vendors Node's `lib/`, not its
  dependencies. `WebSocket`, `Blob` and `File` are present, `EventSource` and
  `reportError` are not, and code that reaches for a missing global fails with
  `ReferenceError`. `http`, `https` and `http2` are unaffected.

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
- HTTP/2 over UNIX-domain sockets and TLS over non-TCP transports fail with
  `ERR_NOT_IMPLEMENTED` (`crates/node/http2/src/ops.rs`,
  `crates/node/net/src/ops.rs`).

## Worker threads

- Workers in a packed executable see the `/app` virtual file system, but a
  relative worker path still resolves against the host working directory, like
  the unpacked runtime; load virtual files by absolute `/app/...` path, which
  stays in the payload's path space on every platform
  (`crates/node/worker-threads/src/worker_threads.js`).
- `getEnvironmentData`/`setEnvironmentData`/`markAsUncloneable` throw
  `ERR_NOT_IMPLEMENTED`; real environment data needs cross-thread clone
  machinery (`crates/node/worker-threads/src/worker_threads.js`).

## Inspector

- `inspector` exposes `Session`, but `connect`/`post` throw
  `ERR_NOT_IMPLEMENTED`; there is no inspector protocol implementation
  (`crates/node/inspector/src/inspector.js`).
