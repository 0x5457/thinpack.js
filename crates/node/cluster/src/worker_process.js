(() => {
  const { EventEmitter } = require("node:events");
  const { Readable, Writable } = require("node:stream");
  const { codes, opError } = require("node:thinpack/internal/errors");

  const SIGNALS = {
    SIGABRT: 6,
    SIGALRM: 14,
    SIGBUS: 7,
    SIGCHLD: 17,
    SIGCONT: 18,
    SIGFPE: 8,
    SIGHUP: 1,
    SIGILL: 4,
    SIGINT: 2,
    SIGKILL: 9,
    SIGPIPE: 13,
    SIGQUIT: 3,
    SIGSEGV: 11,
    SIGSTOP: 19,
    SIGTERM: 15,
    SIGTSTP: 20,
    SIGTTIN: 21,
    SIGTTOU: 22,
    SIGUSR1: 10,
    SIGUSR2: 12,
    SIGVTALRM: 26,
    SIGXCPU: 24,
    SIGXFSZ: 25,
  };
  const SIGNAL_NAMES = {};
  for (const [name, number] of Object.entries(SIGNALS)) SIGNAL_NAMES[number] = name;

  const ipcError = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };

  const isInternal = (message) =>
    message !== null &&
    typeof message === "object" &&
    typeof message.cmd === "string" &&
    message.cmd.startsWith("NODE_");

  const signalNumber = (signal) => {
    if (signal === undefined || signal === null) return SIGNALS.SIGTERM;
    if (typeof signal === "number") return signal;
    const number = SIGNALS[String(signal)];
    if (number === undefined) {
      throw ipcError("ERR_UNKNOWN_SIGNAL", `Unknown signal: ${signal}`);
    }
    return number;
  };

  const toBytes = (chunk) =>
    typeof chunk === "string" ? new TextEncoder().encode(chunk) : chunk;

  // The primary's view of one worker process. It covers the subset of ChildProcess that
  // cluster exposes: the channel, exit status, signals, and optional piped stdio.
  class WorkerProcess extends EventEmitter {
    #id;
    #closed = false;
    #waiting = null;
    #connect;

    constructor(id, pid, silent) {
      super();
      this.#id = id;
      this.pid = pid;
      this.connected = true;
      this.exitCode = null;
      this.signalCode = null;
      this.killed = false;
      this.spawnfile = process.execPath;
      this.spawnargs = [process.execPath];

      if (silent) {
        this.stdin = this.#stdin();
        this.stdout = this.#pipe(1);
        this.stderr = this.#pipe(2);
      } else {
        this.stdin = null;
        this.stdout = null;
        this.stderr = null;
      }

      // The channel handshake runs after the worker connects; on unix it resolves at once.
      // Channel reads and writes wait on it, while `cluster.fork` keeps its synchronous
      // return because construction never blocks.
      this.#connect = Deno.core.ops.op_cluster_channel_accept(this.#id);
      this.#read();
      this.#wait();
    }

    send(message, sendHandle, options, callback) {
      if (typeof sendHandle === "function") {
        callback = sendHandle;
        sendHandle = null;
      } else if (typeof options === "function") {
        callback = options;
      }
      if (message === undefined) {
        throw new codes.ERR_MISSING_ARGS("message");
      }
      if (sendHandle !== undefined && sendHandle !== null) {
        throw ipcError("ERR_INVALID_HANDLE_TYPE", "handle passing is not supported");
      }
      if (!this.connected) {
        if (callback !== undefined) {
          queueMicrotask(() => callback(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed")));
        }
        return false;
      }
      const frame = JSON.stringify(message);
      if (frame === undefined) {
        throw ipcError("ERR_INVALID_ARG_TYPE", "message must be serializable");
      }
      this.#write(frame).then(
        () => callback?.(null),
        () => callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed")),
      );
      return true;
    }

    sendInternal(message, callback) {
      if (!this.connected) {
        callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed"));
        return false;
      }
      const frame = JSON.stringify({ cmd: "NODE_CLUSTER", ...message });
      this.#write(frame).then(
        () => callback?.(null),
        () => callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed")),
      );
      return true;
    }

    disconnect() {
      if (!this.connected) return;
      this.connected = false;
      try {
        Deno.core.ops.op_cluster_channel_close(this.#id);
      } catch {
        // Already closed; the read loop still reports the disconnect.
      }
    }

    kill(signal) {
      const number = signalNumber(signal);
      try {
        Deno.core.ops.op_cluster_kill(this.#id, number);
      } catch (error) {
        if (error.code === "ESRCH" || error.code === "-3") return false;
        queueMicrotask(() => this.emit("error", opError(error)));
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

    #write(frame) {
      return this.#connect.then(() =>
        Deno.core.ops.op_cluster_channel_write(this.#id, frame));
    }

    async #read() {
      try {
        await this.#connect;
      } catch (error) {
        // The worker never completed the channel handshake; report the failure instead of
        // a silent channel that can never read.
        this.emit("error", opError(error));
        this.#finish();
        return;
      }
      if (!this.connected) {
        // `disconnect()` won the race with the handshake; finish the teardown now that
        // the channel exists, so its entry is dropped and `disconnect` still fires.
        this.#finish();
        return;
      }
      while (!this.#closed) {
        let frame;
        try {
          frame = await Deno.core.ops.op_cluster_channel_read(this.#id);
        } catch {
          break;
        }
        if (frame === "") break;
        let message;
        try {
          message = JSON.parse(frame);
        } catch {
          continue;
        }
        if (isInternal(message)) this.emit("internalMessage", message, undefined);
        else this.emit("message", message, undefined);
      }
      this.#finish();
    }

    async #wait() {
      let payload;
      this.#waiting = Deno.core.ops.op_cluster_wait(this.#id);
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
      this.signalCode = signal > 0 ? (SIGNAL_NAMES[signal] ?? `SIG${signal}`) : null;
      this.emit("exit", this.exitCode, this.signalCode);
      this.emit("close", this.exitCode, this.signalCode);
    }

    #finish() {
      if (this.#closed) return;
      this.#closed = true;
      this.connected = false;
      try {
        Deno.core.ops.op_cluster_channel_close(this.#id);
      } catch {
        // The read loop may lose the race with an explicit close.
      }
      this.emit("disconnect");
    }

    #pipe(which) {
      const stream = new Readable({ read() {} });
      const pump = () => {
        Deno.core.ops.op_cluster_read_pipe(this.#id, which).then(
          (chunk) => {
            if (chunk.length === 0) {
              stream.push(null);
              return;
            }
            stream.push(chunk);
            pump();
          },
          (error) => stream.destroy(opError(error)),
        );
      };
      pump();
      return stream;
    }

    #stdin() {
      return new Writable({
        write: (chunk, encoding, callback) => {
          Deno.core.ops.op_cluster_write_stdin(this.#id, toBytes(chunk)).then(
            () => callback(),
            (error) => callback(opError(error)),
          );
        },
        final: (callback) => {
          Deno.core.ops.op_cluster_close_stdin(this.#id).then(
            () => callback(),
            (error) => callback(opError(error)),
          );
        },
      });
    }
  }

  module.exports = WorkerProcess;
})();
