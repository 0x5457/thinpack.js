# Thinpack's v8x

This is the published `v8x 149.4.0-rc.4` crate with two changes to
`vendor/quickjs-ng/quickjs.c`. The root `Cargo.toml` redirects the crates.io
dependency here:

```toml
[patch.crates-io]
v8x = { path = "vendor/v8x" }
```

The complete delta against the published crate is in `THINPACK.patch`; apply it
with `patch -p1` from this directory after replacing the tree with a new
release.

## 1. O(1) token columns (upstream backport)

`js_parse_postfix_expr` derived the column of an identifier (and of a property
name) by walking back from the token to the previous newline. On minified
bundles with multi-hundred-kilobyte lines this makes cold parsing quadratic:
an 1.9 MB Kubernetes client bundle took 6.1 s to parse and an 11.65 MB bundle
takes minutes. The same scan also preceded metadata for every identifier, so
it dominated startup on all large minified inputs.

The fix is upstream quickjs-ng commit
`0e194369a8650487c981e6128079ebac249a0769` ("perf(parser): Avoid quadratic
identifier column lookup"): track the current line start in `JSParseState`
(and the token's line start in `JSToken`), update it at every newline site,
save/restore it across `peek_token`, and use it for identifier and error
columns. The two remaining scans in this older quickjs-ng snapshot
(`js_parse_postfix_expr` property names and `js_parse_error`) use the same
tracked pointer.

Measured with a C harness on the 1.9 MB bundle: 6070 ms before, 58 ms after.
`packed::minified-cold-startup` is the regression guard.

## 2. Hash-table lookups for large scopes

Custom change, not in upstream quickjs-ng at the time of writing.
`find_var_in_child_scope`, `find_lexical_decl`, and the local-scope loop in
`resolve_scope_var` scanned the variable and scope chains repeatedly while
parsing. Functions with tens of thousands of declarations therefore took
quadratic time even when every line was short. The change reuses the existing
`vars_htab` (normally only used by `find_var`) for these lookups. `with`
scopes and argument scopes keep the chain walk, because their shadowing rules
depend on declaration order and on the argument-scope sentinel.

`packed::large-lexical-scope-startup` parses 50,000 declarations in one
function; it drops from about 9.95 s to 11 ms inside the packed runtime.

## Validation

Both changes are covered by the differential runs below, comparing the stock
quickjs-ng 0.15.1 source against this tree:

- quickjs-ng 0.15.1 and 0.18 test scripts: identical output and exit status.
- 600 generated programs with nested blocks, shadowing, `with`, catch
  bindings, default parameters, eval, and synchronous function redeclaration:
  identical output and exit status.
- The thinpack Node.js compatibility suite: 227 passed, 0 failed.
- The packed suite (including both startup guards): 21 passed, 0 failed.

## Updating

1. Replace this directory with the new published crate (without `Cargo.lock`
   or `.cargo-ok`; those are not part of the vendored source).
2. Apply `THINPACK.patch`; resolve rejects by rebasing the two changes onto the
   new `quickjs.c`.
3. Keep the upstream column fix if the new quickjs-ng already contains it;
   drop that hunk from the patch instead of reverting the vendored source.
4. Rerun the startup tests and `cargo xtask lint`.
