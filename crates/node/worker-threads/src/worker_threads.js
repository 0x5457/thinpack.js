(() => {
  const { EventEmitter } = require("node:events");
  const { resolve } = require("node:path");
  const { codes } = require("node:thinpack/internal/errors");

  const core = Deno.core;
  const { ops } = core;
  const brand = core.hostObjectBrand;
  const notImplemented = (name) => new codes.ERR_NOT_IMPLEMENTED(`worker_threads.${name}`);
  const deserializers = {
    MessagePort: (value) => new MessagePort(value.port, value.peer),
  };

  const transferBuffers = (transferList) =>
    transferList === undefined
      ? undefined
      : transferList.filter((item) => item instanceof ArrayBuffer);

  const ids = (raw) => raw.split("\0").map(Number);

  class MessagePort extends EventEmitter {
    #port;
    #peer;
    #closed = false;
    #started = false;
    #unrefed = false;
    #waiting = null;

    constructor(port, peer) {
      super();
      if (typeof port !== "number" || typeof peer !== "number") {
        throw notImplemented("new MessagePort()");
      }
      this.#port = port;
      this.#peer = peer;
    }

    [brand]() {
      if (this.#started) throw notImplemented("transferring a started MessagePort");
      this.#closed = true;
      return { type: "MessagePort", port: this.#port, peer: this.#peer };
    }

    on(event, listener) {
      super.on(event, listener);
      if (event === "message") this.start();
      return this;
    }

    addListener(event, listener) {
      return this.on(event, listener);
    }

    once(event, listener) {
      super.once(event, listener);
      if (event === "message") this.start();
      return this;
    }

    postMessage(value, transferList) {
      if (this.#closed) throw notImplemented("postMessage after the port was transferred");
      if (transferList !== undefined && !Array.isArray(transferList)) {
        throw new codes.ERR_INVALID_ARG_TYPE("transferList", ["Array"], transferList);
      }
      ops.op_port_send(
        this.#peer,
        core.serialize(value, { transferredArrayBuffers: transferBuffers(transferList) }),
      );
    }
    _take() {
      if (this.#closed) return undefined;
      const bytes = ops.op_port_take(this.#port);
      return bytes.length === 0 ? undefined : core.deserialize(bytes, { deserializers });
    }

    start() {
      if (this.#started || this.#closed) return this;
      this.#started = true;
      this.#pump();
      return this;
    }

    async #pump() {
      while (!this.#closed) {
        const promise = ops.op_port_wait(this.#port);
        this.#waiting = promise;
        if (this.#unrefed) core.unrefOpPromise(promise);
        const bytes = await promise;
        this.#waiting = null;
        if (bytes.length === 0) {
          // An empty read means the channel closed; when the peer initiated it,
          // Node still fires `close` on this end.
          if (!this.#closed) {
            this.#closed = true;
            this.emit("close");
          }
          break;
        }
        this.emit("message", core.deserialize(bytes, { deserializers }));
      }
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      ops.op_port_close(this.#port);
      this.emit("close");
    }

    ref() {
      this.#unrefed = false;
      if (this.#waiting !== null) core.refOpPromise(this.#waiting);
      return this;
    }

    unref() {
      this.#unrefed = true;
      if (this.#waiting !== null) core.unrefOpPromise(this.#waiting);
      return this;
    }

    hasRef() {
      return !this.#unrefed;
    }
  }

  class MessageChannel {
    constructor() {
      const [a, b] = ids(ops.op_worker_channel());
      this.port1 = new MessagePort(a, b);
      this.port2 = new MessagePort(b, a);
    }
  }

  const rawContext = ops.op_worker_context();
  const isMainThread = rawContext === "";
  const workerContext = isMainThread ? [0, 0, 0] : ids(rawContext);
  const [threadId, parentPortId, parentPeerId] = workerContext;
  const parentPort = isMainThread ? null : new MessagePort(parentPortId, parentPeerId);
  const workerData = isMainThread
    ? null
    : core.deserialize(ops.op_worker_data(), { deserializers });

  class Worker extends EventEmitter {
    #id;
    #parent;
    #port;
    #pumping = false;
    #waiting = null;
    #unrefed = false;

    constructor(filename, options = {}) {
      super();
      if (typeof filename !== "string") {
        throw new codes.ERR_INVALID_ARG_TYPE("filename", ["string"], filename);
      }
      const data = core.serialize(options.workerData, {
        transferredArrayBuffers: transferBuffers(options.transferList),
      });
      const [port, parent] = ids(ops.op_worker_channel());
      this.#parent = parent;
      this.#port = new MessagePort(port, parent);
      this.#id = ops.op_worker_spawn(resolve(filename), data, parent, port);
      this.#watch();
    }

    on(event, listener) {
      super.on(event, listener);
      if (event === "message") this.#pump();
      return this;
    }

    addListener(event, listener) {
      return this.on(event, listener);
    }

    once(event, listener) {
      super.once(event, listener);
      if (event === "message") this.#pump();
      return this;
    }

    #pump() {
      if (this.#pumping) return;
      this.#pumping = true;
      if (this.#unrefed) this.#port.unref();
      this.#port.on("message", (message) => this.emit("message", message));
    }

    #watch() {
      const promise = ops.op_worker_exit(this.#id);
      this.#waiting = promise;
      if (this.#unrefed) core.unrefOpPromise(promise);
      promise.then((code) => {
        this.#waiting = null;
        this.#port.close();
        const message = ops.op_worker_error(this.#id);
        if (message !== "") this.emit("error", new Error(message));
        this.emit("exit", code);
      });
    }

    postMessage(value, transferList) {
      if (transferList !== undefined && !Array.isArray(transferList)) {
        throw new codes.ERR_INVALID_ARG_TYPE("transferList", ["Array"], transferList);
      }
      ops.op_port_send(
        this.#parent,
        core.serialize(value, { transferredArrayBuffers: transferBuffers(transferList) }),
      );
    }

    terminate() {
      this.#port.close();
      ops.op_worker_terminate(this.#id);
      return Promise.resolve(0);
    }

    ref() {
      this.#unrefed = false;
      this.#port.ref();
      if (this.#waiting !== null) core.refOpPromise(this.#waiting);
      return this;
    }

    unref() {
      this.#unrefed = true;
      this.#port.unref();
      if (this.#waiting !== null) core.unrefOpPromise(this.#waiting);
      return this;
    }

    get threadId() {
      return this.#id;
    }
  }
  const receiveMessageOnPort = (port) => {
    if (!(port instanceof MessagePort)) {
      throw new codes.ERR_INVALID_ARG_TYPE("port", ["MessagePort"], port);
    }
    const message = port._take();
    return message === undefined ? undefined : { message };
  };

  const module_ = {
    isMainThread,
    threadId,
    workerData,
    parentPort,
    SHARE_ENV: Symbol("SHARE_ENV"),
    markAsUncloneable() {
      throw notImplemented("markAsUncloneable");
    },
    getEnvironmentData() {
      throw notImplemented("getEnvironmentData");
    },
    setEnvironmentData() {
      throw notImplemented("setEnvironmentData");
    },
    receiveMessageOnPort,
    MessageChannel,
    MessagePort,
    Worker,
  };

  module.exports = module_;
})();
