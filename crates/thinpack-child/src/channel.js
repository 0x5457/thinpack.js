(() => {
  const { codes, opError } = require("node:thinpack/internal/errors");

  const ipcError = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };

  // Node treats every `NODE_*` command as internal: cluster's own messages and the handle
  // protocol use the same channel as user messages.
  const isInternal = (message) =>
    message !== null &&
    typeof message === "object" &&
    typeof message.cmd === "string" &&
    message.cmd.startsWith("NODE_");

  const setRefed = (state, refed) => {
    state.refed = refed;
    if (state.read === null) return;
    if (refed) Deno.core.refOpPromise(state.read);
    else Deno.core.unrefOpPromise(state.read);
  };

  const flushMessages = (state) => {
    if (state.target.listenerCount("message") === 0) return;
    for (const message of state.pendingMessages.splice(0)) {
      state.target.emit("message", message, undefined);
    }
  };

  const watchMessages = (state) => {
    state.target.on("newListener", (event) => {
      if (event !== "message") return;
      process.nextTick(() => flushMessages(state));
    });
  };

  const finish = (state) => {
    if (state.finished) return;
    state.finished = true;
    state.connected = false;
    state.closed = true;
    state.target.connected = false;
    state.target.channel = null;
    state.pendingMessages.length = 0;
    try {
      Deno.core.ops.op_child_channel_close(state.id);
    } catch {
      // The peer may already have closed the channel; the disconnect event still fires.
    }
    state.target.emit("disconnect");
  };

  const drain = (state) => {
    if (state.closing && state.pending === 0) finish(state);
  };

  const write = (state, frame, callback) => {
    state.pending += 1;
    return state.connect
      .then(() => Deno.core.ops.op_child_channel_write(state.id, frame))
      .then(
        () => {
          state.pending -= 1;
          callback?.(null);
          drain(state);
        },
        (error) => {
          state.pending -= 1;
          const failure = opError(error);
          failure.syscall = "write";
          if (callback !== undefined) callback(failure);
          else queueMicrotask(() => state.target.emit("error", failure));
          drain(state);
        },
      );
  };

  const send = (state, message, sendHandle, options, callback) => {
    if (typeof sendHandle === "function") {
      callback = sendHandle;
      sendHandle = null;
    } else if (typeof options === "function") {
      callback = options;
    }
    if (message === undefined) throw new codes.ERR_MISSING_ARGS("message");
    if (sendHandle !== undefined && sendHandle !== null) {
      throw ipcError("ERR_INVALID_HANDLE_TYPE", "handle passing is not supported");
    }
    if (!state.connected || state.closing) {
      const error = ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed");
      if (callback !== undefined) queueMicrotask(() => callback(error));
      else queueMicrotask(() => state.target.emit("error", error));
      return false;
    }
    const frame = JSON.stringify(message);
    if (frame === undefined) {
      throw ipcError("ERR_INVALID_ARG_TYPE", "message must be serializable");
    }
    write(state, frame, callback);
    return true;
  };

  const sendInternal = (state, message, callback) => {
    if (!state.connected || state.closing) {
      if (callback !== undefined) {
        queueMicrotask(() =>
          callback(ipcError("ERR_IPC_CHANNEL_CLOSED", "channel closed")));
      }
      return false;
    }
    // Only a child query waits for a reply; every other internal message reports write
    // completion so the caller can safely close the channel afterwards.
    const seq = state.kind === "child" ? state.seq++ : (message.seq ?? state.seq++);
    const expectsReply = state.kind === "child" && message.act === "queryServer";
    if (expectsReply && callback !== undefined) state.callbacks.set(seq, callback);
    const frame = JSON.stringify({ cmd: "NODE_CLUSTER", ...message, seq });
    write(state, frame, (error) => {
      if (error !== null) {
        state.callbacks.delete(seq);
        callback?.(error);
        return;
      }
      if (!expectsReply) callback?.(null);
    });
    return true;
  };

  const disconnect = (state) => {
    if (!state.connected) {
      state.target.emit(
        "error",
        ipcError("ERR_IPC_DISCONNECTED", "IPC channel is already disconnected"),
      );
      return;
    }
    // Do not allow new messages. Queued writes finish before the channel is torn down, as
    // in Node.
    state.connected = false;
    state.target.connected = false;
    state.closing = true;
    drain(state);
  };

  const dispatch = (state, frame) => {
    let message;
    try {
      message = JSON.parse(frame);
    } catch {
      return;
    }
    if (
      state.kind === "child" &&
      message !== null &&
      message.act === "port" &&
      state.callbacks.has(message.seq)
    ) {
      const callback = state.callbacks.get(message.seq);
      state.callbacks.delete(message.seq);
      callback(message);
      return;
    }
    if (isInternal(message)) {
      state.target.emit("internalMessage", message, undefined);
      return;
    }
    if (state.target.listenerCount("message") === 0) {
      state.pendingMessages.push(message);
      return;
    }
    state.target.emit("message", message, undefined);
  };

  const pump = async (state) => {
    try {
      await state.connect;
    } catch (error) {
      // The parent never connected its side (the Windows handshake failed); report it
      // instead of leaving a channel that can never read.
      state.target.emit("error", opError(error));
      finish(state);
      return;
    }
    while (!state.finished) {
      let frame;
      try {
        state.read = Deno.core.ops.op_child_channel_read(state.id);
        if (!state.refed) Deno.core.unrefOpPromise(state.read);
        frame = await state.read;
      } catch {
        // A read error means the peer closed or reset the channel. That is the normal
        // disconnect path, not an error on this side; Node reports it as a write error on
        // the side that tried to send.
        break;
      }
      state.read = null;
      if (frame === "") break;
      dispatch(state, frame);
    }
    finish(state);
  };

  const stateOf = (target, id, options) => ({
    kind: "parent",
    target,
    id,
    fd: options.fd ?? -1,
    connected: true,
    closed: false,
    closing: false,
    finished: false,
    callbacks: new Map(),
    pendingMessages: [],
    seq: 0,
    read: null,
    refed: true,
    pending: 0,
    connect: options.connect ?? Promise.resolve(),
  });

  // The parent side of a channel: `node:child_process`'s ChildProcess and `node:cluster`'s
  // WorkerProcess both expose send/message/disconnect on their own event emitter.
  const attach = (target, id, options = {}) => {
    const state = stateOf(target, id, options);
    const control = {
      ref() {
        setRefed(state, true);
        return this;
      },
      unref() {
        setRefed(state, false);
        return this;
      },
      hasRef() {
        return state.refed;
      },
      close() {
        disconnect(state);
      },
      get fd() {
        return state.fd;
      },
    };
    target.channel = control;
    target.connected = true;
    target.send = (message, sendHandle, options, callback) =>
      send(state, message, sendHandle, options, callback);
    target.sendInternal = (message, callback) => sendInternal(state, message, callback);
    target.disconnect = () => disconnect(state);
    watchMessages(state);
    pump(state);
    return state;
  };

  let childState = null;

  // The child side of a channel. It runs before user code, from the startup hook the runtime
  // calls once the module sources are installed.
  const setup = () => {
    if (childState !== null) return childState;

    const port = Number(process.env.NODE_CHANNEL_PORT);
    const fd = Number(process.env.NODE_CHANNEL_FD);
    let channelFd = -1;
    let id;
    // Windows uses a loopback port because the runtime does not pass handles; unix uses
    // the inherited descriptor, as Node does on every platform.
    if (process.platform === "win32" && Number.isInteger(port) && port > 0) {
      id = Deno.core.ops.op_child_channel_adopt_port(port);
    } else if (Number.isInteger(fd) && fd >= 0) {
      channelFd = fd;
      id = Deno.core.ops.op_child_channel_adopt(fd);
    } else {
      throw ipcError("ERR_IPC_DISCONNECTED", "no IPC channel for this process");
    }
    // These describe the bootstrap, not the application; Node removes them before user
    // code runs so a grandchild cannot adopt a channel by accident.
    delete process.env.NODE_CHANNEL_FD;
    delete process.env.NODE_CHANNEL_PORT;
    delete process.env.NODE_CHANNEL_SERIALIZATION_MODE;

    childState = {
      kind: "child",
      target: process,
      id,
      fd: channelFd,
      connected: true,
      closed: false,
      closing: false,
      finished: false,
      callbacks: new Map(),
      pendingMessages: [],
      seq: 0,
      read: null,
      refed: true,
      pending: 0,
      connect: Promise.resolve(),
    };

    process.connected = true;
    process.channel = {
      ref() {
        setRefed(childState, true);
        return this;
      },
      unref() {
        setRefed(childState, false);
        return this;
      },
      hasRef() {
        return childState.refed;
      },
      close() {
        disconnect(childState);
      },
      get fd() {
        return childState.fd;
      },
    };
    process.send = (message, sendHandle, options, callback) =>
      send(childState, message, sendHandle, options, callback);
    process.disconnect = () => disconnect(childState);
    watchMessages(childState);
    pump(childState);
    return childState;
  };

  const sendInternalSync = (message) => {
    const state = setup();
    const seq = state.seq++;
    const frame = JSON.stringify({ cmd: "NODE_CLUSTER", ...message, seq });
    Deno.core.ops.op_child_channel_write_sync(state.id, frame);
  };

  module.exports = {
    attach,
    setup,
    isInternal,
    sendInternal: (message, callback) => sendInternal(setup(), message, callback),
    sendInternalSync,
  };
})();
