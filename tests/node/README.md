# Node.js compatibility suite

Ported Node.js test files, run against the Thinpack runtime. The design follows
Deno's `tests/node_compat`: the upstream tests stay in their original shape, a
manifest records which files run and why others are skipped, and a small cargo
test target turns every file into one Rust test.

There is no shell runner. `cargo test` builds the runtime, discovers the
manifest, runs each file in its own process, and reports a normal test summary.

```sh
cargo test -p thinpack-rt --test node                     # everything
cargo test -p thinpack-rt --test node -- path             # substring filter
cargo test -p thinpack-rt --test node -- --exact path::test-path  # one file
cargo test -p thinpack-rt --test node -- --list           # list, with skip reasons
cargo test -p thinpack-rt --test node -- --ignored        # run only skipped tests
cargo test -p thinpack-rt --test node -- --test-threads 1 # serial
```

When the shared `target/` is managed by an external build cache, point cargo at
a normal target directory (for example `CARGO_TARGET_DIR=target-core`) so the
test target and the runtime binary it exercises come from the same build.

## Layout

```text
tests/node/
  config.toml            manifest: module, ignore reason, timeout, env
  compat/                working directory of every test
    test/                mirror of Node.js' own test/ directory
      common/            our port of test/common (helpers, tmpdir, fixtures)
      fixtures/          only the fixture files the ported tests use
      parallel/          copied test/parallel/test-*.js files
crates/thinpack-rt/tests/node.rs   runner (harness = false)
```

Keeping the `compat/test/...` prefix is not cosmetic:

- `parallel/test-x.js` resolves `require('../common')` to `compat/test/common`.
- Tests that assert on `path.dirname(__filename)` see `test/parallel`, as in Node.
- Tests that read `test/fixtures/...` work because the runner's working directory
  is `compat/`, exactly like Node's own runner running from its test root.

Test files are byte-for-byte copies from Node.js. Never edit a copied test to
make it pass; either fix the runtime or add an `ignore` reason to the manifest.

Source checkout: Node.js, commit `1c90519c1a8` (Node 27.0.0-dev), e.g.
`git clone https://github.com/nodejs/node` at that commit. That is also the
version the vendored cluster-adjacent modules are patched from.

## Manifest

```toml
[tests."parallel/test-path-basename.js"]
module = "path"

[tests."parallel/test-events-customevent.js"]
module = "events"
ignore = "requires Node's internal event_target module"

[tests."parallel/test-cluster-kill-infinite-loop.js"]
module = "cluster"
timeout_ms = 120000
```

- `module` groups tests for filtering and reporting. Use the built-in module
  name (`string_decoder`, not `string-decoder`).
- `ignore` skips the test by default and must contain a concrete reason. It is
  printed by `--list` and the test can be run with `--ignored`.
- `timeout_ms` overrides the 30 second default.
- `env` adds environment variables for one test.
- Every `parallel/test-*.{js,mjs,cjs}` file in the tree must have an entry. The
  runner refuses to start when a copied file is missing from the manifest, so a
  test cannot be added but never run.

## Runner behaviour

Each manifest entry becomes one test named `<module>::<file-stem>`, so
`cargo test -- path` runs a module, `-- --exact <name>` runs one file, and
`--list` shows the inventory (including skip reasons).

A test runs as a child process of the `thinpack-rt` binary cargo built for this
invocation (`CARGO_BIN_EXE_thinpack-rt`), with:

- working directory `tests/node/compat`,
- `TEST_SERIAL_ID` set to a per-run, per-test id, so `common/tmpdir` is isolated,
- `NO_COLOR=1`, and `NODE_OPTIONS`/`NODE_PATH` removed,
- a per-test timeout (30 s default); a timed-out child and its process group are
  killed, and the group is cleaned up after a successful exit as well,
- captured stdout/stderr, shown only on failure and truncated to 32 KiB.

A non-zero exit or a timeout fails the test. `common.skip()` exits 0, as in Node.

Every result line carries the test's duration, and a run of more than five tests
ends with the slowest ones. The compatibility suite is the cheap early warning
that a binding lost its native implementation: a jump there is visible in the
log instead of hiding behind the total.

## Adding a module

1. Copy the relevant Node tests into `compat/test/parallel/` unchanged.
   Keep the upstream file names; the module family decides the `module` value.
2. Add a manifest entry per file.
3. Run the module filter. The runner lists any copied file that is not yet in
   the manifest.
4. For each failure, decide:
   - fix the runtime if the behaviour is implemented but wrong; or
   - add an `ignore` reason naming the missing feature or divergence.
5. Copy only the fixtures the enabled tests need into `compat/test/fixtures/`,
   preserving the upstream relative path.
6. Extend `compat/test/common/index.js` when a test needs another helper. The
   module proxy throws for unknown properties, so a missing helper is explicit.
   `common/index.mjs` mirrors the same names for ESM tests.
7. Re-run the module and then the full suite. An ignored test that starts
   passing should lose its `ignore` field.

