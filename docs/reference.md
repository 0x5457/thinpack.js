# Usage and runtime reference

[English quick start](../README.md) · [简体中文](../README.zh-CN.md) · [Benchmarks](benchmarks.md)

## Downloads and runtime choices

The installer fetches the [latest published release](https://github.com/thinpack-js/thinpack.js/releases/latest), not a build of `main`. Check the release assets for your OS, architecture, and backend. Archives can be checked against `SHA256SUMS`. Set `THINPACK_INSTALL_DIR` to change the install directory or `THINPACK_VERSION` to pin a published release.

`thinpack-release.toml` lists every archive with its backend, target and, for the glibc builds, the oldest glibc it needs. `cargo xtask dist` measures that floor from the binaries, and the installer reads the file to choose an archive this host can run.

QuickJS is the default. For releases that contain matching archives:

- **Static Linux (QuickJS):** `THINPACK_STATIC=1` selects the musl archive for older Linux distributions. Its resolver reads `/etc/hosts` and `/etc/resolv.conf`; no NSS or mDNS.
- **V8:** `THINPACK_BACKEND=v8` selects a `-v8-` archive. There is no static V8 build, so V8 on Linux needs a glibc host.
- **JavaScriptCore:** `THINPACK_BACKEND=jsc` selects a `-jsc-` archive on macOS. It uses the system framework and has [module compatibility limits](#known-limits).

The prebuilt runtime carries all built-ins. To reproduce the smaller `--rebuild` artifacts in the [benchmark](benchmarks.md), use a matching source checkout and a build toolchain:

```sh
thinpack pack hello.js -o hello --rebuild --source /path/to/thinpack.js
```

Thinpack uses `oxc` to trace imports and links only the required built-ins into the rebuilt runtime, then appends the app's sources and assets.

## Build from source

Requires Rust 1.99.1 (pinned by `rust-toolchain.toml`), a C toolchain and CMake.

```sh
cargo build --release -p thinpack-rt -p thinpack-cli
./target/release/thinpack pack examples/hello.js -o hello
./hello
```

`cargo xtask dist` builds the release archive for the host and verifies it by packing and running
`examples/hello.js` from the staged pair. It writes the archive's `thinpack-release.toml` fragment
next to it; the release workflow concatenates the fragments into the manifest the installer reads.

## Commands

| Command / option | Effect |
| --- | --- |
| `pack <entry> [-o <out>]` | analyze the module graph, trim capabilities, write a single-file artifact |
| `run <entry> [-- <args>...]` | run with a matching template without packing |
| `templates` | list the template cache |
| `capabilities` | list the module names accepted by this CLI |
| `--template <path>` | use the given runtime template |
| `--backend <quickjs\|v8\|jsc>` | pick the template backend, `quickjs` by default; `jsc` is macOS-only |
| `--rebuild` `--source <checkout>` | build a template for this app's own capability set and cache it |
| `--asset <key>=<path>` | embed an asset, readable through `node:sea` |
| `--include <path>` | pack extra files (`.d.ts`, templates, computed imports); paths are relative to the project root, and JavaScript and TypeScript are analyzed too |
| `--with <module>` `--without <module>` | override the derived capability set. Both accept comma-separated lists or repeated flags. |
| `--allow-missing <package>` | pack despite a missing package (it becomes a warning); use only for dependencies an inactive feature branch loads |
| `--conditions <name>` | add custom `exports`/`imports` conditions, like Node's `--conditions`; the payload records them for runtime resolution |
| `--strict` | a `require`/`import` that cannot be statically evaluated is an error; missing optional dependencies only warn |
| `--list-capabilities` | print the derived capability set and exit |

The CLI checks the template API before packing, and the runtime verifies the payload's version,
backend and capability-set hash at startup. Templates live under `$THINPACK_CACHE` (then `$XDG_CACHE_HOME/thinpack`, then
`~/.cache/thinpack`) as `templates/<version>/<backend>/<caps-hash>`, and resolve `--template` →
`--rebuild` → exact hit → smallest superset → the `thinpack-rt` next to the CLI. `--rebuild`
comes before the cache so an app always packs against its own closure and uses QuickJS by default.
For ordinary packing, `--backend` takes precedence over the backend of an installed template.
When no matching installed template is available and the cache has both backends, pass `--backend`.

The analyzer follows conditional `require` calls when both branches name modules. For module names
supplied at runtime, use `--with` to include the required built-ins explicitly. `--strict` makes
unresolved dynamic requests an error during packing.

A path built from `__dirname` can name files the module never imports: files read through
`fs.readFileSync`, `readFile`, or `createReadStream` are packed as data, and so is every file under
a directory listed with `fs.readdir` or `readdirSync`. Enumeration counts only on the `fs` module,
and a scanned directory is expanded like `--include` only when it is below the project root and is
neither a package root nor `node_modules`; anything broader still needs `--include`. `path.join`,
`path.resolve`, string concatenation, and single-assignment bindings of them are evaluated. A
`require` of one constant path built that way packs the named file. A computed `require`/`import`
whose loaded binding has a `path.join`/`path.resolve` call with a literal `__dirname` directory and
one final computed segment, or whose argument is a template literal naming
``./commands/${command}.js``, packs that directory's files matching the segment's literal suffix,
or JavaScript modules when it has none; other computed module paths still need `--include`.

The optionality of a guarded `require`/`import()` or an `optionalDependencies` entry extends
through the static dependencies it reaches: a missing module there becomes a warning, and the
handled import fails at run time as it does under Node. A file also reached by a required edge
is still required, so its missing dependencies fail the pack.

Compiled module bytecode is cached under `<cache-root>/bytecode`, which makes a warm start
about 29% faster (pi: 983 ms → 702 ms); `V82JSC_NO_BC_CACHE=1` disables it, and
`--warm-cache` pre-compiles the payload's modules at deploy time without running app code.

### Module format

Entry and dependency formats follow Node.js: `.mjs` is an ES module, `.cjs` is CommonJS, and a
`.js` file is CommonJS unless the nearest `package.json` declares `"type": "module"`. A `.js`
file in a package without a `type` that contains ESM syntax is detected as an ES module at pack
time, with a warning that names the file and suggests `"type": "module"`; the detected format is
recorded in the payload so the loader agrees. If the package explicitly declares
`"type": "commonjs"`, the file still fails at pack time: the error names the file and both
remedies — rename it to `.mjs` or add `"type": "module"` — and the mirror case points at `.cjs`
and at removing the declaration. `.cjs` and `.mjs` fix their format by extension and keep the
parser's own diagnostic. The runtime keeps the same check for modules that reach it another way.

TypeScript follows the same format rules: `.mts` and `.cts` fix the format, and `.ts` reads the
package scope like `.js`. Packing strips the types with Node's strip-only pass (the SWC engine
behind `amaro`), so the executable carries JavaScript and links no TypeScript parser. Only
erasable syntax is accepted: `enum`, `namespace`, parameter properties, `import x = require()`,
`export =` and `.tsx` fail at pack time, as they do in Node without
`--experimental-transform-types`. TypeScript under `node_modules` is not stripped either: Node
refuses it with `ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING`, so packing rejects it instead of
building an artifact that behaves differently. TypeScript outside the payload is not stripped at
run time.

## Built-in modules

Ambient (every template): `console`, `process`, `timers`, `buffer`, `url`, plus `util` because
`console` formats through it.

On demand: `assert`, `async_hooks`, `child_process`, `constants`, `crypto`, `dgram`,
`diagnostics_channel`, `dns`, `events`, `fetch`, `ffi`, `fs`, `http`, `http2`, `https`,
`inspector`, `module`, `net`, `os`, `path`, `perf_hooks`, `querystring`, `readline`, `sea`,
`sqlite`, `stream`, `string_decoder`, `test`, `tls`, `tty`, `util`, `v8`, `vm`, `worker_threads`,
`zlib`.
Subpaths (`fs/promises`, `stream/web`, `path/posix`, `util/types`, …) ship with their module.

`http2` and `sqlite` are optional: a package probing for them does not fail the pack. A module
that was not compiled in throws `ERR_UNKNOWN_BUILTIN_MODULE` on `require`, matching Node.

## Architecture

```text
crates/
  thinpack-core/     engine lifecycle, event loop, backend selection
  thinpack-caps/     capability table (generated from capabilities.toml, engine-free)
  thinpack-resolve/  Node resolution algorithm (over FileSystem, shared by compile and run time)
  thinpack-loader/   CJS and ESM module system
  thinpack-analyze/  oxc static analysis: module graph and capability closure
  thinpack-pack/     payload format: manifest, compression, trailer
  thinpack-vfs/      virtual file system of an artifact (virtual root /app)
  thinpack-io/       shared stdio ops
  thinpack-child/    spawned-process table and stdio pipes shared by child_process and cluster
  thinpack-js/       proc macro: brotli-compress the embedded JavaScript at build time
  thinpack-node-internals/  vendored Node internals: primordials, errors, inspect, encoding bindings
  thinpack-cli/      the thinpack CLI, links no JS engine
  thinpack-rt/       the thinpack-rt runtime binary
  node/<name>/       one crate per Node built-in module
vendor/              patched copies of upstream crates (deno_core, deno_ffi, deno_v8, v8x)
```

The engine appears only in `thinpack-core`; module crates declare their own
`deno_core::Extension` and never depend on a backend; `capabilities.toml` is the single source
of module metadata, and `cargo xtask gen` generates the module table and the feature blocks.
A capability that needs per-isolate state receives `thinpack_core::IsolateInputs` in its
initializer; the packed FS mount and the FFI engine use that path.

## Development

```sh
cargo test
cargo test -p thinpack-rt --test node   # Node.js compatibility suite
cargo xtask gen --check                 # are generated files stale?
cargo xtask lint                        # fmt + clippy, the commit gate
cargo xtask size                        # build, strip, measure, record
cargo xtask apps                        # pack tests/apps, check the size baseline
cargo xtask dist                        # build, verify and archive a release
```

The compatibility suite ports Node.js' own tests unchanged and records known skips in
`tests/node/config.toml`; `tests/node/README.md` covers the layout, filters and the procedure
for adding a module.

## Known limits

- No full Intl data, no Temporal, no inspector protocol, no native backtraces;
  `Intl.Segmenter` on QuickJS is a lazy fallback implementation.
- The `jsc` template is macOS-only. It has no bundled engine: it runs on the system
  `JavaScriptCore.framework`. Its ES module support is a source rewriter aimed at flattened
  bundles; packed apps keep their file boundaries, so live bindings, cycles and namespace edge
  cases may differ from the QuickJS and V8 templates, and import attributes are unsupported.
- `fetch`, `WebSocket` and their types are Node's own undici, vendored from
  `deps/undici/undici.js`, so they behave as Node does; `EventSource`, `MessageEvent`,
  `CloseEvent` and `ErrorEvent` are absent. `Blob`, `File` and `reportError` are present.
  The bundle belongs to the `fetch` capability: an HTTP-only pack carries only the llhttp
  wasm payload extracted into `node:http`, not the bundle, while a
  `fetch` or `WebSocket` reference pulls the same sixteen-module closure and decodes the
  bundle at startup. Omitting the capability omits both globals. [Compatibility gaps](gaps.md) records what
  remains different.
- No N-API or native addon loading; code that needs native code goes through `node:ffi`.
- `crypto` key material covers RSA/RSA-PSS, EC (P-256/P-384/P-521/secp256k1), Ed25519 and
  X25519, plus secret keys from `createSecretKey` for HMAC, ciphers and HKDF. DSA, DH,
  Ed448 and X448 keys are rejected; encrypted PKCS#1/SEC1 PEM is not supported (PKCS#8
  encryption is).
