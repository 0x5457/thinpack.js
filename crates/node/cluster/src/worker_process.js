(() => {
  const { codes, opError } = require("node:thinpack/internal/errors");
  const { Child } = require("node:thinpack/internal/child_process");

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

  // The primary's view of one worker process: the shared `Child` half (pid, piped stdio,
  // exit) plus the IPC channel that makes it a cluster worker.
  class WorkerProcess extends Child {
    #closed = false;
    #connect;

    constructor(id, pid, silent) {
      super();
      this.connected = true;
      this.spawnfile = process.execPath;
      this.spawnargs = [process.execPath];
      this._attach(id, pid, silent);

      // The channel handshake runs after the worker connects; on unix it resolves at once.
      // Channel reads and writes wait on it, while `cluster.fork` keeps its synchronous
      // return because construction never blocks.
      this.#connect = Deno.core.ops.op_cluster_channel_accept(this._id);
      this.#read();
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
        Deno.core.ops.op_cluster_channel_close(this._id);
      } catch {
        // Already closed; the read loop still reports the disconnect.
      }
    }

    #write(frame) {
      return this.#connect.then(() =>
        Deno.core.ops.op_cluster_channel_write(this._id, frame));
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
          frame = await Deno.core.ops.op_cluster_channel_read(this._id);
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

    #finish() {
      if (this.#closed) return;
      this.#closed = true;
      this.connected = false;
      try {
        Deno.core.ops.op_cluster_channel_close(this._id);
      } catch {
        // The read loop may lose the race with an explicit close.
      }
      this.emit("disconnect");
    }
  }

  module.exports = WorkerProcess;
})();