The `node:test` module implements registration (`test`/`it`/`describe`/`suite`
with their `skip`/`todo`/`only` variants), hooks, nested subtests, per-test mock
trackers with `mock.fn` and `mock.method`, and TAP output on stdout with a
non-zero exit when a test fails. It does not implement `t.assert`, `t.plan`,
`t.snapshot`, `t.waitFor`, `t.setTimeout`, `mock.timers`,
`mock.getter`/`setter`/`property`, `run()` (which throws `ERR_NOT_IMPLEMENTED`
instead of returning a `TestsStream`), the reporters, or the `--test` CLI;
`docs/gaps.md` records the rest. `test-node-test.mjs` exercises the supported
subset, and `packed::node-test-late-registration` in
`crates/thinpack-rt/tests/packed.rs` is an expected-failure case for a test
registered after the first run. Other upstream tests remain listed with
specific reasons, including runner features that are not covered yet, runtime
behavior thinpack does not provide, Node's `internal/*` modules, or
`vm.runInNewContext`.

## Current coverage

| Module | Passing | Ignored | Ported |
| --- | ---: | ---: | ---: |
| cluster | 46 | 37 | 83 |
| path | 17 | 0 | 17 |
| querystring | 3 | 1 | 4 |
| events | 34 | 12 | 46 |
| os | 2 | 6 | 8 |
| string_decoder | 1 | 3 | 4 |
| buffer | 51 | 23 | 74 |
| url | 6 | 12 | 18 |
| util | 13 | 23 | 36 |
| timers | 28 | 31 | 59 |
| console | 8 | 14 | 22 |
| tty | 2 | 3 | 5 |
| test | 1 | 0 | 1 |

Every upstream `test/parallel` file for these module families is present; the
skipped cluster files are the handle-passing, dgram, round-robin and UNIX-socket
shapes listed below.

Not ported yet: `assert`, `async_hooks`, `child_process`, `constants`,
`crypto`, `diagnostics_channel`, `dns`, `ffi`, `fs`, `http`, `http2`, `https`,
`inspector`, `module`, `net`, `perf_hooks`, `process`, `readline`, `sea`,
`sqlite`, `stream`, `tls`, `v8`, `vm`, `worker_threads`, and `zlib`. The
hand-written async_hooks check in `crates/node/async-hooks/tests/` predates
this suite and stays there until the upstream files are ported.

## Cluster notes

Supported shape:

- One primary, any number of process workers.
- `child_process`-style IPC over a socketpair, JSON serialization.
- Shared TCP ports via `SO_REUSEPORT` (Node's `SCHED_NONE` distribution; the
  kernel spreads connections, there is no primary-side round robin).
- `cluster.settings`, `setupPrimary`/`setupMaster`, `fork`, `disconnect`,
  `Worker`, `process.send`/`disconnect`/`connected`/`channel`, the
  `fork`/`online`/`listening`/`disconnect`/`exit`/`message`/`setup` events,
  `silent` worker stdio, port-0 allocation (per listen index, like Node), and
  `exclusive` listeners.

Not ported (unsupported by design):

- Handle passing (`worker.send(msg, handle)`, shared server handles, `*handle*`,
  `net-send`, `send-deadlock`, `send-socket-to-worker-http-server`).
- Round-robin scheduling (`SCHED_RR`, `rr-*`, `server-restart-rr`,
  `net-listen-backlog`). `cluster.SCHED_RR` exists for API compatibility but
  workers still bind independently.
- `dgram` (`dgram-*`, `child-index-dgram`, `disconnect-unshared-udp`).
- UNIX-domain socket sharing (`http-pipe`, `listen-pipe-*`,
  `net-listen-relative-path`, `net-server-drop-connection`, `eaccess`).
  A non-exclusive cluster listener on a path throws `ERR_NOT_IMPLEMENTED`; use
  TCP.
- Advanced serialization (`advanced-serialization`): only `serialization: 'json'`.
- Custom fork stdio (`fork-stdio`) and `child_process.fork` itself.
- Tests that use `cluster` internals (`net-reuseport`, `accept-fail`, most
  `rr-*`).
- Tests whose harness needs `child_process.fork` (`bind-twice`, `eaddrinuse`,
  `primary-kill`, `primary-error`, `uncaught-exception`, `fork-windowsHide`).

Known divergences from Node:

- Every non-exclusive listener uses `SO_REUSEPORT`, so two independent cluster
  groups binding the same fixed port both succeed and share it; Node's
  primary-side handle would make the second group fail with `EADDRINUSE`.
- Port 0 is allocated by the primary with a short-lived bind and then closed.
  If another process takes the port before a worker binds it, that worker
  reports `EADDRINUSE`.
- A worker's `disconnect()` closes its listeners first and then the IPC
  channel; with long-lived connections the primary's `disconnect` event can
  arrive later than in Node.
- `settings.uid`, `settings.gid`, `settings.execArgv`, and custom
  `settings.stdio` are ignored. `serialization` must be `json`.
- A packed binary always re-runs its own entry; `setupPrimary({ exec })` cannot
  point at a different script.
- Mutating `process.env` does not call `setenv`/`unsetenv`; cluster itself
  clears `NODE_UNIQUE_ID`/`NODE_CHANNEL_FD` from the native environment so
  workers' children do not inherit them.
