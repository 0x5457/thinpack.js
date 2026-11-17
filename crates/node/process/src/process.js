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
  const COLORS_2 = 1;
  const COLORS_16 = 4;
  const COLORS_256 = 8;
  const COLORS_16M = 24;
  const TERM_ENVS = {
    eterm: COLORS_16, cons25: COLORS_16, console: COLORS_16, cygwin: COLORS_16,
    dtterm: COLORS_16, gnome: COLORS_16, hurd: COLORS_16, jfbterm: COLORS_16,
    konsole: COLORS_16, kterm: COLORS_16, mlterm: COLORS_16, mosh: COLORS_16M,
    putty: COLORS_16, st: COLORS_16, "rxvt-unicode-24bit": COLORS_16M,
    terminator: COLORS_16M, "xterm-kitty": COLORS_16M,
  };
  const CI_ENVS = [
    ["APPVEYOR", COLORS_256], ["BUILDKITE", COLORS_256],
    ["CIRCLECI", COLORS_16M], ["DRONE", COLORS_256],
    ["GITEA_ACTIONS", COLORS_16M], ["GITHUB_ACTIONS", COLORS_16M],
    ["GITLAB_CI", COLORS_256], ["TRAVIS", COLORS_256],
  ];
  const TERM_ENVS_REG_EXP = [
    /ansi/, /color/, /linux/, /direct/, /^con[0-9]*x[0-9]/, /^rxvt/,
    /^screen/, /^xterm/, /^vt100/, /^vt220/,
  ];

  const getColorDepth = (environment = env) => {
    if (environment.FORCE_COLOR !== undefined) {
      switch (environment.FORCE_COLOR) {
        case "":
        case "1":
        case "true":
          return COLORS_16;
        case "2":
          return COLORS_256;
        case "3":
          return COLORS_16M;
        default:
          return COLORS_2;
      }
    }

    if (
      (environment.NODE_DISABLE_COLORS !== undefined && environment.NODE_DISABLE_COLORS !== "") ||
      (environment.NO_COLOR !== undefined && environment.NO_COLOR !== "") ||
      environment.TERM === "dumb"
    ) {
      return COLORS_2;
    }

    if (environment.TMUX) return COLORS_16M;
    if (Object.hasOwn(environment, "TF_BUILD") && Object.hasOwn(environment, "AGENT_NAME")) {
      return COLORS_16;
    }

    if (Object.hasOwn(environment, "CI")) {
      for (const [envName, colors] of CI_ENVS) {
        if (Object.hasOwn(environment, envName)) return colors;
      }
      if (environment.CI_NAME === "codeship") return COLORS_256;
      return COLORS_2;
    }

    if (Object.hasOwn(environment, "TEAMCITY_VERSION")) {
      return /^(9\.(0*[1-9]\d*)\.|\d{2,}\.)/.test(environment.TEAMCITY_VERSION)
        ? COLORS_16
        : COLORS_2;
    }

    switch (environment.TERM_PROGRAM) {
      case "iTerm.app":
        return !environment.TERM_PROGRAM_VERSION || /^[0-2]\./.test(environment.TERM_PROGRAM_VERSION)
          ? COLORS_256
          : COLORS_16M;
      case "HyperTerm":
      case "MacTerm":
        return COLORS_16M;
      case "Apple_Terminal":
        return COLORS_256;
    }

    const colorterm = environment.COLORTERM;
    if (colorterm === "truecolor" || colorterm === "24bit") return COLORS_16M;

    const term = environment.TERM;
    if (term) {
      if (/truecolor/.test(term)) return COLORS_16M;
      if (/^xterm-256/.test(term)) return COLORS_256;
      const name = term.toLowerCase();
      if (Object.hasOwn(TERM_ENVS, name)) return TERM_ENVS[name];
      if (TERM_ENVS_REG_EXP.some((pattern) => pattern.test(name))) return COLORS_16;
    }

    return colorterm ? COLORS_16 : COLORS_2;
  };

  const hasColors = (count, environment) => {
    if (
      environment === undefined &&
      (count === undefined || (typeof count === "object" && count !== null))
    ) {
      environment = count;
      count = 16;
    } else if (!Number.isInteger(count) || count < 2) {
      throw invalidArgument("count", "an integer >= 2");
    }
    return count <= 2 ** getColorDepth(environment);
  };
  const setRawMode = (fd, mode) => {
    let raw;
    if (mode === "io" || mode === "raw") {
      raw = true;
    } else if (typeof mode === "string") {
      const { codes } = internalErrors();
      throw new codes.ERR_INVALID_ARG_VALUE(
        "mode", mode, "must be true, false, 'raw', or 'io'",
      );
    } else {
      raw = Boolean(mode);
    }
    if (!raw && !ops.op_process_is_raw(fd)) return;
    try {
      ops.op_process_set_raw_mode(fd, raw);
    } catch (error) {
      const errno = Number(error.message);
      if (!Number.isInteger(errno)) throw error;
      const { ErrnoException } = internalErrors();
      throw new ErrnoException(errno, "setRawMode");
    }
  };

  const restoreTerminal = () => {
    if (ops.op_process_is_raw(0)) ops.op_process_set_raw_mode(0, false);
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
      result.getColorDepth = getColorDepth;
      result.hasColors = hasColors;
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
  const SIGNALS = {
    SIGABRT: 6, SIGALRM: 14, SIGBUS: 7, SIGCHLD: 17, SIGCONT: 18, SIGFPE: 8,
    SIGHUP: 1, SIGILL: 4, SIGINT: 2, SIGKILL: 9, SIGPIPE: 13, SIGQUIT: 3,
    SIGSEGV: 11, SIGSTOP: 19, SIGTERM: 15, SIGTSTP: 20, SIGTTIN: 21,
    SIGTTOU: 22, SIGUSR1: 10, SIGUSR2: 12, SIGVTALRM: 26, SIGXCPU: 24,
    SIGXFSZ: 25,
  };
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

  const process = {
    argv,
    argv0: executable,
    execPath: executable,
    execArgv: [],
    version: "v26.13.1",
    versions: {
      node: "26.13.1",
      thinpack: "0.1.0",
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
    getuid: () => uid,
    getgid: () => gid,
    geteuid: () => euid,
    getegid: () => egid,
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
      let decoder = null;
      let reading = false;
      let paused = true;
      let ended = false;
      let pending = null;
      const emit = (event, ...args) => {
        for (const listener of listeners.get(event) ?? []) listener(...args);
      };
      const pump = () => {
        if (reading || ended) return;
        reading = true;
        const read = ops.op_thinpack_io_read_stdin();
        pending = read;
        if (paused) Deno.core.unrefOpPromise(read);
        read
          .then((bytes) => {
            reading = false;
            pending = null;
            if (bytes.length === 0) {
              ended = true;
              emit("end");
              return;
            }
            emit("data", decoder === null ? bytes : decoder.decode(bytes, { stream: true }));
            if (!paused) pump();
          })
          .catch((error) => {
            reading = false;
            pending = null;
            emit("error", error);
          });
      };
      return {
        fd: 0,
        isTTY: ops.op_thinpack_io_isatty(0),
        readable: true,
        on(event, listener) {
          if (!listeners.has(event)) listeners.set(event, []);
          listeners.get(event).push(listener);
          if (event === "data") {
            paused = false;
            pump();
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
          decoder = new TextDecoder(String(encoding));
          return this;
        },
        pause() {
          paused = true;
          if (pending !== null) Deno.core.unrefOpPromise(pending);
          return this;
        },
        resume() {
          paused = false;
          if (pending !== null) Deno.core.refOpPromise(pending);
          pump();
          return this;
        },
        get isRaw() {
          return ops.op_process_is_raw(0);
        },
        setRawMode(mode) {
          setRawMode(0, mode);
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
