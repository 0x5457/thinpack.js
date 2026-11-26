(() => {
  const { EventEmitter } = require("node:events");
  const { Readable, Writable } = require("node:stream");
  const { codes, opError, opErrorParts } = require("node:thinpack/internal/errors");

  // Node's `lib/internal/constants.js`: `child_process` and `cluster` translate between the
  // same host signal names, so macOS and Windows do not inherit Linux numbering.
  const { signals: SIGNALS } = require("node:thinpack/internal/constants").os;
  const SIGNAL_NAMES = {};
  // Keep the first name for a number: the table lists canonical names before aliases
  // (SIGABRT before SIGIOT, SIGIO before SIGPOLL).
  for (const [name, number] of Object.entries(SIGNALS)) SIGNAL_NAMES[number] ??= name;

  const signalNumber = (signal) => {
    if (signal === undefined || signal === null) return SIGNALS.SIGTERM;
    if (typeof signal === "number") return signal;
    const number = SIGNALS[String(signal)];
    if (number === undefined) throw new codes.ERR_UNKNOWN_SIGNAL(signal);
    return number;
  };

  const signalName = (signal) =>
    signal > 0 ? (SIGNAL_NAMES[signal] ?? `SIG${signal}`) : null;

  // One spawned process with its stdio pipes: the half `node:child_process` and the cluster
  // primary both need (the same split Node makes in `lib/internal/child_process.js`). A
  // subclass runs its own spawn — a cluster worker also needs an inherited IPC descriptor —
  // and hands the resulting table entry to `_attach`.
  class Child extends EventEmitter {
    // The table entry, kept as an internal field because a subclass's ops (cluster channel
    // reads and writes) address the same entry.
    _id = 0;
    #waiting = null;
    #drained = [];

    constructor() {
      super();
      this.pid = undefined;
      this.exitCode = undefined;
      this.signalCode = null;
      this.killed = false;
      this.connected = false;
      this.stdin = null;
      this.stdout = null;
      this.stderr = null;
    }

    /// Binds this object to a spawned entry: starts the stdio pumps and the exit reporting.
    /// `stdio` names the streams that were spawned as pipes; inherited and ignored streams
    /// keep their `null` stream properties, as in Node.
    _attach(id, pid, stdio) {
      this._id = id;
      this.pid = pid;
      if (stdio.stdin) this.stdin = this.#writable();
      if (stdio.stdout) this.stdout = this.#readable(1);
      if (stdio.stderr) this.stderr = this.#readable(2);
      this.#wait();
    }

    kill(signal) {
      if (this.signalCode !== null || this.exitCode !== undefined) return false;
      // An unknown signal is a usage error and throws, as Node's `kill()` does; only the
      // signal itself failing to reach the process becomes an `error` event.
      const number = signalNumber(signal);
      try {
        Deno.core.ops.op_child_kill(this._id, number);
      } catch (error) {
        // `ESRCH` means the process is already gone, which `kill()` reports as false
        // rather than as an event, the same as Node.
        if (opErrorParts(error)[0] !== "ESRCH") {
          queueMicrotask(() => this.emit("error", opError(error)));
        }
        return false;
      }
      this.killed = true;
      return true;
    }

    ref() {
      if (this.#waiting !== null) Deno.core.refOpPromise(this.#waiting);
      return this;
    }

    unref() {
      if (this.#waiting !== null) Deno.core.unrefOpPromise(this.#waiting);
      return this;
    }

    #readable(which) {
      const stream = new Readable({ read() {} });
      this.#drained.push(new Promise((resolve) => {
        const pump = () => {
          Deno.core.ops.op_child_read(this._id, which).then(
            (chunk) => {
              if (chunk.length === 0) {
                resolve();
                stream.push(null);
                return;
              }
              stream.push(chunk);
              pump();
            },
            (error) => {
              resolve();
              stream.destroy(opError(error));
            },
          );
        };
        pump();
      }));
      return stream;
    }

    #writable() {
      return new Writable({
        write: (chunk, encoding, callback) => {
          Deno.core.ops.op_child_write(this._id, chunk).then(
            () => callback(),
            (error) => callback(opError(error)),
          );
        },
        final: (callback) => {
          Deno.core.ops.op_child_close_stdin(this._id).then(
            () => callback(),
            (error) => callback(opError(error)),
          );
        },
      });
    }

    async #wait() {
      let payload;
      this.#waiting = Deno.core.ops.op_child_wait(this._id);
      try {
        payload = await this.#waiting;
      } catch (error) {
        this.#waiting = null;
        this.emit("error", opError(error));
        return;
      }
      this.#waiting = null;
      const [code, signal] = payload.split("\0").map(Number);
      this.exitCode = code >= 0 ? code : null;
      this.signalCode = signalName(signal);
      this.emit("exit", this.exitCode, this.signalCode);
      // `close` follows `exit` once every stdio stream has ended, as in Node, so a listener
      // that waits for it has seen all of the output.
      await Promise.all(this.#drained);
      this.emit("close", this.exitCode, this.signalCode);
    }
  }

  module.exports = {
    Child,
    signalName,
  };
})();
