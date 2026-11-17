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
| `--rebuild` `--source <checkout>` | build a template with cargo and cache it |
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
exact hit → smallest superset covering the required capabilities → `--rebuild` → the
`thinpack-rt` sitting next to the CLI in a development tree.

The runtime caches compiled module bytecode under `<cache-root>/bytecode`; a warm start is
about 23% faster than a cold one (pi: 906 ms → 700 ms). `V82JSC_NO_BC_CACHE=1` disables it,
deleting the directory clears it. An artifact can be pre-warmed at deploy time with
`--warm-cache`: it compiles only the payload's modules and runs no application code.

## Size

small profile, stripped. The `gzip -9` numbers are the same file after maximum compression:
that is the transfer size, not a format the runtime executes.

| Backend | Core (no built-in modules) | All modules |
| --- | ---: | ---: |
| QuickJS | 5,527,968 B (5.27 MiB), gzip 2,292,655 B (2.19 MiB) | 13,386,304 B (12.77 MiB), gzip 5,615,822 B (5.36 MiB) |
| V8 | 48,123,016 B (45.89 MiB), gzip 17,621,969 B (16.81 MiB) | 56,007,816 B (53.41 MiB), gzip 20,964,500 B (19.99 MiB) |

Artifact size follows the capability closure the app resolves, not the full set above. The
only measurement entry point is `cargo xtask size` (build, strip, measure, write
`measurements/`).

## Compared with other single-file approaches

Same app (`@earendil-works/pi-coding-agent@1.5.0`'s `dist/cli.js`, 2,377 modules, 19.4 MB
unpacked), Linux x86_64. Artifact sizes are MiB, peak RSS in MB:

| Approach | Artifact | Peak RSS | Runnable |
| --- | ---: | ---: | --- |
| thinpack (QuickJS, 34 capabilities) | 14.31 MiB (gzip 7.18 MiB) | 105 MB | yes |
| bun 1.4.4 `build --compile` | 115.1 MiB (gzip 51.3 MiB) | 82–90 MB | yes |
| Node 26.8.1 SEA (esbuild CJS bundle) | 157.7 MiB (gzip 46.9 MiB) | — | no |

- The size gap comes from the runtime base: the app sources compress to 2.6 MiB, while the
  other two carry a 102 MiB and a 143 MiB general-purpose runtime.
- Startup is 3–4× slower (QuickJS parses less throughput than JSC/V8); the Node SEA artifact
  fails resource resolution via `import.meta.url`, so it serves as a size reference only.

### Other CLIs

Each app gets a template built for its own capability closure (`pack --rebuild`); bun is
1.4.4 `build --compile`. Sizes in MiB; thinpack cells also give gzip -9. Linux x86_64.

| Program | thinpack | bun | thinpack runs | bun runs |
| --- | ---: | ---: | --- | --- |
| `typescript@5.9.3` (`tsc`) | 10.57 MiB (gzip 5.68 MiB) | 104.1 MiB | yes, compiles `hello.ts` (4.8 s) | no: exits 0, prints nothing |
| `prettier@3.10.1` | 9.01 MiB (gzip 4.16 MiB) | 104.1 MiB | yes, with `--include …/internal` | no: `Cannot find module '../internal/legacy-cli.mjs'` |
| `eslint@10.15` | 8.50 MiB (gzip 3.62 MiB) | build fails (optional `jiti`) | yes, `--version` | — |
| `terser@5.51.2` | 8.05 MiB (gzip 3.20 MiB) | 104.1 MiB | yes, minifies | no: exits 0, prints nothing |
| `js-yaml@5.5.0` | 7.82 MiB (gzip 2.98 MiB) | 104.3 MiB | yes, YAML to JSON | no |
| `pi-coding-agent@1.5.0` | 14.31 MiB (gzip 7.18 MiB) | 115.1 MiB | yes, `--version` prints `1.5.0` | yes (`--version` prints `0.0.0`) |

Startup of the thinpack artifacts (`--version`, median of seven runs, ms), cold and warm
bytecode cache:

| Program | cold | warm |
| --- | ---: | ---: |
| `tsc` | 522 ms | 348 ms |
| `prettier` | 218 ms | 157 ms |
| `eslint` | 262 ms | 182 ms |
| `terser` | 208 ms | 140 ms |
| `js-yaml` | 135 ms | 97 ms |
| `pi` | 906 ms | 700 ms |

- bun's artifacts are runtime-dominated: the bun binary alone is 102 MiB, so every row lands
  near 104 MiB regardless of the app.
- bun bundles `tsc`, `terser` and `prettier` without an error, but the resulting binaries do
  not run the CLI (silent exit 0, or a missing `$bunfs` module). Their rows above are sizes
  only.
- `tsc`'s 4.8 s is type-checking work, not startup.
- `prettier` and `tsc` need `--include`: the former hides its import behind `new Function`,
  the latter reads `lib/*.d.ts` through `fs`.

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
cargo xtask gen --check   # are generated files stale?
cargo xtask size          # build, strip, measure, record
```

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
