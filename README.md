# thinpack.js

Packs a Node.js app into a single executable file. `deno_core`/`v8x` provide the JS engine,
`oxc` static analysis computes the built-in module closure the app references, only those
implementations are linked, and the app's sources and assets are appended to the template.

The same apps land **9–25× smaller** than the other single-file packers: `pi` at 12.4 MiB
against bun's 115.2 and Node SEA's 157.8; `tsc` at 9.0 MiB against 109.6 and 149.8; `js-yaml`
at 5.8 MiB against 104.3 and 143.8.

- **Capability trimming**: `require`/`import` are resolved from the entry; unreferenced
  built-in modules never enter the binary.
- **Single-file artifact**: the payload is appended after the template and read by the runtime
  from itself; identical input produces identical bytes.
- **Two backends**: QuickJS by default, V8 on request, same code and separate size budgets.
- **Behavior parity**: user-visible modules follow the vendored Node sources, and the Node.js
  compatibility suite is part of the test run.

## Quick start

Requires Rust 1.99.1 (pinned by `rust-toolchain.toml`).

```sh
cargo build --release -p thinpack-rt -p thinpack-cli
./target/release/thinpack pack app.js -o app
./app
```

## Commands

| Command / option | Effect |
| --- | --- |
| `pack <entry> [-o <out>]` | analyze the module graph, trim capabilities, write a single-file artifact |
| `run <entry> [-- <args>...]` | run with a matching template without packing |
| `templates` | list the template cache |
| `--template <path>` | use the given runtime template |
| `--backend <quickjs\|v8>` | pick the template backend, `quickjs` by default |
| `--rebuild` `--source <checkout>` | build a template for this app's own capability set and cache it |
| `--asset <key>=<path>` | embed an asset, readable through `node:sea` |
| `--include <path>` | pack extra data files (`.d.ts`, templates); JavaScript is analyzed too |
| `--with <module>` `--without <module>` | override the derived capability set; ambient modules cannot be removed |
| `--strict` | a `require`/`import` that cannot be statically evaluated is an error; missing optional dependencies only warn |
| `--list-capabilities` | print the derived capability set and exit |

The payload records version, backend and capability-set hash, and the runtime verifies them at
startup. Templates live under `$THINPACK_CACHE` (then `$XDG_CACHE_HOME/thinpack`, then
`~/.cache/thinpack`) as `templates/<version>/<backend>/<caps-hash>`, and resolve `--template` →
`--rebuild` → exact hit → smallest superset → the `thinpack-rt` next to the CLI. `--rebuild`
comes before the cache so an app always packs against its own closure.

Compiled module bytecode is cached under `<cache-root>/bytecode`, which makes a warm start
about 29% faster (pi: 983 ms → 702 ms); `V82JSC_NO_BC_CACHE=1` disables it, and
`--warm-cache` pre-compiles the payload's modules at deploy time without running app code.

## Size

Small profile, stripped, on Linux x86_64. `gzip -9` is the same file after maximum
compression: the transfer size, not a format the runtime executes.

| Backend | Core (no built-in modules) | All modules |
| --- | ---: | ---: |
| QuickJS | 5,451,616 B (5.20 MiB), gzip 2,323,704 B (2.22 MiB) | 12,169,792 B (11.61 MiB), gzip 5,776,622 B (5.51 MiB) |
| V8 | 48,049,288 B (45.82 MiB), gzip 17,653,560 B (16.84 MiB) | 54,787,208 B (52.25 MiB), gzip 21,132,769 B (20.15 MiB) |

An artifact is this profile plus its payload (`pack --rebuild` builds with the same flags), so
it follows the capability closure the app resolves. The Node.js sources are embedded
brotli-compressed (`thinpack_js::js!`): 2.0 MB of JavaScript costs 0.43 MB of template and a
few milliseconds per start. Measured with `cargo xtask size` (build, strip, measure, write
`measurements/`).

## Compared with other single-file approaches

Same app (`@earendil-works/pi-coding-agent@1.5.0`, 2,377 modules, 16.5 MB of sources),
Linux x86_64, peak RSS in MB:

| Approach | Artifact | gzip | Peak RSS | Runs |
| --- | ---: | ---: | ---: | --- |
| thinpack (QuickJS, 34 capabilities) | 12.42 MiB | 7.16 MiB | 104 | yes |
| bun 1.4.4 `build --compile` | 115.24 MiB | 51.30 MiB | 84–91 | yes |
| Node 26.8.1 SEA (esbuild CJS blob) | 157.75 MiB | 46.94 MiB | — | no |

