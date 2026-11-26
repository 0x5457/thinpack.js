(() => {
  const { codes, opErrorParts } = require("node:thinpack/internal/errors");
  const {
    Child,
    signalName,
  } = require("node:thinpack/internal/child_process");
  const ipc = require("node:thinpack/internal/child_process/channel");

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

  const stdioError = (code, message) => {
    const error = new Error(message);
    error.code = code;
    return error;
  };

  // Node's `stdio` option, narrowed to the connections this runtime supports. A standard
  // slot is piped, inherited, ignored, or an explicit descriptor moved to that slot; one
  // further slot may be `'ipc'`. Slots beyond stderr may only be ignored or `'ipc'`, since
  // the runtime exposes no `child.stdio` array for extra pipes.
  const stdioOf = (options, sync = false) => {
    const value = options.stdio ?? "pipe";
    let entries;
    if (typeof value === "string") {
      if (value === "inherit") entries = [0, 1, 2];
      else if (value === "ignore" || value === "overlapped" || value === "pipe") {
        entries = [value, value, value];
      } else {
        throw new codes.ERR_INVALID_ARG_VALUE("stdio", value);
      }
    } else if (Array.isArray(value)) {
      entries = [...value];
    } else {
      throw new codes.ERR_INVALID_ARG_VALUE("stdio", value);
    }
    while (entries.length < 3) entries.push(undefined);

    const streams = [];
    let ipcSlot = -1;
    const useIpc = (index) => {
      if (ipcSlot !== -1) {
        throw stdioError("ERR_IPC_ONE_PIPE", "Child process can have only one IPC pipe");
      }
      ipcSlot = index;
    };
    for (let index = 0; index < 3; index += 1) {
      const entry = entries[index] ?? "pipe";
      if (entry === "pipe" || entry === "overlapped") {
        streams.push("pipe");
      } else if (entry === "ignore") {
        streams.push("ignore");
      } else if (entry === "inherit") {
        streams.push("inherit");
      } else if (entry === "ipc") {
        useIpc(index);
        // The IPC descriptor replaces this stream in the child.
        streams.push("ignore");
      } else if (typeof entry === "number") {
        if (entry < 0) streams.push("pipe");
        else if (entry === index) streams.push("inherit");
        else streams.push(`fd:${entry}`);
      } else {
        throw new codes.ERR_INVALID_ARG_VALUE("stdio", entry);
      }
    }
    for (let index = 3; index < entries.length; index += 1) {
      const entry = entries[index] ?? "ignore";
      if (entry === "ignore") continue;
      if (entry === "ipc") {
        useIpc(index);
        continue;
      }
      throw new codes.ERR_INVALID_ARG_VALUE("stdio", entry);
    }
    if (sync && ipcSlot !== -1) {
      throw stdioError("ERR_IPC_SYNC_FORK", "IPC cannot be used with synchronous forks");
    }
    return { streams, ipc: ipcSlot };
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
      const { streams, ipc: ipcSlot } = stdioOf(this.#options);
      const serialization = this.#options.serialization;
      if (serialization !== undefined && serialization !== "json") {
        if (serialization !== "advanced") {
          throw new codes.ERR_INVALID_ARG_VALUE("options.serialization", serialization);
        }
        if (ipcSlot !== -1) {
          throw new codes.ERR_NOT_IMPLEMENTED("child_process serialization: advanced");
        }
      }
      const argv = [this.spawnfile, ...this.spawnargs.slice(1)].join("\0");
      let spawned;
      try {
        spawned = Deno.core.ops.op_child_spawn(
          argv,
          envString(this.#options.env),
          this.#options.cwd ? String(this.#options.cwd) : "",
          streams.join("\0"),
          ipcSlot,
        );
      } catch (error) {
        queueMicrotask(() => this.emit("error", spawnError(error, this.spawnfile)));
        return;
      }
      const [id, pid, channel, fd] = spawned.split("\0").map(Number);
      this._attach(id, pid, {
        stdin: streams[0] === "pipe",
        stdout: streams[1] === "pipe",
        stderr: streams[2] === "pipe",
      });
      if (channel >= 0) {
        ipc.attach(this, channel, {
          fd,
          connect: Deno.core.ops.op_child_channel_accept(channel, id),
        });
      }
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
    const { streams } = stdioOf(options, true);
    const argv = [String(file), ...args.map(String)].join("\0");
    let payload;
    try {
      payload = Deno.core.ops.op_child_spawn_sync(
        argv,
        envString(options.env),
        options.cwd ? String(options.cwd) : "",
        streams.join("\0"),
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
    const stdout = streams[1] === "pipe"
      ? Buffer.from(buffer.subarray(20, 20 + stdoutLength))
      : null;
    const stderr = streams[2] === "pipe"
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
