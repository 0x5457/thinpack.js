# thinpack.js

Packs a Node.js app into a single executable file. `deno_core` and `v8x` provide the JS
engine, `oxc` static analysis determines which built-in modules the app actually references,
only those implementations are linked, and the sources, dependencies and embedded assets are
appended to the end of the runtime template.

- **Capability trimming**: resolve `require`/`import` from the entry, compute the built-in
  module closure; unreferenced modules never enter the binary.
- **Single-file artifact**: the payload is appended after the template and read by the runtime
  from itself; identical input produces identical bytes.
- **Two backends**: QuickJS by default, V8 on request; same code, with a separate size budget
  per backend.
- **Behavior parity**: user-visible modules follow the vendored Node sources, and acceptance
  tests take their paths from the target npm packages.

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
| `--rebuild` `--source <checkout>` | build a template for this app's own capability set with cargo and cache it |
| `--asset <key>=<path>` | embed an asset, readable through `node:sea` |
| `--include <path>` | pack extra data files (`.d.ts`, templates); JavaScript is analyzed too |
| `--with <module>` `--without <module>` | override the derived capability set; ambient modules cannot be removed |
| `--strict` | a `require`/`import` that cannot be statically evaluated is an error; missing optional dependencies only warn |
| `--list-capabilities` | print the derived capability set and exit |

## Templates and cache

Template identity is the triple of version, backend and capability-set hash. The payload
records all three, the runtime verifies them at startup, and a mismatch reports both
identities and asks for a re-pack.

The cache key is `<version>/<backend>/<caps-hash>`, resolved under `$THINPACK_CACHE`, then
`$XDG_CACHE_HOME/thinpack`, then `~/.cache/thinpack`. Template selection: `--template` →
`--rebuild` → exact hit → smallest superset covering the required capabilities → the
`thinpack-rt` sitting next to the CLI in a development tree. `--rebuild` comes before the
cache so that an app always packs against its own closure instead of a larger template that
happens to be installed.

The runtime caches compiled module bytecode under `<cache-root>/bytecode`; a warm start is
about 29% faster than a cold one (pi: 983 ms → 702 ms). `V82JSC_NO_BC_CACHE=1` disables it,
deleting the directory clears it. An artifact can be pre-warmed at deploy time with
`--warm-cache`: it compiles only the payload's modules and runs no application code.

## Size

small profile, stripped. The `gzip -9` numbers are the same file after maximum compression:
that is the transfer size, not a format the runtime executes.

| Backend | Core (no built-in modules) | All modules |
| --- | ---: | ---: |
| QuickJS | 5,451,616 B (5.20 MiB), gzip 2,323,704 B (2.22 MiB) | 12,169,792 B (11.61 MiB), gzip 5,776,622 B (5.51 MiB) |
| V8 | 48,049,288 B (45.82 MiB), gzip 17,653,560 B (16.84 MiB) | 54,787,208 B (52.25 MiB), gzip 21,132,769 B (20.15 MiB) |

Artifact size follows the capability closure the app resolves, not the full set above. The
only measurement entry point is `cargo xtask size` (build, strip, measure, write
`measurements/`). `pack --rebuild` builds templates with the same flags, so an artifact is
this profile plus its payload. The Node.js sources themselves are embedded
brotli-compressed (`thinpack_js::js!`) and decode when a module first loads: 2.0 MB of
JavaScript costs 0.43 MB of template and a few milliseconds per start.

## Compared with other single-file approaches

Same app (`@earendil-works/pi-coding-agent@1.5.0`'s `dist/cli.js`, 2,377 modules, 16.5 MB of
sources), Linux x86_64. Artifact sizes are MiB, peak RSS in MB:

| Approach | Artifact | Peak RSS | Runnable |
| --- | ---: | ---: | --- |
| thinpack (QuickJS, 34 capabilities) | 12.42 MiB (gzip 7.16 MiB) | 104 MB | yes |
| bun 1.4.4 `build --compile` | 115.24 MiB (gzip 51.30 MiB) | 84–91 MB | yes |
| Node 26.8.1 SEA (esbuild CJS bundle) | 157.75 MiB (gzip 46.94 MiB) | — | no |