Both alternatives are runtime-dominated — 102 MiB and 143.5 MiB of engine — so their artifact
barely follows the app, while thinpack's payload is 2.6 MiB of compressed sources. Startup is
about 4× slower (pi `--help`: 777 ms against bun's 192 ms) because QuickJS parses slower; the
SEA artifact dies in `fileURLToPath(import.meta.url)` before `main`.

### Other CLIs

Each app gets a template for its own closure (`pack --rebuild`) from the entry the package
ships as its `bin`; bun gets the same entry, SEA an esbuild `--bundle --format=cjs` blob
injected with postject. Sizes in MiB (gzip), startup for `--version` as cold/warm median of
seven runs:

| Program | thinpack | bun | Node SEA | thinpack startup | SEA startup |
| --- | ---: | ---: | ---: | ---: | ---: |
| `typescript@5.9.3` (`tsc`) | 9.00 (5.65) | 109.63 (50.19) | 149.82 (45.35) | 578 / 347 ms | 77 ms |
| `prettier@3.10.1` | 7.41 (4.14) | 110.04 (50.85) | 143.57 (44.32) | 235 / 155 ms | — |
| `eslint@10.15.0` | 6.93 (3.60) | 107.15 (49.78) | 147.25 (44.96) | 288 / 186 ms | 64 ms |
| `terser@5.51.2` | 6.06 (2.91) | 105.38 (49.48) | 144.94 (44.57) | 226 / 145 ms | 35 ms |
| `js-yaml@5.5.0` | 5.80 (2.69) | 104.29 (49.28) | 143.75 (44.37) | 150 / 100 ms | — |
| `pi-coding-agent@1.5.0` | 12.42 (7.16) | 115.24 (51.30) | 157.75 (46.94) | 983 / 702 ms | — |

The SEA binaries start 4–8× faster at ~13–25× the size. thinpack runs all six as shipped
(`tsc` and `prettier` need `--include` for their `lib`/`internal` files). bun needs a copy of
`bin/tsc` and `bin/terser` named `*.js`, `--external jiti` for `eslint` and a wrapper for
`prettier`, but `bun <entry>` runs all six unchanged — the limit is `--compile`'s static
analysis. The SEA blob has no `import.meta` and resolves `import()` against built-ins only, so
only `terser` works; `tsc` and `eslint` print `--version` but cannot do their job, and
`prettier`, `js-yaml` and `pi` fail while loading.

## Built-in modules

Ambient (every template): `console`, `process`, `timers`, `buffer`, `url`, plus `util` because
`console` formats through it.

On demand: `assert`, `async_hooks`, `child_process`, `constants`, `crypto`,
`diagnostics_channel`, `dns`, `events`, `ffi`, `fs`, `http`, `http2`, `https`, `inspector`,
`module`, `net`, `os`, `path`, `perf_hooks`, `querystring`, `readline`, `sea`, `sqlite`,
`stream`, `string_decoder`, `tls`, `tty`, `util`, `v8`, `vm`, `worker_threads`, `zlib`.
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
  thinpack-js/       proc macro: brotli-compress the embedded JavaScript at build time
  thinpack-node-internals/  vendored Node internals: primordials, errors, inspect, encoding bindings
  thinpack-cli/      the thinpack CLI, links no JS engine
  thinpack-rt/       the thinpack-rt runtime binary
  node/<name>/       one crate per Node built-in module
vendor/              patched copies of upstream crates (deno_core, deno_ffi)
```

The engine appears only in `thinpack-core`; module crates declare their own
`deno_core::Extension` and never depend on a backend; `capabilities.toml` is the single source
of module metadata, and `cargo xtask gen` generates the module table and the feature blocks.

## Development

```sh
cargo test
cargo test -p thinpack-rt --test node   # Node.js compatibility suite
cargo xtask gen --check                 # are generated files stale?
cargo xtask lint                        # fmt + clippy, the commit gate
cargo xtask size                        # build, strip, measure, record
```

The compatibility suite ports Node.js' own tests unchanged and records known skips in
`tests/node/config.toml`; `tests/node/README.md` covers the layout, filters and the procedure
for adding a module.

## Known limits

- No full Intl data, no Temporal, no inspector protocol, no native backtraces;
  `Intl.Segmenter` on QuickJS is a lazy fallback implementation.
- No N-API or native addon loading; code that needs native code goes through `node:ffi`.
- WebAssembly is always available and cannot be trimmed (WAMR on QuickJS, about 217 KB).
- ESM loader hooks run inside synchronous checkpoints, so hooks that wait on I/O fail.
- `/app/**` comes from the payload: `fs` reads it, writes return `EROFS`; anything outside
  `/app` goes to the host file system.
- The artifact's `process.argv` is `[execPath, invoked path, ...args]` (same shape as Node
  SEA); the only runtime switch is `--warm-cache`, every other argument goes to the app.
- `require.extensions` handlers are not invoked and `module.parent` is not implemented;
  `require.main` and `require.cache` are. See `docs/gaps.md` for the recorded gaps.

## License

MIT. The vendored Node.js sources are MIT too; `vendor/` holds patched copies of upstream MIT
code, with the reasons recorded in `vendor/deno_core/PATCHES.md`.
