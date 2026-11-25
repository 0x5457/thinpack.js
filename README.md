<p align="center">
  <img src="docs/assets/thinpack-logo.png" alt="Thinpack.js logo" width="144">
</p>

<h1 align="center">thinpack.js</h1>

<p align="center">Pack a Node.js program into a single executable. No separate Node.js, Bun, or npm install is needed to run it.</p>

<p align="center"><b>English</b> | <a href="README.zh-CN.md">简体中文</a></p>

[![CI](https://github.com/thinpack-js/thinpack.js/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/thinpack-js/thinpack.js/actions/workflows/ci.yml)

## Quick start

On macOS or Linux, run this in a new directory:

```sh
curl -fsSL https://raw.githubusercontent.com/thinpack-js/thinpack.js/main/scripts/install.sh | sh
printf '%s\n' 'console.log("Hello from Thinpack!")' > hello.js
"$HOME/.local/bin/thinpack" pack hello.js -o hello
./hello
```

On Windows x64, extract the [matching release archive](https://github.com/thinpack-js/thinpack.js/releases/latest), then run this in the extracted directory:

```powershell
'console.log("Hello from Thinpack!")' | Set-Content -Encoding Ascii hello.js
.\thinpack.exe pack hello.js -o hello.exe
.\hello.exe
```

Both print `Hello from Thinpack!`. Replace `hello.js` with your app's entry point. Pack for the target OS and architecture. Keep `thinpack` and its runtime template together on the build machine; ship just the output executable.

## Size and compatibility

In a [Linux x64 benchmark](docs/benchmarks.md), `tsc` packed to **9.0 MiB** with Thinpack and **109.6 MiB** with Bun `--compile`. Thinpack used QuickJS with `--rebuild` to link only the needed built-ins. The prebuilt template is larger, and QuickJS can trade startup speed for size.

Thinpack supports many Node.js APIs, but it is not a drop-in replacement for Node.js. It does not load native Node addons. Use `--include <path>` to package extra files or computed local imports; paths are relative to the project root. Verify important commands on every platform you ship.

- [Downloads, backend options, and CLI flags](docs/reference.md)
- [Node.js compatibility limits](docs/reference.md#known-limits)
- [Benchmarks and methodology](docs/benchmarks.md)
- [MIT license](LICENSE)
