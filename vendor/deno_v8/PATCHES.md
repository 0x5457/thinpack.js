# deno_v8 0.5.0 (vendored)

The complete delta against the crates.io release is the `jsc` feature in `Cargo.toml` and its
arm in `lib.rs`. Everything else matches upstream.

## Why

`deno_core` depends on the `deno_v8` facade, not on v8x directly, and the facade only offers
`v8` (rusty_v8) and `quickjs` (v8x). The `jsc` feature lets the vendored `deno_core` compile
against v8x's system-JavaScriptCore backend without touching `serde_v8`: features unify across
both `deno_v8` edges, so `deno_core`'s `jsc = ["v8/jsc"]` is enough.

The feature maps to `v8x/system_jsc`, the macOS framework path. `v8x/jsc` (the vendored WebKit
build) is deliberately not used: the published v8x crate omits the WebKit tree and
`tools/setup_webkit.sh`, so that path cannot be built from this repository.

## Updating

1. Replace this directory with the new `deno_v8` release.
2. Add the `jsc` feature and its `lib.rs` arm again; the compile-time backend checks must list
   all three engines as mutually exclusive.
3. Run `cargo xtask lint`; on macOS it also compiles the JSC backend. Then build and test the
   JSC template (`cargo xtask dist --backend jsc`).
