# Benchmarks

[English quick start](../README.md) · [简体中文](../README.zh-CN.md) · [Usage and runtime reference](reference.md)

## Runtime template size

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

The app numbers below are the recording from `71dc765`. `cargo xtask apps` measures the
same apps from the pinned fixtures in `tests/apps/`, packs each with its own capability
closure, and holds the artifact and its gzip size to `tests/apps/baseline.toml` — the current
sizes. The Size workflow runs it on main, weekly, and on a pull request labelled `size`;
growth beyond 1% fails the run, and an intentional change re-records the baseline with
`cargo xtask apps --write-baseline`.

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
ships as its `bin`; for `pi` that is the statically linked `dist/cli.js`, because its `bin`
launcher hands off to an ES module bundle. bun gets the `bin` entry, SEA an esbuild
`--bundle --format=cjs` blob injected with postject. Sizes in MiB (gzip), startup for
`--version` as cold/warm median of seven runs:

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