- WebAssembly is always available and cannot be trimmed (WAMR on QuickJS, about 217 KB).
- The analyzer does not run ESM loader hooks. It determines formats from the file extension,
  package scope and syntax on disk. A hook cannot make a module analyzable or add it to the
  payload. To include a hook file inside the project, use `pack --include`. At runtime,
  `module.register` hooks run in their own loader isolate, as Node's loader thread does, so
  they may await I/O; the isolate has the app's capabilities and payload but its own globals.
  `module.registerHooks` stays synchronous on the app's isolate, and `require()` (including
  `require(esm)`) does not use the asynchronous hooks.
- `/app/**` comes from the payload: `fs` reads it, writes return `EROFS`, and modules under
  it are the bundled ones; anything outside `/app` goes to the host file system, including a
  module the app computes at run time (a project ESLint config, say), as it does under Node.
- The payload is rooted at the entry's nearest `package.json`. A required dependency resolved
  above that root (a hoisted monorepo install) fails the pack; an optional one is left out with
  a warning, and the app must handle its absence.
- Resolution is lexical, i.e. Node's `--preserve-symlinks` semantics: the payload records the
  path a symlink names, so `__dirname`, `require.resolve` and a linked package's own dependency
  lookup see the link location rather than the real path. Run Node with `--preserve-symlinks`
  when comparing a pnpm/workspace layout.
- The artifact's `process.argv` is `[execPath, invoked path, ...args]` (same shape as Node
  SEA); the only runtime switch is `--warm-cache`, every other argument goes to the app.
- `require.extensions` handlers are invoked for required files (a custom handler replaces the
  default compile/parse path); `require.main` and `require.cache` are available. See
  [Compatibility gaps](gaps.md) for the recorded gaps.
