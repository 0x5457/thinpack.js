## What's new in 0.3.0

- **Smaller QuickJS executables.** A one-line Hello World is 4.03 MiB on Linux x64
  with `--rebuild`, down from 5.37 MiB.
- **Better Node.js compatibility.** HTTP/1.1 now uses Node's llhttp parser.
  This release adds UDP sockets, improves HTTP timeout behavior, fixes piped
  stdin, and supports JSON IPC in `child_process.spawn`.
- **More predictable module loading.** `require.resolve(request, { paths })`
  uses the supplied search roots. Package exports use the most specific match.
  The packer can strip TypeScript types and follows more static module paths.

If a packed Babel 7 or 8 app uses a package-name preset, include the Babel config
with the preset and select the packed config at runtime. See the
[reference](https://github.com/thinpack-js/thinpack.js/blob/v0.3.0/docs/reference.md).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/thinpack-js/thinpack.js/v0.3.0/scripts/install.sh | sh
```

Archives include a CLI and runtime template. QuickJS is the default.
Set `THINPACK_BACKEND=v8` for V8 or `THINPACK_BACKEND=jsc` on macOS.
Set `THINPACK_STATIC=1` for a static Linux archive. Manual downloads can be
checked against `SHA256SUMS`.
