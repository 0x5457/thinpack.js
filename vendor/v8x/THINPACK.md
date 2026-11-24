# Thinpack's v8x

This is the published `v8x 149.4.0-rc.4` crate with two changes to
`vendor/quickjs-ng/quickjs.c` plus the JSC shim changes in `src/jsc`. The root
`Cargo.toml` redirects the crates.io dependency here:

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

## 3. Warning-free JSC shim

The JSC port mirrors the v8 C ABI: JSC's generated `kJSType*` constants appear
in `match` patterns and the `extern "C"` entry points hand private ABI structs
across a `pub(crate)` boundary. `src/jsc/mod.rs` allows `non_upper_case_globals`
and `private_interfaces` for the whole module, and `src/jsc/init.rs` allows
`unused_mut` on the system-framework build, which pushes no options. Without
these, a `build-warnings = deny` environment (CI) fails on the macOS JSC job.
No runtime behavior changes.

## 4. Undefined module-request specifiers

`v8__ModuleRequest__GetSpecifier` returned a null pointer when JSC left the
specifier undefined (for example, a NUL in the import text). rusty_v8's
`ModuleRequest::get_specifier` unwraps that pointer and aborts. Return the
undefined value instead, as the QuickJS adapter does, so deno_core reports
"the engine did not provide a string" rather than panicking.

## 5. Own-only property enumeration

JSC's public C API has no own-only enumeration: `JSObjectCopyPropertyNames`
returns enumerable names **including inherited ones**, so `Object.keys`,
`util.inspect` and `assert.deepStrictEqual` saw every `Buffer.prototype`
method as a property of a Buffer on the JSC backend. `GetOwnPropertyNames`
now collects the own names through `Object.getOwnPropertyNames` /
`getOwnPropertySymbols` and applies `PropertyFilter` with
`getOwnPropertyDescriptor` and `propertyIsEnumerable`; `GetPropertyNames`
uses that path for `OwnOnly` and keeps the chain walk for
`IncludePrototypes`. `jsstr_is_array_index` also reads JSC's 8-bit strings,
which have no UTF-16 buffer, so `SkipIndices` works on ASCII index names.

## 6. UTF-8 writes use UTF-16 units and honor the flag

`JSStringGetUTF8CString` truncates a string at an unpaired surrogate, so a
`Buffer.from`/`Buffer#write` of text containing one lost the tail. Encode from
the UTF-16 units instead, replacing unpaired surrogates with U+FFFD (the USV
semantics `kReplaceInvalidUtf8` asks for) and stopping before a character that
does not fit. `WriteUtf8` also wrote a NUL unconditionally; V8 only does that
with `WriteFlags::kNullTerminate` (bit 1), so an exact-size write overran its
slice.

## 7. Top-level await inside a block

`has_top_level_await` only accepted `await` at brace depth 0, so a module with
`await` inside a top-level `try`/`if`/loop was wrapped in a synchronous
function and failed to parse. Before building the wrapper, run
`JSCheckScriptSyntax` on the synchronous body; a module that does not parse
synchronously is wrapped in an async function. This also removes the async
method false positives the depth check was guarding against.

## 8. ESM link checks and import locations

The rewriter turns imports into destructuring, which silently yields
`undefined` for a missing export. `rewrite_es_module` now records
(specifier, imported name, line) for every named/default import and
named re-export; `InstantiateModule` checks those against the dependency's
export list before evaluating it, and rejects with the V8 message and a real
stack pointing at the importer's import line. `export *` dependencies are
exempt because the rewriter cannot enumerate their names. The original line
starts are also stored so `SourceOffsetToLocation` reports the offsets deno
uses in import errors, and an `Errored` module can be re-instantiated (the
dynamic-import loop imports the failing graph twice).

## 9. Hoisted declarations after a statement

A logical line like `globalThis.seen = true; export const present = 1;` began
with a statement, so the line-oriented rewriter never recognized the export
and left the keyword inside the generated function. Split at `;` when the
remainder starts with `import`/`export`, then process the declaration normally.

## 10. Evaluation errors become rejections once

A throwing synthetic evaluation step (a CommonJS module with a TDZ error, for
one) leaves a pending exception. `Evaluate` turns it into a rejected promise so
the importer or `import()` sees the rejection, and marks the rejection handled
so JSC does not also report it as uncaught. Source modules keep the same
contract through `eval_module_source`.

## Validation

Changes 1 and 2 are covered by the differential runs below, comparing the stock
quickjs-ng 0.15.1 source against this tree; change 4 is covered by
`packed::esm-invalid-specifier-raw-nul`, change 5 by the Node `buffer`, `util`
and `assert` suites, change 7 by the Node dynamic-import cases, and changes 8
and 9 by `packed::esm-link-error`, `packed::esm-large-link-error` and
`packed::esm-dynamic-link-error`:

- quickjs-ng 0.15.1 and 0.18 test scripts: identical output and exit status.
- 600 generated programs with nested blocks, shadowing, `with`, catch
  bindings, default parameters, eval, and synchronous function redeclaration:
  identical output and exit status.
- The thinpack Node.js compatibility suite: 227 passed, 0 failed.
- The packed suite (including both startup guards): 21 passed, 0 failed.

## Updating

1. Replace this directory with the new published crate (without `Cargo.lock`
   or `.cargo-ok`; those are not part of the vendored source).
2. Apply `THINPACK.patch`; resolve rejects by rebasing the changes onto the
   new `quickjs.c` and JSC files.
3. Keep the upstream column fix if the new quickjs-ng already contains it;
   drop that hunk from the patch instead of reverting the vendored source.
4. Rerun the startup tests and `cargo xtask lint`. On macOS that also compiles
   change 3 through the JSC backend.
