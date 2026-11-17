(() => {
  const { codes } = require("node:thinpack/internal/errors");

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

  let state = null;

  const setup = () => {
    if (state !== null) return state;

    const fd = Number(process.env.NODE_CHANNEL_FD);
    if (!Number.isInteger(fd) || fd < 0) {
      throw ipcError("ERR_IPC_DISCONNECTED", "no IPC channel for this cluster worker");
    }
    delete process.env.NODE_CHANNEL_FD;

    const id = Deno.core.ops.op_cluster_channel_adopt(fd);
    state = {
      id,
      connected: true,
      closed: false,
      callbacks: new Map(),
      seq: 0,
      read: null,
      refed: true,
      pending: 0,
      closing: false,
    };

    process.connected = true;
    process.channel = {
      ref() {
        setRefed(true);
        return this;
      },
      unref() {
        setRefed(false);
        return this;
      },
      hasRef() {
        return state.refed;
      },
      close() {
        disconnect();
      },
      get fd() {
        return -1;
      },
    };
    process.send = send;
    process.disconnect = disconnect;
    pump();
    return state;
  };

  const setRefed = (refed) => {
    if (state === null) return;
    state.refed = refed;
    if (state.read === null) return;
    if (refed) Deno.core.refOpPromise(state.read);
    else Deno.core.unrefOpPromise(state.read);
  };

  const close = () => {
    if (state === null || state.closed) return;
    state.closed = true;
    state.connected = false;
    process.connected = false;
    process.channel = null;
    try {
      Deno.core.ops.op_cluster_channel_close(state.id);
    } catch {
      // The channel is already gone; the disconnect event still fires below.
    }
    process.emit("disconnect");
  };

  const pump = async () => {
    while (state !== null && !state.closed) {
      let frame;
      try {
        state.read = Deno.core.ops.op_cluster_channel_read(state.id);
        if (!state.refed) Deno.core.unrefOpPromise(state.read);
        frame = await state.read;
      } catch {
        break;
      }
      state.read = null;
      if (frame === "") break;
      dispatch(frame);
    }
    close();
  };

  const dispatch = (frame) => {
    let message;
    try {
      message = JSON.parse(frame);
    } catch {
      return;
    }
    if (!isInternal(message)) {
      process.emit("message", message, undefined);
      return;
    }
    if (message.act === "port" && state.callbacks.has(message.seq)) {
      const callback = state.callbacks.get(message.seq);
      state.callbacks.delete(message.seq);
      callback(message);
      return;
    }
    process.emit("internalMessage", message, undefined);
  };

  const write = (frame, callback) => {
    if (state === null || state.closed) {
      callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed"));
      return Promise.resolve();
    }
    state.pending += 1;
    return Deno.core.ops.op_cluster_channel_write(state.id, frame).then(
      () => {
        state.pending -= 1;
        callback?.(null);
        drain();
      },
      () => {
        state.pending -= 1;
        callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed"));
        drain();
      },
    );
  };

  // `process.send()` followed by `process.disconnect()` must deliver the queued message
  // before the channel is torn down, which is what Node does.
  const drain = () => {
    if (state !== null && state.closing && state.pending === 0) close();
  };

  const send = (message, sendHandle, options, callback) => {
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
    if (state === null || !state.connected || state.closing) {
      if (callback !== undefined) {
        queueMicrotask(() =>
          callback(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed")));
      }
      return false;
    }
    const frame = JSON.stringify(message);
    if (frame === undefined) {
      throw ipcError("ERR_INVALID_ARG_TYPE", "message must be serializable");
    }
    write(frame, callback);
    return true;
  };

  const sendInternal = (message, callback) => {
    if (state === null || !state.connected || state.closing) {
      callback?.(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed"));
      return false;
    }
    const seq = state.seq++;
    // Only queries wait for a reply; every other internal message reports write completion so
    // the caller can safely close the channel afterwards.
    const expectsReply = message.act === "queryServer";
    if (callback !== undefined && expectsReply) state.callbacks.set(seq, callback);
    const frame = JSON.stringify({ cmd: "NODE_CLUSTER", ...message, seq });
    write(frame, (error) => {
      if (error !== null) {
        state.callbacks.delete(seq);
        callback?.(error);
        return;
      }
      if (!expectsReply) callback?.(null);
    });
    return true;
  };

  const sendOnline = () => {
    if (state === null) return;
    const seq = state.seq++;
    const frame = JSON.stringify({ cmd: "NODE_CLUSTER", act: "online", seq });
    Deno.core.ops.op_cluster_channel_write_sync(state.id, frame);
  };

  const disconnect = () => {
    if (state === null || state.closed) {
      throw ipcError("ERR_IPC_DISCONNECTED", "IPC channel is already disconnected");
    }
    state.closing = true;
    if (state.pending === 0) close();
  };

  module.exports = {
    setup,
    send,
    sendInternal,
    disconnect,
    write,
    sendOnline,
    isInternal,
  };
})();
