thinpack packs a Node.js app into a single executable: `deno_core` provides the engine,
`oxc` computes which built-in modules the app actually reaches, and only those are linked.

## What's new in 0.2.0

- **A third backend, and archives for all three.** `THINPACK_BACKEND=jsc` installs the `-jsc-`
pair for macOS, whose template links the system `JavaScriptCore.framework` instead of carrying an
engine; its module support is a source rewriter aimed at flattened bundles, so a module graph
that leans on exact ES module edge cases can behave differently (see *Known limits* in the
README). V8 archives are new as well — glibc Linux, macOS and Windows, `-v8-` in the name.
- **Static Linux archives.** `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` link
no libc and run on any distribution, Alpine included, for about 1 MiB more per packed artifact.
The installer picks them when the host cannot run the glibc build, `THINPACK_STATIC=1` forces
it, and CI refuses to publish a musl artifact that is not static-PIE.
- **`node:test`**, `node:punycode`, and the undici globals (`fetch`, `Headers`, `Request`,
`Response`, `FormData`, `WebSocket`) that Node v26 links in. The globals are lazy getters in
their own `fetch` capability: an app that never references them carries none of the bundle.
- **Crypto without vendored OpenSSL.** The key-material paths (RSA/RSA-PSS, EC, Ed25519,
X25519, PKCS#1/PKCS#8/SPKI/SEC1 and JWK, X.509 public keys, PKCS#8 encryption) use RustCrypto
instead of a bundled libcrypto, which takes about 2.7 MiB off an artifact that uses `node:crypto`
and removes Perl and Make from the build. Signing stays on ring. DSA, DH, Ed448/X448, encrypted
PKCS#1/SEC1 PEM and EC explicit parameters are recorded as gaps.
- **Faster cold start on large bundles.** Parsing a minified 1.9 MB bundle no longer scans back
to the start of the line per identifier, and functions with tens of thousands of declarations
no longer walk the scope chain per lookup.
- **Fixes:** ESM diagnostics survive packaging (dynamic-import, instantiation and CommonJS
compile errors), packed ESM entries load on Windows, `node:test` mock scoping and late
registration follow Node, `node:fs` reads the filesystem magic from `statfs`, and `node:timers`
uses the `Immediate` queue.
- **Security:** `hickory-resolver` 0.26 clears six Dependabot advisories in `node:dns`. `oxc`
0.159, `notify` 9 and `sha2` 0.11 are also on their current releases.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/0x5457/thinpack.js/main/scripts/install.sh | sh
```

Set `THINPACK_BACKEND=v8` for the V8 build. You can also unpack the archive for your
platform and keep `thinpack` and `thinpack-rt` in the same directory.

```sh
thinpack pack app.js -o app
./app
```

## Assets

| Target | Runs on |
| --- | --- |
| `x86_64-unknown-linux-musl` | 64-bit Linux, any distribution (static) |
| `aarch64-unknown-linux-musl` | 64-bit ARM Linux, any distribution (static) |
| `x86_64-unknown-linux-gnu` | 64-bit Linux, glibc 2.39+ (dynamic, smaller) |
| `aarch64-unknown-linux-gnu` | 64-bit ARM Linux, glibc 2.39+ (dynamic, smaller) |
| `aarch64-apple-darwin` | Apple Silicon macOS |
| `x86_64-apple-darwin` | Intel macOS |
| `x86_64-pc-windows-msvc` | 64-bit Windows |

Each archive includes `thinpack` and a runtime template with every built-in module. Archives
with `-v8-` in their names use V8 and are available for glibc Linux, macOS and Windows;
the `-jsc-` archives are the macOS pair that links the system framework. The default archives
use QuickJS. `thinpack pack --rebuild` from a source checkout builds a template with the app's
own capabilities.

The glibc artifacts are the smaller ones and are linked against the runner's glibc (Ubuntu 24.04,
2.39). The static musl artifacts link no libc and run anywhere, Alpine included, at about 1 MiB
more per artifact you pack. They also carry musl's resolver, which reads `/etc/hosts` and
`/etc/resolv.conf` only, so `dns.lookup` sees no NSS entries (LDAP, sssd, NIS) and no mDNS; and
`dlopen` through `node:ffi` is the one thing they cannot do at all, because a static binary has no
dynamic loader. `scripts/install.sh` picks between them by the host's libc, and `THINPACK_STATIC=1`
(Linux only) forces the static one.

Verify a download against `SHA256SUMS`.
