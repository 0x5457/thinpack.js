(() => {
  const { ops } = Deno.core;

  const fields = (raw) => (raw === "" ? [] : raw.split("\0"));

  const [platform, arch, pidText] = fields(ops.op_process_info());
  const pid = Number(pidText);
  const [uid, gid, euid, egid] = fields(ops.op_process_ids()).map(Number);
  const argv = fields(ops.op_process_argv());
  const executable = argv[0];
  const env = {};
  for (const entry of fields(ops.op_process_env())) {
    const separator = entry.indexOf("=");
    if (separator > 0) {
      env[entry.slice(0, separator)] = entry.slice(separator + 1);
    }
  }

  const invalidArgument = (name, expected) => {
    const error = new TypeError(
      'The "' + name + '" argument must be of type ' + expected,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };
  const internalErrors = () =>
    globalThis.__thinpack_require__("node:thinpack/internal/errors");
  const internalValidators = () =>
    globalThis.__thinpack_require__("node:thinpack/internal/validators");

  // Color support belongs to the terminal environment, not to a stream. The
  // shared implementation loads on first use: extension startup scripts run
  // before a packed runtime installs the builtin sources a `require` needs.
  let ttyColors;
  const ttyColorSupport = () =>
    ttyColors ??= globalThis.__thinpack_require__("node:thinpack/internal/tty");

  // `process.stdin` is the only readable stdio stream, so it owns the one raw
  // mode the process surface exposes.
  let stdinRawMode = false;
  const setRawMode = (mode) => {
    let rawMode;
    if (mode === "io" || mode === "raw") {
      rawMode = mode;
    } else if (typeof mode === "string") {
      const { codes } = internalErrors();
      throw new codes.ERR_INVALID_ARG_VALUE(
        "mode", mode, "must be true, false, 'raw', or 'io'",
      );
    } else {
      rawMode = mode ? "raw" : false;
    }
    // Disabling raw mode that was never enabled must not create the terminal.
    if (rawMode === false && !stdinRawMode) return;
    const ttyMode = rawMode === false ? "normal" : rawMode;
    try {
      ops.op_process_set_raw_mode(0, ttyMode);
    } catch (error) {
      const errno = Number(error.message);
      if (!Number.isInteger(errno)) throw error;
      const { ErrnoException } = internalErrors();
      throw new ErrnoException(errno, "setRawMode");
    }
    stdinRawMode = rawMode !== false;
  };

  const restoreTerminal = () => {
    if (stdinRawMode) setRawMode(false);
  };
  let asyncHooks;
  const internalAsyncHooks = () =>
    asyncHooks ??= globalThis.__thinpack_require__(
      "node:thinpack/internal/async_hooks",
    );

  const nextTickQueue = [];
  let nextTickScheduled = false;
  const drainNextTick = () => {
    nextTickScheduled = false;
    const hooks = internalAsyncHooks();
    while (nextTickQueue.length > 0) {
      const tick = nextTickQueue.shift();
      try {
        hooks.runInScope(tick.scope, tick.callback, undefined, tick.args);
      } finally {
        hooks.destroyScope(tick.scope);
      }
    }
  };
  const nextTick = (callback, ...args) => {
    if (typeof callback !== "function") {
      throw invalidArgument("callback", "function");
    }
    const hooks = internalAsyncHooks();
    const tick = { callback, args, scope: hooks.createResourceScope("TickObject") };
    hooks.attachResource(tick.scope, tick);
    nextTickQueue.push(tick);
    if (!nextTickScheduled) {
      nextTickScheduled = true;
      queueMicrotask(drainNextTick);
    }
  };

  const write = (isError) => (chunk, encoding, callback) => {
    if (typeof chunk === "string") {
      ops.op_thinpack_io_write(chunk, isError);
    } else if (ArrayBuffer.isView(chunk)) {
      ops.op_thinpack_io_write_bytes(
        new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength),
        isError,
      );
    } else if (chunk instanceof ArrayBuffer) {
      ops.op_thinpack_io_write_bytes(new Uint8Array(chunk), isError);
    } else {
      throw invalidArgument("chunk", "string, Buffer, or Uint8Array");
    }
    const done = typeof encoding === "function" ? encoding : callback;
    if (typeof done === "function") {
      queueMicrotask(done);
    }
    return true;
  };

  const stream = (isError, fd) => {
    const listeners = new Map();
    const on = (event, listener) => {
      if (!listeners.has(event)) listeners.set(event, []);
      listeners.get(event).push(listener);
      return result;
    };
    const isTTY = ops.op_thinpack_io_isatty(fd);
    const result = {
      fd,
      isTTY,
      writable: true,
      destroyed: false,
      writableEnded: false,
      write: write(isError),
      end(chunk, encoding, callback) {
        if (typeof chunk === "function") {
          callback = chunk;
          chunk = undefined;
          encoding = undefined;
        } else if (typeof encoding === "function") {
          callback = encoding;
          encoding = undefined;
        }
        if (chunk !== undefined) result.write(chunk, encoding);
        if (typeof callback === "function") queueMicrotask(callback);
        return result;
      },
      destroy() {
        return result;
      },
      on,
      addListener: on,
      once(event, listener) {
        const wrapper = (...args) => {
          result.off(event, wrapper);
          listener(...args);
        };
        return on(event, wrapper);
      },
      off(event, listener) {
        const entries = listeners.get(event);
        if (entries) listeners.set(event, entries.filter((entry) => entry !== listener));
        return result;
      },
      removeListener(event, listener) {
        return result.off(event, listener);
      },
    };
    if (isTTY) {
      const [columns, rows] = fields(ops.op_process_tty_size(fd)).map(Number);
      result.columns = columns;
      result.rows = rows;
      result.getWindowSize = () => [columns, rows];
      result.getColorDepth = (environment = process.env) =>
        ttyColorSupport().getColorDepth(environment);
      result.hasColors = (count, environment) =>
        ttyColorSupport().hasColors(count, environment);
    }
    return result;
  };
  const emitWarning = (warning, type, code) => {
    const details = warning instanceof Error ? warning : new Error(String(warning));
    const name = type || details.name || "Warning";
    const warningCode = code || details.code;
    if (type) details.name = type;
    if (warningCode) details.code = warningCode;
    const prefix = warningCode ? "[" + warningCode + "] " : "";
    ops.op_thinpack_io_write(
      "(node:" + pid + ") " + prefix + name + ": " + details.message + "\n",
      true,
    );
    process.nextTick(() => process.emit("warning", details));
  };

  const monotonicNanoseconds = () => {
    const perf = globalThis.performance;
    if (perf && typeof perf.now === "function") {
      return BigInt(Math.round(perf.now() * 1e6));
    }
    return BigInt(Date.now()) * 1_000_000n;
  };
  let startNanoseconds = null;

  const hrtime = (previous) => {
    const current = monotonicNanoseconds();
    const seconds = Number(current / 1_000_000_000n);
    const nanoseconds = Number(current % 1_000_000_000n);
    if (previous === undefined) return [seconds, nanoseconds];
    let deltaSeconds = seconds - previous[0];
    let deltaNanoseconds = nanoseconds - previous[1];
    if (deltaNanoseconds < 0) {
      deltaSeconds -= 1;
      deltaNanoseconds += 1_000_000_000;
    }
    return [deltaSeconds, deltaNanoseconds];
  };
  hrtime.bigint = monotonicNanoseconds;

  let exitCode;
  let exiting = false;
  // The shared host table, so process.kill accepts every name os.constants.signals
  // exposes and the numbers match the platform.
  const SIGNALS = {};
  for (const line of ops.op_binding_signals().split("\n")) {
    const separator = line.indexOf("=");
    if (separator <= 0) continue;
    SIGNALS[line.slice(0, separator)] = Number(line.slice(separator + 1));
  }
  const signalNumber = (signal) => {
    if (signal === undefined) return SIGNALS.SIGTERM;
    if (typeof signal === "number") return signal;
    const number = SIGNALS[String(signal)];
    if (number !== undefined) return number;
    const error = new Error(`Unknown signal: ${signal}`);
    error.code = "ERR_UNKNOWN_SIGNAL";
    throw error;
  };
  const processListeners = new Map();
  const addProcessListener = (event, listener, prepend = false) => {
    if (!processListeners.has(event)) processListeners.set(event, []);
    const entries = processListeners.get(event);
    if (prepend) entries.unshift(listener);
    else entries.push(listener);
    return process;
  };
  const removeProcessListener = (event, listener) => {
    const entries = processListeners.get(event);
    if (entries !== undefined) {
      processListeners.set(event, entries.filter((entry) => entry !== listener));
    }
    return process;
  };

  const addOnceListener = (event, listener, prepend) => {
    const wrapper = (...args) => {
      removeProcessListener(event, wrapper);
      listener(...args);
    };
    return addProcessListener(event, wrapper, prepend);
  };
  // Node's process is an EventEmitter, so an unset limit follows the live
  // `EventEmitter.defaultMaxListeners`. `events` is optional: probe the builtin
  // list before loading it, and keep Node's initial default when it is absent.
  const defaultMaxListeners = () => {
    if (!globalThis.__thinpack_builtin_modules.includes("node:events")) return 10;
    return globalThis.__thinpack_require__("node:events").defaultMaxListeners;
  };

  const process = {
    argv,
    argv0: executable,
    execPath: executable,
    execArgv: [],
    version: "v26.13.1",
    versions: {
      node: "26.13.1",
      thinpack: "0.3.0",
      undici: "8.12.4",
      v8: "14.9.0",
    },
    release: { name: "node" },
    config: { variables: {}, target_defaults: {} },
    title: "node",
    mainModule: undefined,
    resourceUsage: () => {
      const values = ops.op_process_resource_usage().split("\0").map(Number);
      return {
        userCPUTime: values[0],
        systemCPUTime: values[1],
        maxRSS: values[2],
        sharedMemorySize: values[3],
        unsharedDataSize: values[4],
        unsharedStackSize: values[5],
        minorPageFault: values[6],
        majorPageFault: values[7],
        swappedOut: values[8],
        fsRead: values[9],
        fsWrite: values[10],
        ipcSent: values[11],
        ipcReceived: values[12],
        signalsCount: values[13],
        voluntaryContextSwitches: values[14],
        involuntaryContextSwitches: values[15],
      };
    },
    env,
    on: (event, listener) => addProcessListener(event, listener),
    addListener: (event, listener) => addProcessListener(event, listener),
    prependListener: (event, listener) => addProcessListener(event, listener, true),
    once: (event, listener) => addOnceListener(event, listener, false),
    prependOnceListener: (event, listener) => addOnceListener(event, listener, true),
    off: (event, listener) => removeProcessListener(event, listener),
    removeListener: (event, listener) => removeProcessListener(event, listener),
    removeAllListeners: (event) => {
      if (event === undefined) processListeners.clear();
      else processListeners.delete(event);
      return process;
    },
    emit: (event, ...args) => {
      const entries = processListeners.get(event) ?? [];
      for (const listener of entries.slice()) listener(...args);
      return entries.length > 0;
    },
    listeners: (event) => (processListeners.get(event) ?? []).slice(),
    listenerCount: (event) => (processListeners.get(event) ?? []).length,
    _maxListeners: undefined,
    getMaxListeners() {
      if (this._maxListeners !== undefined) return this._maxListeners;
      return defaultMaxListeners();
    },
    setMaxListeners(n) {
      internalValidators().validateNumber(n, "setMaxListeners", 0);
      this._maxListeners = n;
      return this;
    },
    features: {
      inspector: false,
      debug: false,
      uv: true,
      ipv6: true,
      tls: true,
      tls_alpn: true,
      tls_sni: true,
      tls_ocsp: false,
      tls_openssl: false,
      sea: false,
      cached_builtins: false,
    },
    getBuiltinModule: (id) => {
      const specifier = String(id);
      return globalThis.__thinpack_require__(
        specifier.startsWith("node:") ? specifier : "node:" + specifier,
      );
    },
    memoryUsage: () => {
      const [rss, heapTotal, heapUsed, external, arrayBuffers] =
        fields(ops.op_process_memory()).map(Number);
      return { rss, heapTotal, heapUsed, external, arrayBuffers };
    },
    cpuUsage: (previous) => {
      const [user, system] = fields(ops.op_process_cpu_usage()).map(Number);
      if (previous === undefined) return { user, system };
      return { user: user - previous.user, system: system - previous.system };
    },
    ...(platform === "win32"
      ? {}
      : {
        getuid: () => uid,
        getgid: () => gid,
        geteuid: () => euid,
        getegid: () => egid,
      }),
    umask: (mask) => {
      if (mask === undefined) return ops.op_process_umask(-1);
      const value = typeof mask === "string" ? Number.parseInt(mask, 8) : mask;
      if (!Number.isInteger(value) || value < 0 || value > 0o777) {
        throw invalidArgument("mask", "integer");
      }
      return ops.op_process_umask(value);
    },
    hrtime,
    uptime: () => {
      if (startNanoseconds === null) startNanoseconds = monotonicNanoseconds();
      return Number(monotonicNanoseconds() - startNanoseconds) / 1e9;
    },
    platform,
    arch,
    pid,
    stdin: (() => {
      const listeners = new Map();
      // Node 26 reports this for a pipe-backed socket. Pull readers stop at
      // this many buffered units and resume the read op from `read()`.
      let highWaterMark = 64 * 1024;
      const queue = [];
      let queuedLength = 0;
      let decoder = null;
      // A readable listener pulls chunks, a data listener consumes all of them.
      let demand = false;
      let flowing = false;
      let paused = false;
      let reading = false;
      let ended = false;
      let endEmitted = false;
      let readableScheduled = false;
      let pending = null;
      const emit = (event, ...args) => {
        for (const listener of listeners.get(event) ?? []) listener(...args);
      };
      const listening = (event) => (listeners.get(event)?.length ?? 0) > 0;
      const take = (size) => {
        const chunk = queue[0];
        if (size >= chunk.length) {
          queue.shift();
          queuedLength -= chunk.length;
          return chunk;
        }
        queue[0] = chunk.slice(size);
        queuedLength -= size;
        return chunk.slice(0, size);
      };
      // A multi-piece read allocates its result once instead of recopying the
      // prefix for every chunk it joins.
      const collect = (amount) => {
        if (amount === 0) return null;
        if (amount <= queue[0].length) return take(amount);
        const text = typeof queue[0] === "string";
        let chunk = text ? "" : Buffer.allocUnsafe(amount);
        let offset = 0;
        while (amount > 0) {
          const piece = take(Math.min(amount, queue[0].length));
          if (text) chunk += piece;
          else {
            chunk.set(piece, offset);
            offset += piece.length;
          }
          amount -= piece.length;
        }
        return chunk;
      };
      const finish = () => {
        if (endEmitted) return;
        endEmitted = true;
        nextTick(() => emit("end"));
      };
      const drain = () => {
        while (flowing && queue.length > 0) {
          const chunk = queue.shift();
          queuedLength -= chunk.length;
          emit("data", chunk);
        }
        if (ended && queuedLength === 0) finish();
      };
      const deliver = (chunk) => {
        if (chunk.length === 0) return;
        queue.push(chunk);
        queuedLength += chunk.length;
        if (listening("readable")) emit("readable");
        drain();
      };
      // A sized read can leave data buffered; Node signals another `readable`
      // for it instead of waiting for more bytes from the pipe.
      const signalReadable = () => {
        if (readableScheduled || flowing || paused || !demand || queuedLength === 0) return;
        readableScheduled = true;
        nextTick(() => {
          readableScheduled = false;
          if (queuedLength > 0 && demand && !flowing && !paused) emit("readable");
        });
      };
      // Node raises the high-water mark to the next power of two when a sized
      // read asks for more, so the pump keeps pulling until it can satisfy it.
      const MAX_HWM = 0x40000000;
      const demandFor = (size) => {
        if (size <= highWaterMark) return;
        if (size > MAX_HWM) {
          const { codes } = internalErrors();
          throw new codes.ERR_OUT_OF_RANGE("size", "<= 1GiB", size);
        }
        let bound = 1;
        while (bound < size) bound *= 2;
        highWaterMark = bound;
      };
      const more = () => {
        if (ended || paused || !(flowing || demand)) return;
        if (!flowing && queuedLength >= highWaterMark) return;
        pump();
      };
      const pump = () => {
        if (reading || ended) return;
        reading = true;
        const read = ops.op_thinpack_io_read_stdin();
        pending = read;
        if (paused) Deno.core.unrefOpPromise(read);
        read.then(
          (bytes) => {
            reading = false;
            pending = null;
            if (bytes.length === 0) {
              ended = true;
              // A trailing partial character is only decodable at EOF.
              if (decoder !== null) {
                const tail = decoder.decode();
                if (tail.length > 0) deliver(tail);
              }
              if (queuedLength === 0) finish();
              else signalReadable();
              return;
            }
            deliver(
              decoder === null
                ? Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength)
                : decoder.decode(bytes, { stream: true }),
            );
            more();
          },
          // Only the read op rejects here. A throwing listener rejects the
          // promise from the fulfilled handler and must stay an uncaught
          // consumer error, not a stream `error` event.
          (error) => {
            reading = false;
            pending = null;
            emit("error", error);
          },
        );
      };
      return {
        fd: 0,
        isTTY: ops.op_thinpack_io_isatty(0),
        readable: true,
        on(event, listener) {
          if (!listeners.has(event)) listeners.set(event, []);
          listeners.get(event).push(listener);
          if (event === "data") {
            flowing = true;
            paused = false;
            if (pending !== null) Deno.core.refOpPromise(pending);
            drain();
            more();
          } else if (event === "readable") {
            demand = true;
            paused = false;
            if (pending !== null) Deno.core.refOpPromise(pending);
            if (queuedLength > 0) emit("readable");
            else more();
          }
          return this;
        },
        once(event, listener) {
          const wrapper = (...args) => {
            this.removeListener(event, wrapper);
            listener(...args);
          };
          return this.on(event, wrapper);
        },
        removeListener(event, listener) {
          const list = listeners.get(event);
          if (list !== undefined) listeners.set(event, list.filter((entry) => entry !== listener));
          return this;
        },
        off(event, listener) {
          return this.removeListener(event, listener);
        },
        setEncoding(encoding) {
          // Node's StringDecoder keeps a leading BOM, unlike a default
          // TextDecoder.
          decoder = new TextDecoder(String(encoding), { ignoreBOM: true });
          return this;
        },
        read(size) {
          let requested = size;
          if (requested === undefined) requested = NaN;
          else if (!Number.isInteger(requested)) requested = Number.parseInt(requested, 10);
          const all = Number.isNaN(requested);
          const wanted = all ? queuedLength : Math.max(requested, 0);
          if (!all) demandFor(wanted);
          // Node's read() starts the stream even before a listener is attached.
          demand = true;
          if (!paused && pending !== null) Deno.core.refOpPromise(pending);
          // Node returns null until the requested size is buffered, except
          // at EOF, where it returns what is left.
          if (!all && wanted > queuedLength && !ended) {
            more();
            return null;
          }
          const chunk = collect(Math.min(wanted, queuedLength));
          if (ended && queuedLength === 0) finish();
          else {
            more();
            if (chunk !== null) signalReadable();
          }
          return chunk;
        },
        // Node's stdin is a Readable; this object is not, so `for await` and
        // `node:stream/consumers` need the pull loop here. The generator
        // serializes next() calls and runs its finally block on early return.
        async *[Symbol.asyncIterator]() {
          try {
            for (;;) {
              const chunk = this.read();
              if (chunk !== null) {
                yield chunk;
                continue;
              }
              if (ended && queuedLength === 0) return;
              await new Promise((resolve, reject) => {
                const settle = () => {
                  this.removeListener("readable", settle);
                  this.removeListener("end", settle);
                  this.removeListener("error", fail);
                  resolve();
                };
                const fail = (error) => {
                  this.removeListener("readable", settle);
                  this.removeListener("end", settle);
                  this.removeListener("error", fail);
                  reject(error);
                };
                this.on("error", fail);
                this.on("end", settle);
                this.on("readable", settle);
              });
            }
          } finally {
            paused = true;
            if (pending !== null) Deno.core.unrefOpPromise(pending);
          }
        },
        pause() {
          paused = true;
          flowing = false;
          if (pending !== null) Deno.core.unrefOpPromise(pending);
          return this;
        },
        resume() {
          paused = false;
          flowing = true;
          if (pending !== null) Deno.core.refOpPromise(pending);
          drain();
          more();
          return this;
        },
        get isRaw() {
          return stdinRawMode;
        },
        setRawMode(mode) {
          setRawMode(mode);
          return this;
        },
      };
    })(),
    stdout: stream(false, 1),
    stderr: stream(true, 2),
    cwd: () => ops.op_process_cwd(),
    chdir: (directory) => {
      const path = String(directory);
      try {
        ops.op_process_chdir(path);
      } catch (error) {
        const errno = Number(error.message);
        if (!Number.isInteger(errno)) throw error;
        const { UVException } = internalErrors();
        throw new UVException({ errno, syscall: "chdir", path });
      }
    },
    nextTick,
    emitWarning,
    kill: (pid, signal) => {
      try {
        ops.op_process_kill(pid | 0, signalNumber(signal));
      } catch (error) {
        const errno = Number(String(error.message).split("\0")[0]);
        if (!Number.isInteger(errno)) throw error;
        const { UVException } = internalErrors();
        throw new UVException({ errno, syscall: "kill" });
      }
      return true;
    },
    exit: (code) => {
      if (exiting) return;
      exiting = true;
      if (code !== undefined) exitCode = code | 0;
      const resolved = exitCode === undefined ? 0 : exitCode | 0;
      process.emit("exit", resolved);
      ops.op_process_exit(exitCode === undefined ? resolved : exitCode | 0);
    },
  };

  Object.defineProperty(process, "exitCode", {
    get: () => exitCode,
    set: (value) => {
      exitCode = value;
    },
  });

  globalThis.process = process;
  // Compat marker, only present when the build includes `node:crypto`: the value
  // is the OpenSSL version bundled with Node 26.13.1. This runtime's crypto is
  // Rust crates (ring/aes), not OpenSSL, but consumers such as
  // `util.isKeyObject` gate on the field being set. The builtin list is read
  // lazily because this startup script runs before the loader's op state
  // carries it.
  Object.defineProperty(process.versions, "openssl", {
    enumerable: true,
    configurable: true,
    get() {
      const value = globalThis.__thinpack_builtin_modules.includes("node:crypto")
        ? "3.5.9"
        : undefined;
      Object.defineProperty(process.versions, "openssl", {
        value,
        enumerable: true,
      });
      return value;
    },
  });
  __thinpack_on_finish(restoreTerminal);
  __thinpack_on_finish(() => {
    if (exiting) return;
    exiting = true;
    process.emit("exit", exitCode === undefined ? 0 : exitCode | 0);
    if (exitCode !== undefined) {
      ops.op_process_exit(exitCode | 0);
    }
  });
  Object.defineProperty(Deno, "args", {
    value: process.argv.slice(2),
    configurable: true,
    writable: true,
  });

})();
