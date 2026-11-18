thinpack packs a Node.js app into a single executable: `deno_core` provides the engine,
`oxc` computes which built-in modules the app actually reaches, and only those are linked.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/0x5457/thinpack.js/main/scripts/install.sh | sh
```

Or unpack the archive for your platform and keep `thinpack` and `thinpack-rt` in the same
directory — the CLI loads its runtime template from next to itself.

```sh
thinpack pack app.js -o app
./app
```

## Assets

| Target | Runs on |
| --- | --- |
| `x86_64-unknown-linux-gnu` | 64-bit Linux, glibc 2.39+ |
| `aarch64-unknown-linux-gnu` | 64-bit ARM Linux, glibc 2.39+ |
| `aarch64-apple-darwin` | Apple Silicon macOS |
| `x86_64-apple-darwin` | Intel macOS |
| `x86_64-pc-windows-msvc` | 64-bit Windows |

Every archive carries the same two binaries: `thinpack` (the packer, links no JS engine) and
`thinpack-rt` (the runtime template, QuickJS backend, every built-in module). The template is
a superset, so any app packs; `thinpack pack --rebuild` from a source checkout builds one that
carries only that app's capability set, which is where the smallest artifacts come from.

The Linux archives are linked against the runner's glibc (Ubuntu 24.04, 2.39). On an older
distribution, build from source.

Verify a download against `SHA256SUMS`.