- The size gap comes from the runtime base: the app sources compress to 2.6 MiB, while the
  other two carry a 102 MiB and a 143 MiB general-purpose runtime.
- Startup is about 4× slower (QuickJS parses less throughput than JSC/V8): pi `--help` takes
  777 ms against bun's 192 ms. The Node SEA artifact dies in `fileURLToPath(import.meta.url)`
  before `main`, so it serves as a size reference only.

### Other CLIs

Each app gets a template built for its own capability closure (`pack --rebuild`), from the
entry the package ships as its `bin`; bun 1.4.4 `build --compile` and Node 26.8.1 SEA get the
same entry. A SEA blob runs as CommonJS, so that side bundles the entry with esbuild 0.25.10
(`--bundle --platform=node --format=cjs`) and injects it with postject 1.0.0-alpha.6.
Linux x86_64:

| Program | thinpack | bun | Node SEA |
| --- | ---: | ---: | ---: |
| `typescript@5.9.3` (`tsc`) | 9.00 MiB (gzip 5.65 MiB) | 109.63 MiB (gzip 50.19 MiB) | 149.82 MiB (gzip 45.35 MiB) |
| `prettier@3.10.1` | 7.41 MiB (gzip 4.14 MiB) | 110.04 MiB (gzip 50.85 MiB) | 143.57 MiB (gzip 44.32 MiB) |
| `eslint@10.15.0` | 6.93 MiB (gzip 3.60 MiB) | 107.15 MiB (gzip 49.78 MiB) | 147.25 MiB (gzip 44.96 MiB) |
| `terser@5.51.2` | 6.06 MiB (gzip 2.91 MiB) | 105.38 MiB (gzip 49.48 MiB) | 144.94 MiB (gzip 44.57 MiB) |
| `js-yaml@5.5.0` | 5.80 MiB (gzip 2.69 MiB) | 104.29 MiB (gzip 49.28 MiB) | 143.75 MiB (gzip 44.37 MiB) |
| `pi-coding-agent@1.5.0` | 12.42 MiB (gzip 7.16 MiB) | 115.24 MiB (gzip 51.30 MiB) | 157.75 MiB (gzip 46.94 MiB) |

What each artifact then does with `--version` and with its own workload:

| Program | thinpack | bun | Node SEA |
| --- | --- | --- | --- |
| `tsc` | yes, `--include …/lib`: compiles `hello.ts` (4.3 s of type-checking) | yes, from a copy of `bin/tsc` named `*.js` | `--version` only, prints `5.9.3`; no type-checking: `lib/*.d.ts` are not in the blob |
| `prettier` | yes, `--include …/internal` | yes, through a wrapper importing `internal/legacy-cli.mjs` | no: `bin` imports through `new Function`, and `internal/legacy-cli.mjs` needs `createRequire(import.meta.url)` |
| `eslint` | yes, `--version` | yes, with `--external jiti` | `--version` only, prints `10.15.0`; no linting: the flat config loads through `import()`, which a blob resolves against built-in modules only (bundled with `jiti` external) |
| `terser` | yes, minifies | yes, from a copy of `bin/terser` named `*.js` | yes, minifies |
| `js-yaml` | yes, YAML to JSON | no | no: `new URL('../package.json', import.meta.url)` at module top level |
| `pi` | yes, `--version` prints `1.5.0` | yes, `--version` prints `0.0.0` | no: `fileURLToPath(import.meta.url)` |

Startup (`--version`, median of seven runs, ms); thinpack cold and warm bytecode cache, SEA
where the artifact starts at all:

| Program | thinpack cold | thinpack warm | Node SEA |
| --- | ---: | ---: | ---: |
| `tsc` | 578 ms | 347 ms | 77 ms |
| `prettier` | 235 ms | 155 ms | — |
| `eslint` | 288 ms | 186 ms | 64 ms |
| `terser` | 226 ms | 145 ms | 35 ms |
| `js-yaml` | 150 ms | 100 ms | — |
| `pi` | 983 ms | 702 ms | — |

