(() => {
  const { Readable, Writable } = require("node:stream");
  const { EventEmitter } = require("node:events");
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");

  const notImplemented = (name) => new codes.ERR_NOT_IMPLEMENTED(`child_process.${name}`);

  const spawnError = (error, command) => {
    const [code, detail] = opErrorParts(error);
    if (detail === undefined) return error;
    const failure = new Error(detail);
    failure.code = code;
    failure.errno = code === "ENOENT" ? -2 : undefined;
    failure.syscall = "spawn";
    failure.path = command;
    return failure;
  };

  const SIGNALS = {
    SIGABRT: 6, SIGALRM: 14, SIGBUS: 7, SIGCHLD: 17, SIGCONT: 18, SIGFPE: 8,
    SIGHUP: 1, SIGILL: 4, SIGINT: 2, SIGKILL: 9, SIGPIPE: 13, SIGQUIT: 3,
    SIGSEGV: 11, SIGSTOP: 19, SIGTERM: 15, SIGTSTP: 20, SIGTTIN: 21,
    SIGTTOU: 22, SIGUSR1: 10, SIGUSR2: 12, SIGVTALRM: 26, SIGXCPU: 24,
    SIGXFSZ: 25,
  };

  const signalNumber = (signal) => {
    if (signal === undefined || signal === null) return SIGNALS.SIGTERM;
    if (typeof signal === "number") return signal;
    const number = SIGNALS[String(signal)];
    if (number === undefined) {
      const error = new Error("Unknown signal: " + signal);
      error.code = "ERR_UNKNOWN_SIGNAL";
      throw error;
    }
    return number;
  };

  const envString = (env) => {
    if (!env) return "";
    return Object.entries(env)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => `${key}=${value}`)
      .join("\0");
  };

  class ChildProcess extends EventEmitter {
    #id = 0;
    #stdoutDone;
    #stderrDone;

    constructor(command, args, options) {
      super();
      this.spawnfile = command;
      this.spawnargs = [command, ...args];
      this.pid = undefined;
      this.exitCode = undefined;
      this.signalCode = null;
      this.killed = false;
      this.stdout = null;
      this.stderr = null;
      this.stdin = null;
      this.#options = options;
    }

    #options;

    start() {
      const argv = [this.spawnfile, ...this.spawnargs.slice(1)].join("\0");
      let id;
      try {
        id = Deno.core.ops.op_child_spawn(
          argv,
          envString(this.#options.env),
          this.#options.cwd ? String(this.#options.cwd) : "",
        );
      } catch (error) {
        queueMicrotask(() => this.emit("error", spawnError(error, this.spawnfile)));
        return;
      }
      this.#id = id;
      this.pid = Deno.core.ops.op_child_pid(id);
      this.stdout = this.#pipe(1);
      this.stderr = this.#pipe(2);
      this.stdin = this.#stdinPipe();
      queueMicrotask(() => this.emit("spawn"));

      const wait = Deno.core.ops.op_child_wait(id).then((code) => {
        this.exitCode = code;
        this.emit("exit", code, null);
        return Promise.all([this.#stdoutDone, this.#stderrDone]).then(() => {
          this.emit("close", code, null);
        });
      });
      wait.catch((error) => this.emit("error", spawnError(error, this.spawnfile)));
    }

    #pipe(which) {
      let resolveDone;
      const done = new Promise((resolve) => {
        resolveDone = resolve;
      });
      if (which === 1) this.#stdoutDone = done;
      else this.#stderrDone = done;
      const stream = new Readable({ read() {} });
      const pump = () => {
        Deno.core.ops
          .op_child_read(this.#id, which)
          .then((chunk) => {
            if (chunk.length === 0) {
              resolveDone();
              stream.push(null);
              return;
            }
            stream.push(chunk);
            pump();
          })
          .catch((error) => {
            resolveDone();
            stream.destroy(spawnError(error, this.spawnfile));
          });
      };
      pump();
      return stream;
    }

    #stdinPipe() {
      const child = this;
      return new Writable({
        write(chunk, encoding, callback) {
          Deno.core.ops
            .op_child_write(child.#id, chunk)
            .then(() => callback())
            .catch((error) => callback(spawnError(error, child.spawnfile)));
        },
        final(callback) {
          Deno.core.ops
            .op_child_close_stdin(child.#id)
            .then(() => callback())
            .catch((error) => callback(spawnError(error, child.spawnfile)));
        },
      });
    }

    kill(signal) {
      try {
        Deno.core.ops.op_child_kill(this.#id, signalNumber(signal));
      } catch (error) {
        queueMicrotask(() => this.emit("error", error));
        return false;
      }
      this.killed = true;
      return true;
    }

    ref() {
      return this;
    }

    unref() {
      return this;
    }
  }

  const spawn = (command, args = [], options = {}) => {
    const normalizedArgs = Array.isArray(args) ? args : [];
    const normalizedOptions = Array.isArray(args) ? options ?? {} : args ?? {};
    const child = new ChildProcess(
      String(command),
      normalizedArgs.map(String),
      normalizedOptions,
    );
    child.start();
    return child;
  };

  const execFile = (file, args = [], options = {}, callback) => {
    if (typeof args === "function") {
      callback = args;
      args = [];
      options = {};
    } else if (typeof options === "function") {
      callback = options;
      options = {};
    }
    const child = spawn(file, Array.isArray(args) ? args : [], options);
    const encoding = options?.encoding ?? "utf8";
    let stdout = "";
    let stderr = "";
    child.stdout?.setEncoding(encoding);
    child.stderr?.setEncoding(encoding);
    child.stdout?.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr?.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => callback?.(error, stdout, stderr));
    child.on("close", (code) => {
      if (code === 0) {
        callback?.(null, stdout, stderr);
        return;
      }
      const error = new Error(`Command failed: ${file} ${args.join(" ")}\n${stderr}`);
      error.code = code;
      error.stdout = stdout;
      error.stderr = stderr;
      callback?.(error, stdout, stderr);
    });
    return child;
  };

  const exec = (command, options = {}, callback) => {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    return execFile("/bin/sh", ["-c", String(command)], options, callback);
  };

  const runSync = (file, args, options = {}) => {
    const argv = [String(file), ...args.map(String)].join("\0");
    let payload;
    try {
      payload = Deno.core.ops.op_child_spawn_sync(
        argv,
        envString(options.env),
        options.cwd ? String(options.cwd) : "",
        options.input === undefined ? new Uint8Array() : toBytes(options.input),
      );
    } catch (error) {
      return { error: spawnError(error, String(file)), status: null, signal: null, stdout: null, stderr: null };
    }
    const buffer = Buffer.from(payload.buffer ?? payload, payload.byteOffset ?? 0, payload.byteLength);
    const status = buffer.readInt32LE(0);
    const stdoutLength = buffer.readUInt32LE(4);
    const stderrLength = buffer.readUInt32LE(8);
    const stdout = Buffer.from(buffer.subarray(12, 12 + stdoutLength));
    const stderr = Buffer.from(buffer.subarray(12 + stdoutLength, 12 + stdoutLength + stderrLength));
    const encoding = options.encoding;
    return {
      status,
      signal: null,
      stdout: encoding ? stdout.toString(encoding) : stdout,
      stderr: encoding ? stderr.toString(encoding) : stderr,
      output: [null, stdout, stderr],
      pid: 0,
      error: undefined,
    };
  };

  const toBytes = (data) => {
    if (typeof data === "string") return new Uint8Array(Buffer.from(data));
    if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    return new Uint8Array(data);
  };

  const spawnSync = (command, args = [], options = {}) => {
    const normalizedArgs = Array.isArray(args) ? args : [];
    const normalizedOptions = Array.isArray(args) ? options ?? {} : args ?? {};
    return runSync(String(command), normalizedArgs, normalizedOptions);
  };

  const execFileSync = (file, args = [], options = {}) => {
    if (!Array.isArray(args)) {
      options = args ?? {};
      args = [];
    }
    const result = runSync(String(file), args, options);
    if (result.error) throw result.error;
    if (result.status !== 0) {
      const error = new Error(`Command failed: ${file} ${args.join(" ")}`);
      error.status = result.status;
      error.stdout = result.stdout;
      error.stderr = result.stderr;
      throw error;
    }
    return result.stdout;
  };

  const execSync = (command, options = {}) => {
    const result = runSync("/bin/sh", ["-c", String(command)], options);
    if (result.error) throw result.error;
    if (result.status !== 0) {
      const error = new Error(`Command failed: ${command}`);
      error.status = result.status;
      error.stdout = result.stdout;
      error.stderr = result.stderr;
      throw error;
    }
    return result.stdout;
  };

  module.exports = {
    ChildProcess,
    spawn,
    execFile,
    exec,
    spawnSync,
    execFileSync,
    execSync,
    fork: () => {
      throw notImplemented("fork");
    },
  };
})();
