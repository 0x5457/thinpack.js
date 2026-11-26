(() => {
  const { ops } = Deno.core;

  const invalidFd = (fd) => {
    const error = new RangeError('"fd" must be a positive integer: ' + fd);
    error.code = "ERR_INVALID_FD";
    return error;
  };
  const ttyInitFailed = (fd) => {
    const error = new Error("TTY initialization failed");
    error.code = "ERR_TTY_INIT_FAILED";
    error.errno = -25;
    error.syscall = "uv_tty_init";
    error.fd = fd;
    return error;
  };

  const isatty = (fd) =>
    typeof fd === "number" &&
    Number.isInteger(fd) &&
    fd >= 0 &&
    fd <= 2147483647 &&
    ops.op_thinpack_io_isatty(fd);

  const stdio = (fd) => {
    if (fd === 0) return process.stdin;
    if (fd === 1) return process.stdout;
    if (fd === 2) return process.stderr;
    return null;
  };
  const ttyTarget = (fd) => {
    if (!Number.isInteger(fd) || fd < 0) throw invalidFd(fd);
    const target = stdio(fd);
    if (target === null || !target.isTTY) throw ttyInitFailed(fd);
    return target;
  };
  const ansi = (name) => (...args) => require("node:readline")[name](...args);

  const writeTargets = new WeakMap();

  // Node's color methods read only the environment and their arguments, never
  // the stream. The shared implementation loads on first use so loading node:tty
  // neither duplicates it nor pulls in the color tables for unrelated APIs.
  let ttyColors;
  const ttyColorSupport = () =>
    ttyColors ??= require("node:thinpack/internal/tty");
  const getColorDepth = (environment = process.env) =>
    ttyColorSupport().getColorDepth(environment);
  const hasColors = (count, environment) =>
    ttyColorSupport().hasColors(count, environment);

  class WriteStream {
    constructor(fd = 1) {
      const target = ttyTarget(fd);
      writeTargets.set(this, target);
      this.fd = fd;
      this.isTTY = true;
      this.columns = target.columns;
      this.rows = target.rows;
    }

    write(chunk, encoding, callback) {
      return writeTargets.get(this).write(chunk, encoding, callback);
    }

    end(chunk, encoding, callback) {
      if (typeof chunk === "function") {
        callback = chunk;
        chunk = undefined;
      } else if (typeof encoding === "function") {
        callback = encoding;
        encoding = undefined;
      }
      if (chunk !== undefined && chunk !== null) this.write(chunk, encoding);
      if (typeof callback === "function") queueMicrotask(callback);
      return this;
    }

    getWindowSize() {
      return [this.columns, this.rows];
    }

    cursorTo(x, y, callback) {
      return ansi("cursorTo")(this, x, y, callback);
    }

    moveCursor(dx, dy, callback) {
      return ansi("moveCursor")(this, dx, dy, callback);
    }

    clearLine(dir, callback) {
      return ansi("clearLine")(this, dir, callback);
    }

    clearScreenDown(callback) {
      return ansi("clearScreenDown")(this, callback);
    }
  }

  // Node's color methods read only the environment and their arguments, never
  // the stream, so the prototype exposes them without touching one.
  WriteStream.prototype.getColorDepth = getColorDepth;
  WriteStream.prototype.hasColors = hasColors;

  class ReadStream {
    #target;

    constructor(fd = 0) {
      if (!Number.isInteger(fd) || fd < 0) throw invalidFd(fd);
      if (fd !== 0) throw ttyInitFailed(fd);
      this.#target = ttyTarget(fd);
      this.fd = 0;
      this.isTTY = true;
    }

    get isRaw() {
      return this.#target.isRaw;
    }

    setRawMode(mode) {
      this.#target.setRawMode(mode);
      return this;
    }

    setEncoding(encoding) {
      this.#target.setEncoding(encoding);
      return this;
    }

    on(event, listener) {
      this.#target.on(event, listener);
      return this;
    }

    once(event, listener) {
      this.#target.once(event, listener);
      return this;
    }

    off(event, listener) {
      this.#target.removeListener(event, listener);
      return this;
    }

    removeListener(event, listener) {
      this.#target.removeListener(event, listener);
      return this;
    }

    pause() {
      this.#target.pause();
      return this;
    }

    resume() {
      this.#target.resume();
      return this;
    }
  }

  module.exports = { isatty, WriteStream, ReadStream };
})();
