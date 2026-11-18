(() => {
  const { EventEmitter } = require("node:events");
  const { Buffer } = require("node:buffer");
  const { codes, UVException } = require("node:thinpack/internal/errors");
  const { setInterval } = require("node:timers");
  const { validateIgnoreOption } = require("node:thinpack/internal/validators");

  const kFSWatchStart = Symbol("kFSWatchStart");
  const kFSStatWatcherStart = Symbol("kFSStatWatcherStart");
  const kFSStatWatcherAddOrCleanRef = Symbol("kFSStatWatcherAddOrCleanRef");
  const WATCH_POLL_MS = 50;

  class FSWatcher extends EventEmitter {
    #id = null;
    #timer = null;
    #encoding = "utf8";
    #closed = false;
    #queue = [];
    #waiters = [];
    #ignoreMatcher = null;

    [kFSWatchStart](path, persistent = true, recursive = false, encoding = "utf8",
                    ignore, throwIfNoEntry = true) {
      if (recursive) {
        throw new codes.ERR_NOT_IMPLEMENTED("fs.watch with { recursive: true }");
      }
      validateIgnoreOption(ignore, "options.ignore");
      this.#ignoreMatcher = createIgnoreMatcher(ignore);
      this.#encoding = encoding;
      const fs = require("node:fs");
      if (!fs.existsSync(path)) {
        // Like Node's watcher when the FSEvent handle fails to start, a missing
        // entry with `throwIfNoEntry: false` leaves the watcher inert instead of
        // throwing.
        if (throwIfNoEntry === false) return this;
        throw new UVException({ errno: -2, syscall: "watch", path: String(path) });
      }
      this.#id = Deno.core.ops.op_fs_watch_start(String(path));
      this.#timer = setInterval(() => this.#poll(), WATCH_POLL_MS);
      if (!persistent) this.#timer.unref();
      return this;
    }

    #poll() {
      if (this.#closed) return;
      const payload = Deno.core.ops.op_fs_watch_take(this.#id);
      if (payload === "") return;
      const parts = payload.split("\0");
      for (let index = 0; index + 1 < parts.length; index += 2) {
        const eventType = parts[index];
        const filename = this.#encoding === "buffer"
          ? Buffer.from(parts[index + 1])
          : parts[index + 1];
        if (this.#ignoreMatcher?.(filename)) continue;
        this.emit("change", eventType, filename);
        this.#queue.push({ eventType, filename });
      }
      this.#flush();
    }

    #flush() {
      while (this.#waiters.length > 0 && this.#queue.length > 0) {
        this.#waiters.shift().resolve({ value: this.#queue.shift(), done: false });
      }
      if (this.#closed) {
        while (this.#waiters.length > 0) {
          this.#waiters.shift().resolve({ value: undefined, done: true });
        }
      }
    }

    next() {
      if (this.#queue.length > 0) {
        return Promise.resolve({ value: this.#queue.shift(), done: false });
      }
      if (this.#closed) {
        return Promise.resolve({ value: undefined, done: true });
      }
      return new Promise((resolve) => this.#waiters.push({ resolve }));
    }

    return() {
      this.close();
      return Promise.resolve({ value: undefined, done: true });
    }

    [Symbol.asyncIterator]() {
      return this;
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      if (this.#timer !== null) {
        clearInterval(this.#timer);
        this.#timer = null;
      }
      if (this.#id !== null) {
        Deno.core.ops.op_fs_watch_close(this.#id);
        this.#id = null;
      }
      this.#flush();
      this.emit("close");
    }

    ref() {
      this.#timer?.ref();
    }

    unref() {
      this.#timer?.unref();
    }
  }

  class StatWatcher extends EventEmitter {
    #bigint;
    #filename;
    #interval = 5007;
    #timer = null;
    #previous = null;
    #refs = 1;
    #closed = false;

    constructor(bigint) {
      super();
      this.#bigint = Boolean(bigint);
    }

    [kFSStatWatcherStart](filename, persistent = true, interval = 5007) {
      this.#filename = filename;
      this.#interval = interval;
      this.#previous = this.#stat();
      this.#timer = setInterval(() => this.#poll(), interval);
      if (!persistent) this.#timer.unref();
    }

    #stat() {
      const fs = require("node:fs");
      try {
        return fs.statSync(this.#filename, { bigint: this.#bigint });
      } catch (error) {
        if (error.code === "ENOENT") return null;
        throw error;
      }
    }

    #poll() {
      if (this.#closed) return;
      let current;
      try {
        current = this.#stat();
      } catch {
        return;
      }
      if (!this.#changed(this.#previous, current)) return;
      const previous = this.#previous;
      this.#previous = current;
      this.emit("change", current, previous);
    }

    #changed(previous, current) {
      if (previous === null || current === null) return previous !== current;
      if (this.#bigint) {
        return previous.mtimeNs !== current.mtimeNs ||
          previous.ctimeNs !== current.ctimeNs ||
          previous.size !== current.size ||
          previous.ino !== current.ino ||
          previous.mode !== current.mode;
      }
      return previous.mtimeMs !== current.mtimeMs ||
        previous.ctimeMs !== current.ctimeMs ||
        previous.size !== current.size ||
        previous.ino !== current.ino ||
        previous.mode !== current.mode;
    }

    [kFSStatWatcherAddOrCleanRef](operation) {
      if (operation === "add") {
        this.#refs += 1;
        this.#timer?.ref();
      } else if (operation === "clean") {
        this.#refs = Math.max(0, this.#refs - 1);
        if (this.#refs === 0) this.#timer?.unref();
      } else if (operation === "cleanAll") {
        this.#refs = 0;
        this.#timer?.unref();
      }
    }

    stop() {
      if (this.#closed) return;
      this.#closed = true;
      if (this.#timer !== null) {
        clearInterval(this.#timer);
        this.#timer = null;
      }
      this.emit("stop");
    }

    close() {
      this.stop();
    }

    ref() {
      this.#timer?.ref();
    }

    unref() {
      this.#timer?.unref();
    }
  }
  let minimatchModule;
  const createIgnoreMatcher = (ignore) => {
    if (ignore === undefined) return null;
    const entries = Array.isArray(ignore) ? ignore : [ignore];
    const matchers = entries.map((entry) => {
      if (typeof entry === "string") {
        minimatchModule ??= require("node:thinpack/internal/deps/minimatch/index");
        const pattern = new minimatchModule.Minimatch(entry, {
          __proto__: null,
          nocase: false,
          windowsPathsNoEscape: true,
          nonegate: true,
          nocomment: true,
          optimizationLevel: 2,
          platform: process.platform,
          matchBase: true,
        });
        return (filename) => pattern.match(filename);
      }
      if (entry instanceof RegExp) {
        return (filename) => entry.test(filename);
      }
      return entry;
    });
    return (filename) => matchers.some((matcher) => matcher(filename));
  };

  const { AbortError } = require("node:thinpack/internal/errors");
  async function* watch(filename, options = {}) {
    const watcher = new FSWatcher();
    watcher[kFSWatchStart](
      filename,
      options.persistent ?? true,
      options.recursive ?? false,
      options.encoding ?? "utf8",
      options.ignore,
      options.throwIfNoEntry ?? true,
    );
    const signal = options.signal;
    if (signal?.aborted) {
      watcher.close();
      throw new AbortError();
    }
    const onAbort = () => watcher.close();
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
      for await (const event of watcher) {
        if (signal?.aborted) throw new AbortError();
        yield event;
      }
      if (signal?.aborted) throw new AbortError();
    } finally {
      signal?.removeEventListener("abort", onAbort);
      watcher.close();
    }
  }

  module.exports = {
    FSWatcher,
    StatWatcher,
    createIgnoreMatcher,
    watch,
    kFSWatchStart,
    kFSStatWatcherStart,
    kFSStatWatcherAddOrCleanRef,
  };
})();
