# deno_core 0.413.0 (vendored)

This directory is a source copy of deno_core 0.413.0 kept for a single patch: a synchronous
module evaluation entry point for CJS `require(esm)` (the Node 22.12+ behavior).

## Differences from upstream

1. `modules/require_esm.rs` (new): `require_esm(scope, specifier)` drives
   `RecursiveModuleLoad` synchronously (`SideModuleKind::Sync`), instantiates and calls
   `mod_evaluate_sync`, then returns the module namespace. TLA is reported by
   `mod_evaluate_sync` as `CoreErrorKind::TLA`; a module that is already being evaluated
   (a cycle) returns its namespace directly.
2. `modules/mod.rs`: declares `mod require_esm;` and `pub use require_esm::require_esm;`.
3. `lib.rs`: `pub use crate::modules::require_esm;`.
4. `runtime/mod.rs`: `mod jsrealm;` → `pub(crate) mod jsrealm;`, which the patch needs for
   `JsRealm::module_map_from`.
5. `modules/map.rs`: when instantiation fails and V8 has no pending exception, return a
   constructed JS Error instead of letting `tc_scope.exception().unwrap()` abort the process
   (a dynamic `import()` with a computed specifier no longer trips a debug_assert and goes
   through full loader resolution).
6. `Cargo.toml`: `[lints.rust] deprecated = "allow"`. The upstream code predates the
   dependency versions we pin, so those deprecation warnings belong to upstream and not to
   this project's lint gate.
7. `extensions.rs`: `debug_assert!(code.is_ascii())` removed from both `ExtensionFileSource`
   arms, together with the now-unused `find_non_ascii`. Embedded sources are JavaScript text
   and the engine decodes them as UTF-8; undici's bundle carries an author name in a licence
   header (`formdata-polyfill`, "Jimmy Wärting"), and rewriting a vendored file to satisfy a
   debug assertion trades a real attribution for a cosmetic one. Debug builds of the whole
   test suite run this path, so the assertion was load-bearing for vendored sources.

Every other file matches upstream. Upgrade procedure: unpack the target version from
crates.io, apply the changes above, update the path dependency in `Cargo.toml`, then run
`cargo xtask lint` and `cargo test`, and re-pack the target apps to verify the artifacts.

## Why fork

`deno_core`'s `ModuleMap` is `pub(crate)` and there is no public synchronous module
evaluation interface. Writing a second one at the v8 layer would produce a second module
cache, so the same file could be evaluated once by `import` and once by `require`; the patch
makes both share one `ModuleMap`, the same approach Deno's Node compatibility layer takes.
This is the only upstream dependency we fork; module crates still do not depend on it.
