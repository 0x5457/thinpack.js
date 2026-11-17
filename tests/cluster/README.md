# Node.js cluster tests

These files are copied from Node.js `test/parallel/test-cluster-*.js` and run unchanged
against the Thinpack runtime. `common/` is a minimal stand-in for Node's
`test/common/index.js`, covering the helpers these tests use (`mustCall`,
`mustCallAtLeast`, `mustNotCall`, `mustSucceed`, `isAlive`, `localhostIPv4`, `skip`,
`common/tmpdir`, `common/countdown`).

Source: Node.js checkout at `/home/tw/Work/node` (commit `1c90519c1a8`, Node 27.0.0-dev),
which is also the version the vendored cluster-adjacent modules are patched from.

Run:

```sh
cargo build -p thinpack-rt
tests/cluster/run.sh                     # all ported tests
tests/cluster/run.sh test-cluster-basic.js # one test, by basename or path
```

The runner uses `target-core/debug/thinpack-rt` by default; set `THINPACK_RT` to another
unpacked build. The entry must be an unpacked runtime: some tests spawn `process.execPath`
with `-e`, which a packed single-file binary intentionally ignores.

## Supported shape

- One primary, any number of process workers.
- `child_process`-style IPC over a socketpair, JSON serialization.
- Shared TCP ports via `SO_REUSEPORT` (Node's `SCHED_NONE` distribution; the kernel spreads
  connections, there is no primary-side round robin).
- `cluster.settings`, `setupPrimary`/`setupMaster`, `fork`, `disconnect`, `Worker`,
  `process.send`/`disconnect`/`connected`/`channel`, the `fork`/`online`/`listening`/
  `disconnect`/`exit`/`message`/`setup` events, `silent` worker stdio, port-0 allocation
  (per listen index, like Node), and `exclusive` listeners.

## Not ported (unsupported by design)

- Handle passing (`worker.send(msg, handle)`, shared server handles, `*handle*`, `net-send`,
  `send-deadlock`, `send-socket-to-worker-http-server`).
- Round-robin scheduling (`SCHED_RR`, `rr-*`, `server-restart-rr`, `net-listen-backlog`).
  `cluster.SCHED_RR` exists for API compatibility but workers still bind independently.
- `dgram` (`dgram-*`, `child-index-dgram`, `disconnect-unshared-udp`).
- UNIX-domain socket sharing (`http-pipe`, `listen-pipe-*`, `net-listen-relative-path`,
  `net-server-drop-connection`, `eaccess`). A non-exclusive cluster listener on a path
  throws `ERR_NOT_IMPLEMENTED`; use TCP.
- Advanced serialization (`advanced-serialization`): only `serialization: 'json'`.
- Custom fork stdio (`fork-stdio`) and `child_process.fork` itself.
- Tests that use `cluster` internals (`net-reuseport`, `accept-fail`, most `rr-*`).
- Tests whose harness needs `child_process.fork` (`bind-twice`, `eaddrinuse`,
  `primary-kill`, `primary-error`, `uncaught-exception`, `fork-windowsHide`).

## Known divergences from Node

- Every non-exclusive listener uses `SO_REUSEPORT`, so two independent cluster groups
  binding the same fixed port both succeed and share it; Node's primary-side handle would
  make the second group fail with `EADDRINUSE`.
- Port 0 is allocated by the primary with a short-lived bind and then closed. If another
  process takes the port before a worker binds it, that worker reports `EADDRINUSE`.
- A worker's `disconnect()` closes its listeners first and then the IPC channel; with
  long-lived connections the primary's `disconnect` event can arrive later than in Node.
- `settings.uid`, `settings.gid`, `settings.execArgv`, and custom `settings.stdio` are
  ignored. `serialization` must be `json`.
- A packed binary always re-runs its own entry; `setupPrimary({ exec })` cannot point at a
  different script.
- Mutating `process.env` does not call `setenv`/`unsetenv`; cluster itself clears
  `NODE_UNIQUE_ID`/`NODE_CHANNEL_FD` from the native environment so workers' children do
  not inherit them.
