(() => {
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");
  const {
    Child,
    signalName,
  } = require("node:thinpack/internal/child_process");

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

  // `exec` runs the command through the platform shell; Windows resolves the shell the
  // way Node does, through ComSpec.
  const shell = () =>
    process.platform === "win32"
      ? [process.env.ComSpec ?? "cmd.exe", "/d", "/s", "/c"]
      : ["/bin/sh", "-c"];

  const envString = (env) => {
    if (!env) return "";
    return Object.entries(env)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => `${key}=${value}`)
      .join("\0");
  };

  // Node's `stdio` option, narrowed to the connections this runtime supports: each of the
  // three standard streams is piped, inherited from the parent or ignored. An explicit
  // descriptor is accepted in its own position, so `[0, 1, 2]` is the long form of
  // `'inherit'`; streams and extra descriptors are not supported.
  const stdioOf = (options) => {
    const value = options.stdio || "pipe";
    const entries = typeof value === "string" ? [value, value, value] : value;
    if (!Array.isArray(entries) || entries.length > 3) {
      throw new codes.ERR_INVALID_ARG_VALUE("stdio", value);
    }
    return [0, 1, 2].map((index) => {
      const entry = entries[index] ?? "pipe";
      const mode = typeof entry === "number" && entry === index ? "inherit" : entry;
      if (mode !== "pipe" && mode !== "inherit" && mode !== "ignore") {
        throw new codes.ERR_INVALID_ARG_VALUE("stdio", entry);
      }
      return mode;
    });
  };

  class ChildProcess extends Child {
    #options;

    constructor(command, args, options) {
      super();
      this.spawnfile = command;
      this.spawnargs = [command, ...args];
      this.#options = options;
    }

    start() {
      const stdio = stdioOf(this.#options);
      const argv = [this.spawnfile, ...this.spawnargs.slice(1)].join("\0");
      let spawned;
      try {
        spawned = Deno.core.ops.op_child_spawn(
          argv,
          envString(this.#options.env),
          this.#options.cwd ? String(this.#options.cwd) : "",
          stdio.join("\0"),
        );
      } catch (error) {
        queueMicrotask(() => this.emit("error", spawnError(error, this.spawnfile)));
        return;
      }
      const [id, pid] = spawned.split("\0").map(Number);
      this._attach(id, pid, {
        stdin: stdio[0] === "pipe",
        stdout: stdio[1] === "pipe",
        stderr: stdio[2] === "pipe",
      });
      queueMicrotask(() => this.emit("spawn"));
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
    // Node's `execFile` always pipes the child and does not forward `options.stdio`; its
    // callback needs the streams regardless of what the caller passed.
    const child = spawn(file, Array.isArray(args) ? args : [], { ...options, stdio: "pipe" });
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
    const [file, ...flags] = shell();
    return execFile(file, [...flags, String(command)], options, callback);
  };

  const runSync = (file, args, options = {}) => {
    const stdio = stdioOf(options);
    const argv = [String(file), ...args.map(String)].join("\0");
    let payload;
    try {
      payload = Deno.core.ops.op_child_spawn_sync(
        argv,
        envString(options.env),
        options.cwd ? String(options.cwd) : "",
        stdio.join("\0"),
        options.input === undefined ? new Uint8Array() : toBytes(options.input),
      );
    } catch (error) {
      return { error: spawnError(error, String(file)), status: null, signal: null, stdout: null, stderr: null };
    }
    const buffer = Buffer.from(payload.buffer ?? payload, payload.byteOffset ?? 0, payload.byteLength);
    // Header layout: status (i32), signal (i32), pid (u32), stdout length (u32),
    // stderr length (u32), all little-endian; then the two outputs.
    const status = buffer.readInt32LE(0);
    const signal = signalName(buffer.readInt32LE(4));
    const pid = buffer.readUInt32LE(8);
    const stdoutLength = buffer.readUInt32LE(12);
    const stderrLength = buffer.readUInt32LE(16);
    const stdout = stdio[1] === "pipe"
      ? Buffer.from(buffer.subarray(20, 20 + stdoutLength))
      : null;
    const stderr = stdio[2] === "pipe"
      ? Buffer.from(buffer.subarray(20 + stdoutLength, 20 + stdoutLength + stderrLength))
      : null;
    const encoding = options.encoding;
    return {
      status: status >= 0 ? status : null,
      signal,
      stdout: stdout && encoding ? stdout.toString(encoding) : stdout,
      stderr: stderr && encoding ? stderr.toString(encoding) : stderr,
      output: [null, stdout, stderr],
      pid,
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
    const [file, ...flags] = shell();
    const result = runSync(file, [...flags, String(command)], options);
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
