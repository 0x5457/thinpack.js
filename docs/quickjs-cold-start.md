# QuickJS cold start on large bundles

The record for issues
[#15](https://github.com/0x5457/thinpack.js/issues/15) and
[#18](https://github.com/0x5457/thinpack.js/issues/18): a packed ESM CLI with a
large minified entry saturates one CPU core before its first statement runs.

## Root cause

`js_parse_postfix_expr` in the bundled quickjs-ng 0.15.1 computes the column of
an identifier by walking backwards from the token to the previous newline. It
did the same for property names in a call chain. Minified output has very long
lines, so every identifier rescans a large prefix of its line: parsing is
quadratic in line length. The 1.9 MB Kubernetes client bundle used for
reproduction has 171 lines, the longest 600 KB, and 132k identifiers; it spent
6.07 s in the parser.

quickjs-ng fixed this upstream in `0e19436` ("perf(parser): Avoid quadratic
identifier column lookup") by tracking the current line start in the parser
state and in each token. This tree carries that fix as a backport, extended to
the two remaining scans in the older snapshot (`js_parse_postfix_expr` property
names and `js_parse_error`). See `vendor/v8x/THINPACK.md`.

## Secondary quadratic

Even with short lines, functions with tens of thousands of declarations spent
seconds in repeated variable and scope-chain scans
(`find_var_in_child_scope`, `find_lexical_decl`, and the local-scope loop in
`resolve_scope_var`). Those lookups now use the existing variable hash table;
`with` and argument scopes keep the original chain walk. This is a Thinpack
change, not upstream. It is a different input shape from the reported bundle,
but generated code can reach it.

## Results

C harness parsing the same source, stock quickjs-ng 0.15.1 versus this tree:

| Input | Before | After |
| --- | --- | --- |
| 1.9 MB Kubernetes client bundle (171 lines) | 6070 ms | 58 ms |
| 490 KB single line, 50,000 call expressions | 3577 ms | 15 ms |
| 50,000 declarations in one function | 4803 ms | 11 ms |

Inside the packed runtime, `packed::minified-cold-startup` and
`packed::large-lexical-scope-startup` both exceed the 1 s budget on the stock
engine (the first times out at 10 s, the second takes 9.95 s) and pass in about
10 ms of parser time with the fixes. A packed 1.9 MB bundle from
`@kubernetes/client-node` 2.0.2 reaches its marker in 0.35 s wall clock.

Engine parity checks on this tree: quickjs-ng test scripts (0.15.1 and 0.18),
600 generated scoping programs, the 227-test Node.js compatibility suite, and
the 21 packed tests all match the stock engine or pass. `cargo xtask lint`
(rustfmt, QuickJS clippy, and clippy for the V8 core and template) and
`cargo test --workspace` pass.

## Remaining work

- The original application bundle from the reports is still unavailable. The
  replacement is the generated `CoreV1Api` client, which reproduces the long
  line and large function shapes.
- Validate on macOS ARM64 with the application's own version/help invocation.
- Offer the hash-table change to quickjs-ng; the column fix is already
  upstream, so this vendored delta shrinks to one change when the pinned
  quickjs-ng snapshot moves past `0e19436`.