The SEA binaries start 4–7× faster than thinpack's, at 17–24× the size.

- As shipped, `--compile` fails to produce a working binary on every row but `pi`, two of them
  silently: an extensionless `bin/tsc` or `bin/terser` is embedded byte for byte as an asset,
  leaving a binary that exits 0 and does nothing; `bin/prettier.cjs` reaches
  `internal/legacy-cli.mjs` through `new Function`, so the bundle dies on `Cannot find module`;
  `eslint` needs its optional peer `jiti` excluded to build; `js-yaml` reads `../package.json`
  at module top level, which lands in `/$bunfs` and is the one failure no flag fixes.
- `bun <entry>` runs all of these CLIs unchanged, so the limit is `--compile`'s static
  analysis, not the runtime.
- bun's artifacts are runtime-dominated: the bun binary alone is 102 MiB, so every row sits
  between 104 and 116 MiB regardless of the app.
- SEA runs one CommonJS blob: `import.meta` is undefined and `import()` inside the blob
  resolves against built-in modules only. That is why five of the six rows stop short —
  `prettier`, `js-yaml` and `pi` while loading, `eslint` when it loads the flat config, `tsc`
  when it looks for its lib files. Assets do not help: a blob reaches them through
  `node:sea`, never through `fs`.
- SEA artifacts are runtime-dominated. The injected Node binary is 143.5 MiB unstripped, so
  `js-yaml` (248 KiB of app) lands at 143.75 MiB and `pi` (14.19 MiB of app) at 157.75 MiB,
  and gzip tracks the runtime at 44–47 MiB, below bun's 49–51 MiB. Stripping the injected
  file breaks it (SIGILL); stripping Node before injection gives `terser` 113.63 MiB (gzip
  39.21 MiB), 19× its thinpack artifact.

## Built-in modules

Ambient (every template contains them): `console`, `process`, `timers`, `buffer`, `url`.
`util` is in every template as well, because `console` formats through `util.format` and
depends on it.

On demand: `assert`, `async_hooks`, `child_process`, `constants`, `crypto`,
`diagnostics_channel`, `dns`, `events`, `ffi`, `fs`, `http`, `http2`, `https`, `inspector`,
`module`, `net`, `os`, `path`, `perf_hooks`, `querystring`, `readline`, `sea`, `sqlite`,
`stream`, `string_decoder`, `tls`, `tty`, `util`, `v8`, `vm`, `worker_threads`, `zlib`.
Subpaths (`fs/promises`, `stream/web`, `path/posix`, `util/types`, …) ship with their module.

`http2` and `sqlite` are marked optional: a target package probing for them does not fail the
pack. A module that was not compiled in throws `ERR_UNKNOWN_BUILTIN_MODULE` on `require`,
matching Node.

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
  thinpack-cli/      the thinpack CLI, links no JS engine
  thinpack-rt/       the thinpack-rt runtime binary
  node/<name>/       one crate per Node built-in module
vendor/              patched copies of upstream crates (deno_core, deno_ffi)
```

Structure carries the constraints: the engine appears only in `thinpack-core`; module crates
declare their own `deno_core::Extension` and never depend on a backend; `capabilities.toml`
is the single source of module metadata, and `cargo xtask gen` generates the module table
and the feature blocks.

## Development

```sh
cargo test
cargo test -p thinpack-rt --test node   # Node.js compatibility suite
cargo xtask gen --check   # are generated files stale?
cargo xtask size          # build, strip, measure, record
```

The compatibility suite ports Node.js' own tests unchanged and records known
skips in `tests/node/config.toml`; see `tests/node/README.md` for the layout,
filters (`cargo test ... -- path`) and the procedure for adding a module.

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

MIT. The vendored Node.js sources are MIT too; `vendor/` holds patched copies of upstream
MIT code, with the reasons recorded in `vendor/deno_core/PATCHES.md`.
