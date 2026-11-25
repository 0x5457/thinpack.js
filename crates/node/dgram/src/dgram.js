// Copyright Joyent, Inc. and other Node contributors.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to permit
// persons to whom the Software is furnished to do so, subject to the
// following conditions:
//
// The above copyright notice and this permission notice shall be included
// in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN
// NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
// OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
// USE OR OTHER DEALINGS IN THE SOFTWARE.

// The public surface follows Node's lib/dgram.js. The native UDP handle is
// replaced by ops that bind a socket on demand and by one pending receive op per
// bound socket; that op is the handle that keeps the event loop alive, so close
// cancels it before dropping the socket.
(() => {
  const { EventEmitter } = require("node:events");
  const {
    codes,
    ErrnoException,
    ExceptionWithHostPort,
    opErrorParts,
  } = require("node:thinpack/internal/errors");
  const {
    validateAbortSignal,
    validateFunction,
    validateNumber,
    validateString,
    validateUint32,
  } = require("node:thinpack/internal/validators");
  const {
    getSystemErrorName,
    getSystemErrorMessage,
  } = require("node:thinpack/internal/util");

  const kErrorMonitor = EventEmitter.errorMonitor;
  const kAsyncDispose = Symbol.asyncDispose ?? Symbol.for("nodejs.asyncDispose");
  const kState = Symbol("dgram state");
  const RECV_BUFFER = true;
  const SEND_BUFFER = false;

  class ERR_SOCKET_ALREADY_BOUND extends Error {
    constructor() {
      super("Socket is already bound");
      this.code = "ERR_SOCKET_ALREADY_BOUND";
    }
  }
  class ERR_SOCKET_BAD_BUFFER_SIZE extends TypeError {
    constructor() {
      super("Buffer size must be a positive integer");
      this.code = "ERR_SOCKET_BAD_BUFFER_SIZE";
    }
  }
  class ERR_SOCKET_BUFFER_SIZE extends Error {
    constructor(syscall, errno = -9) {
      super(
        "Could not get or set buffer size: " + syscall + " returned " +
          getSystemErrorName(errno) + " (" + getSystemErrorMessage(errno) + ")",
      );
      this.code = "ERR_SOCKET_BUFFER_SIZE";
    }
  }
  class ERR_SOCKET_BAD_PORT extends RangeError {
    constructor(name, port, allowZero = true) {
      super(
        name + " should be " + (allowZero ? ">=" : ">") +
          " 0 and < 65536. Received " + describeType(port) + ".",
      );
      this.code = "ERR_SOCKET_BAD_PORT";
    }
  }
  class ERR_SOCKET_BAD_TYPE extends TypeError {
    constructor() {
      super("Bad socket type specified. Valid types are: udp4, udp6");
      this.code = "ERR_SOCKET_BAD_TYPE";
    }
  }
  class ERR_SOCKET_DGRAM_IS_CONNECTED extends Error {
    constructor() {
      super("Already connected");
      this.code = "ERR_SOCKET_DGRAM_IS_CONNECTED";
    }
  }
  class ERR_SOCKET_DGRAM_NOT_CONNECTED extends Error {
    constructor() {
      super("Not connected");
      this.code = "ERR_SOCKET_DGRAM_NOT_CONNECTED";
    }
  }
  class ERR_SOCKET_DGRAM_NOT_RUNNING extends Error {
    constructor() {
      super("Not running");
      this.code = "ERR_SOCKET_DGRAM_NOT_RUNNING";
    }
  }

  const describeType = (value) => {
    if (typeof value === "number") return "type number (" + value + ")";
    if (typeof value === "string") return "type string ('" + value + "')";
    return "type " + typeof value + " (" + String(value) + ")";
  };

  const validatePort = (port, name = "Port", allowZero = true) => {
    const value = typeof port === "string" ? port.trim() : port;
    const number = Number(value);
    if (
      (typeof port !== "number" && typeof port !== "string") ||
      value === "" || !Number.isInteger(number) ||
      number < 0 || number > 65535 || (number === 0 && !allowZero)
    ) {
      throw new ERR_SOCKET_BAD_PORT(name, port, allowZero);
    }
    return number;
  };

  // libuv passes UDP ports through a uint16 conversion; bind() inherits that
  // rather than rejecting an out-of-range value the way send() and connect() do.
  const bindPort = (port) => Number(port) & 0xFFFF;

  const isIPv4 = (value) => {
    const parts = value.split(".");
    return parts.length === 4 && parts.every((part) =>
      /^(?:0|[1-9]\d{0,2})$/.test(part) && Number(part) <= 255
    );
  };

  const isIPv6 = (value) => {
    if (!value.includes(":")) return false;
    const address = value.includes("%") ? value.slice(0, value.indexOf("%")) : value;
    try {
      new URL("http://[" + address + "]/");
      return true;
    } catch {
      return false;
    }
  };

  const isIP = (value) => {
    if (typeof value !== "string") return 0;
    if (isIPv4(value)) return 4;
    return isIPv6(value) ? 6 : 0;
  };

  const dnsError = (hostname) => {
    const error = new Error("getaddrinfo ENOTFOUND " + hostname);
    error.errno = -3008;
    error.code = "ENOTFOUND";
    error.syscall = "getaddrinfo";
    error.hostname = String(hostname);
    return error;
  };

  const socketError = (error, syscall, address, port) => {
    const [code, detail] = opErrorParts(error);
    if (detail !== undefined) {
      if (/^-?\d+$/.test(code)) {
        return new ExceptionWithHostPort(Number(code), syscall, address, port);
      }
      const failure = new Error(detail);
      failure.code = code;
      return failure;
    }
    const errno = Number(code);
    if (Number.isInteger(errno)) {
      return new ExceptionWithHostPort(errno, syscall, address, port);
    }
    return error;
  };

  const healthCheck = (socket) => {
    if (!socket[kState].handle) throw new ERR_SOCKET_DGRAM_NOT_RUNNING();
  };

  const sliceBuffer = (buffer, offset, length) => {
    if (typeof buffer === "string") {
      buffer = Buffer.from(buffer);
    } else if (!ArrayBuffer.isView(buffer)) {
      throw new codes.ERR_INVALID_ARG_TYPE(
        "buffer",
        ["Buffer", "TypedArray", "DataView", "string"],
        buffer,
      );
    }
    offset >>>= 0;
    length >>>= 0;
    if (offset > buffer.byteLength) {
      throw new codes.ERR_BUFFER_OUT_OF_BOUNDS("offset");
    }
    if (offset + length > buffer.byteLength) {
      throw new codes.ERR_BUFFER_OUT_OF_BOUNDS("length");
    }
    return Buffer.from(buffer.buffer, buffer.byteOffset + offset, length);
  };

  const fixBufferList = (list) => {
    const fixed = new Array(list.length);
    for (let index = 0; index < list.length; index += 1) {
      const buffer = list[index];
      if (typeof buffer === "string") {
        fixed[index] = Buffer.from(buffer);
      } else if (Buffer.isBuffer(buffer)) {
        fixed[index] = buffer;
      } else if (!ArrayBuffer.isView(buffer)) {
        return null;
      } else {
        fixed[index] = Buffer.from(buffer.buffer, buffer.byteOffset, buffer.byteLength);
      }
    }
    return fixed;
  };

  class Socket extends EventEmitter {
    type;
    [kState];

    constructor(type, listener) {
      super();
      let options;
      if (type !== null && typeof type === "object") {
        options = type;
        type = options.type;
        if (options.recvBufferSize) validateUint32(options.recvBufferSize, "options.recvBufferSize");
        if (options.sendBufferSize) validateUint32(options.sendBufferSize, "options.sendBufferSize");
        if (options.lookup !== undefined) validateFunction(options.lookup, "lookup");
      }
      if (type !== "udp4" && type !== "udp6") throw new ERR_SOCKET_BAD_TYPE();

      this.type = type;
      this[kState] = {
        handle: {},
        lookup: options?.lookup,
        reuseAddr: Boolean(options?.reuseAddr),
        reusePort: Boolean(options?.reusePort),
        ipv6Only: Boolean(options?.ipv6Only),
        recvBufferSize: options?.recvBufferSize,
        sendBufferSize: options?.sendBufferSize,
        id: 0,
        binding: undefined,
        bound: false,
        receiving: false,
        connected: false,
        connecting: false,
        unrefed: false,
        recv: undefined,
        sendQueueBytes: 0,
        sendQueueCount: 0,
      };

      if (typeof listener === "function") this.on("message", listener);

      if (options?.signal !== undefined) {
        const { signal } = options;
        validateAbortSignal(signal, "options.signal");
        const onAborted = () => {
          if (this[kState].handle) this.close();
        };
        if (signal.aborted) {
          onAborted();
        } else {
          signal.addEventListener("abort", onAborted, { once: true });
          this.once("close", () => signal.removeEventListener("abort", onAborted));
        }
      }
    }

    get #family() {
      return this.type === "udp4" ? 4 : 6;
    }

    #resolve(address, bind) {
      if (!address) {
        if (bind) address = this.type === "udp4" ? "0.0.0.0" : "::";
        else address = this.type === "udp4" ? "127.0.0.1" : "::1";
      }
      if (isIP(address) === this.#family) return Promise.resolve(address);
      if (isIP(address) !== 0) return Promise.reject(dnsError(address));
      const lookup = this[kState].lookup;
      if (lookup !== undefined) {
        return new Promise((resolve, reject) => {
          lookup(address, this.#family, (error, ip) => {
            if (error) reject(error);
            else resolve(ip);
          });
        });
      }
      return Deno.core.ops
        .op_dgram_lookup(String(address), this.#family)
        .catch(() => Promise.reject(dnsError(address)));
    }

    #id(syscall) {
      healthCheck(this);
      const { id } = this[kState];
      if (id === 0) throw new ErrnoException(-9, syscall);
      return id;
    }

    #startReceiving() {
      this[kState].receiving = true;
      this.#pump();
    }

    #pump() {
      const state = this[kState];
      if (!state.handle || !state.bound || state.recv !== undefined) return;
      const id = state.id;
      const pending = Deno.core.ops.op_dgram_recv(id);
      state.recv = pending;
      if (state.unrefed) Deno.core.unrefOpPromise(pending);
      pending.then(({ data, address, port }) => {
        if (this[kState] !== state || state.id !== id) return;
        state.recv = undefined;
        const buffer = Buffer.from(data.buffer, data.byteOffset, data.byteLength);
        this.emit("message", buffer, {
          address,
          family: isIP(address) === 6 ? "IPv6" : "IPv4",
          port,
          size: buffer.length,
        });
        this.#pump();
      }, (error) => {
        if (this[kState] !== state || state.id !== id) return;
        state.recv = undefined;
        this.emit("error", socketError(error, "recvmsg"));
        this.#pump();
      });
    }

    bind(port_, address_, callback_) {
      healthCheck(this);
      const state = this[kState];
      if (state.binding !== undefined || state.bound) throw new ERR_SOCKET_ALREADY_BOUND();

      let port = port_;
      let address = address_;
      let callback = callback_;

      const last = arguments[arguments.length - 1];
      if (typeof last === "function") callback = last;

      if (port !== null && typeof port === "object") {
        if (Number.isInteger(port.fd) && port.fd > 0) {
          throw new codes.ERR_NOT_IMPLEMENTED("dgram.Socket.bind({ fd })");
        }
        address = port.address;
        port = port.port;
      } else if (typeof address === "function") {
        address = undefined;
      }
      if (typeof port === "function") {
        port = undefined;
        address = undefined;
      }
      if (address !== undefined && address !== null) {
        validateString(address, "hostname");
      }

      if (typeof callback === "function") {
        const onError = () => {
          this.removeListener(kErrorMonitor, onError);
          this.removeListener("listening", onListening);
        };
        const onListening = () => {
          this.removeListener(kErrorMonitor, onError);
          this.removeListener("listening", onListening);
          callback.call(this);
        };
        this.on(kErrorMonitor, onError);
        this.on("listening", onListening);
      }

      state.binding = this.#bind(port, address);
      return this;
    }

    async #bind(port, address) {
      const state = this[kState];
      let ip;
      try {
        if (address === null) address = undefined;
        ip = await this.#resolve(address, true);
        if (this[kState] !== state || !state.handle) return;
        let flags = 0;
        if (this.type === "udp6") flags |= 1;
        if (state.reuseAddr) flags |= 2;
        if (state.reusePort) flags |= 4;
        if (state.ipv6Only) flags |= 8;
        const id = Deno.core.ops.op_dgram_bind(ip, bindPort(port), flags);
        state.id = id;
        state.bound = true;
        if (state.recvBufferSize) {
          Deno.core.ops.op_dgram_set_buffer_size(id, state.recvBufferSize, RECV_BUFFER);
        }
        if (state.sendBufferSize) {
          Deno.core.ops.op_dgram_set_buffer_size(id, state.sendBufferSize, SEND_BUFFER);
        }
        this.#startReceiving();
        this.emit("listening");
      } catch (error) {
        state.binding = undefined;
        if (this[kState] !== state || !state.handle) return;
        const failure = socketError(error, "bind", ip ?? address, port);
        process.nextTick(() => {
          if (this[kState] === state) this.emit("error", failure);
        });
      }
    }

    close(callback) {
      if (typeof callback === "function") this.on("close", callback);
      healthCheck(this);
      const state = this[kState];
      this[kState] = { ...state, handle: null };
      if (state.id !== 0) Deno.core.ops.op_dgram_close(state.id);
      process.nextTick(() => this.emit("close"));
      return this;
    }

    [kAsyncDispose]() {
      if (!this[kState].handle) return Promise.resolve();
      return new Promise((resolve) => this.close(resolve));
    }

    connect(port, address, callback) {
      port = validatePort(port, "Port", false);
      if (typeof address === "function") {
        callback = address;
        address = "";
      } else if (address === undefined) {
        address = "";
      }
      validateString(address, "address");

      const state = this[kState];
      if (state.connected || state.connecting) throw new ERR_SOCKET_DGRAM_IS_CONNECTED();
      state.connecting = true;
      if (!state.bound && state.binding === undefined) this.bind({ port: 0 });

      const run = async () => {
        if (state.binding !== undefined) await state.binding;
        if (this[kState] !== state || !state.handle || !state.bound) return;
        if (callback) this.once("connect", callback);
        let failure;
        try {
          const ip = await this.#resolve(address, false);
          if (this[kState] !== state || !state.handle) return;
          await Deno.core.ops.op_dgram_connect(state.id, ip, port);
        } catch (error) {
          failure = socketError(error, "connect", address, port);
        }
        if (this[kState] !== state || !state.handle) return;
        state.connecting = false;
        if (failure !== undefined) {
          state.connected = false;
          process.nextTick(() => {
            if (callback) {
              this.removeListener("connect", callback);
              callback(failure);
            } else {
              this.emit("error", failure);
            }
          });
          return;
        }
        state.connected = true;
        // Record the state first and queue the emit outside the catch: a listener
        // that throws must escape as an uncaught exception (as in Node), not
        // become a connect failure that resets the socket and calls back again.
        process.nextTick(() => this.emit("connect"));
      };
      run();
    }

    disconnect() {
      healthCheck(this);
      const state = this[kState];
      if (!state.connected) throw new ERR_SOCKET_DGRAM_NOT_CONNECTED();
      try {
        Deno.core.ops.op_dgram_disconnect(state.id);
      } catch (error) {
        throw socketError(error, "connect");
      }
      state.connected = false;
    }

    send(buffer, offset, length, port, address, callback) {
      const state = this[kState];
      const connected = state.connected;

      if (!connected) {
        if (address || (port && typeof port !== "function")) {
          buffer = sliceBuffer(buffer, offset, length);
        } else {
          callback = port;
          port = offset;
          address = length;
        }
      } else {
        if (typeof length === "number") {
          buffer = sliceBuffer(buffer, offset, length);
          if (typeof port === "function") {
            callback = port;
            port = null;
          }
        } else {
          callback = offset;
        }
        if (port || address) throw new ERR_SOCKET_DGRAM_IS_CONNECTED();
      }

      let list;
      if (!Array.isArray(buffer)) {
        if (typeof buffer === "string") {
          list = [Buffer.from(buffer)];
        } else if (!ArrayBuffer.isView(buffer)) {
          throw new codes.ERR_INVALID_ARG_TYPE(
            "buffer",
            ["Buffer", "TypedArray", "DataView", "string"],
            buffer,
          );
        } else {
          list = [buffer];
        }
      } else if (!(list = fixBufferList(buffer))) {
        throw new codes.ERR_INVALID_ARG_TYPE(
          "buffer list arguments",
          ["Buffer", "TypedArray", "DataView", "string"],
          buffer,
        );
      }

      if (!connected) port = validatePort(port, "Port", false);
      if (typeof callback !== "function") callback = undefined;
      if (typeof address === "function") {
        callback = address;
        address = undefined;
      } else if (address != null) {
        validateString(address, "address");
      }

      healthCheck(this);
      if (!state.bound && state.binding === undefined) this.bind({ port: 0 });
      if (list.length === 0) list = [Buffer.alloc(0)];

      const payload = list.length === 1
        ? (Buffer.isBuffer(list[0])
          ? list[0]
          : Buffer.from(list[0].buffer, list[0].byteOffset, list[0].byteLength))
        : Buffer.concat(list);
      const destination = connected ? undefined : port;

      const run = async () => {
        if (state.binding !== undefined) await state.binding;
        if (this[kState] !== state || !state.handle || !state.bound) return;
        let ip;
        if (!connected) {
          try {
            ip = await this.#resolve(address, false);
          } catch (error) {
            // A lookup failure is reported like Node's doSend(): through the
            // callback, or as an 'error' event when there is none.
            if (callback) process.nextTick(callback, error);
            else process.nextTick(() => this.emit("error", error));
            return;
          }
          if (this[kState] !== state || !state.handle) return;
        }
        state.sendQueueBytes += payload.length;
        state.sendQueueCount += 1;
        try {
          const sent = await Deno.core.ops.op_dgram_send(
            state.id,
            payload,
            ip,
            destination || 0,
          );
          if (callback) process.nextTick(callback, null, sent);
        } catch (error) {
          // libuv reports send errors through the request callback; without one
          // they are dropped, as Node's doSend() does.
          if (callback) {
            process.nextTick(
              callback,
              socketError(error, "send", address, destination),
            );
          }
        } finally {
          state.sendQueueBytes -= payload.length;
          state.sendQueueCount -= 1;
        }
      };
      run();
    }

    sendto(buffer, offset, length, port, address, callback) {
      validateNumber(offset, "offset");
      validateNumber(length, "length");
      validateNumber(port, "port");
      validateString(address, "address");
      this.send(buffer, offset, length, port, address, callback);
    }

    address() {
      healthCheck(this);
      const state = this[kState];
      if (!state.bound) throw new ErrnoException(-9, "getsockname");
      try {
        return Deno.core.ops.op_dgram_local_info(state.id);
      } catch (error) {
        throw socketError(error, "getsockname");
      }
    }

    remoteAddress() {
      healthCheck(this);
      const state = this[kState];
      if (!state.connected) throw new ERR_SOCKET_DGRAM_NOT_CONNECTED();
      try {
        return Deno.core.ops.op_dgram_remote_info(state.id);
      } catch (error) {
        throw socketError(error, "getpeername");
      }
    }

    setBroadcast(arg) {
      const id = this.#id("setBroadcast");
      try {
        Deno.core.ops.op_dgram_set_broadcast(id, Boolean(arg));
      } catch (error) {
        throw socketError(error, "setBroadcast");
      }
    }

    setTTL(ttl) {
      validateNumber(ttl, "ttl");
      healthCheck(this);
      try {
        // Node passes the value through a C int conversion, so NaN and Infinity
        // become 0 and a fraction truncates before libuv's 1..255 check.
        Deno.core.ops.op_dgram_set_ttl(this[kState].id, ttl | 0);
      } catch (error) {
        throw socketError(error, "setTTL");
      }
      return ttl;
    }

    setMulticastTTL(ttl) {
      validateNumber(ttl, "ttl");
      healthCheck(this);
      try {
        Deno.core.ops.op_dgram_set_multicast_ttl(this[kState].id, ttl | 0);
      } catch (error) {
        throw socketError(error, "setMulticastTTL");
      }
      return ttl;
    }

    setMulticastLoopback(arg) {
      const id = this.#id("setMulticastLoopback");
      try {
        Deno.core.ops.op_dgram_set_multicast_loopback(id, Boolean(arg));
      } catch (error) {
        throw socketError(error, "setMulticastLoopback");
      }
      return arg;
    }

    setMulticastInterface(interfaceAddress) {
      healthCheck(this);
      validateString(interfaceAddress, "interfaceAddress");
      try {
        Deno.core.ops.op_dgram_set_multicast_interface(
          this[kState].id,
          interfaceAddress,
        );
      } catch (error) {
        throw socketError(error, "setMulticastInterface");
      }
    }

    addMembership(multicastAddress, interfaceAddress) {
      healthCheck(this);
      if (!multicastAddress) throw new codes.ERR_MISSING_ARGS("multicastAddress");
      try {
        Deno.core.ops.op_dgram_add_membership(
          this[kState].id,
          String(multicastAddress),
          interfaceAddress ?? null,
        );
      } catch (error) {
        throw socketError(error, "addMembership");
      }
    }

    dropMembership(multicastAddress, interfaceAddress) {
      healthCheck(this);
      if (!multicastAddress) throw new codes.ERR_MISSING_ARGS("multicastAddress");
      try {
        Deno.core.ops.op_dgram_drop_membership(
          this[kState].id,
          String(multicastAddress),
          interfaceAddress ?? null,
        );
      } catch (error) {
        throw socketError(error, "dropMembership");
      }
    }

    addSourceSpecificMembership(sourceAddress, groupAddress, interfaceAddress) {
      healthCheck(this);
      validateString(sourceAddress, "sourceAddress");
      validateString(groupAddress, "groupAddress");
      try {
        Deno.core.ops.op_dgram_add_source_membership(
          this[kState].id,
          sourceAddress,
          groupAddress,
          interfaceAddress ?? null,
        );
      } catch (error) {
        throw socketError(error, "addSourceSpecificMembership");
      }
    }

    dropSourceSpecificMembership(sourceAddress, groupAddress, interfaceAddress) {
      healthCheck(this);
      validateString(sourceAddress, "sourceAddress");
      validateString(groupAddress, "groupAddress");
      try {
        Deno.core.ops.op_dgram_drop_source_membership(
          this[kState].id,
          sourceAddress,
          groupAddress,
          interfaceAddress ?? null,
        );
      } catch (error) {
        throw socketError(error, "dropSourceSpecificMembership");
      }
    }

    ref() {
      const state = this[kState];
      if (state.handle) {
        state.unrefed = false;
        if (state.recv !== undefined) Deno.core.refOpPromise(state.recv);
      }
      return this;
    }

    unref() {
      const state = this[kState];
      if (state.handle) {
        state.unrefed = true;
        if (state.recv !== undefined) Deno.core.unrefOpPromise(state.recv);
      }
      return this;
    }

    setRecvBufferSize(size) {
      this.#bufferSize(size, RECV_BUFFER, true);
    }

    setSendBufferSize(size) {
      this.#bufferSize(size, SEND_BUFFER, true);
    }

    getRecvBufferSize() {
      return this.#bufferSize(0, RECV_BUFFER, false);
    }

    getSendBufferSize() {
      return this.#bufferSize(0, SEND_BUFFER, false);
    }

    #bufferSize(size, recv, set) {
      if (size >>> 0 !== size) throw new ERR_SOCKET_BAD_BUFFER_SIZE();
      healthCheck(this);
      const syscall = recv ? "uv_recv_buffer_size" : "uv_send_buffer_size";
      const id = this[kState].id;
      if (id === 0) throw new ERR_SOCKET_BUFFER_SIZE(syscall);
      try {
        if (set) {
          Deno.core.ops.op_dgram_set_buffer_size(id, size, recv);
          return undefined;
        }
        return Deno.core.ops.op_dgram_get_buffer_size(id, recv);
      } catch (error) {
        const [code] = opErrorParts(error);
        const errno = Number(code);
        throw new ERR_SOCKET_BUFFER_SIZE(
          syscall,
          Number.isInteger(errno) ? errno : -9,
        );
      }
    }

    getSendQueueSize() {
      healthCheck(this);
      return this[kState].sendQueueBytes;
    }

    getSendQueueCount() {
      healthCheck(this);
      return this[kState].sendQueueCount;
    }
  }

  const createSocket = (type, listener) => new Socket(type, listener);

  module.exports = { createSocket, Socket };
})();
