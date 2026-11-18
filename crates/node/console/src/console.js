(() => {
  const write = (text, isError) => Deno.core.ops.op_thinpack_io_write(text, isError);

  const format = (...values) => {
    let utilFormat = null;
    try {
      utilFormat = globalThis.__thinpack_require__("node:util").format;
    } catch {
      utilFormat = null;
    }
    if (utilFormat !== null) return utilFormat(...values);
    return values.map((value) => {
      if (typeof value === "string") return value;
      if (typeof value === "bigint") return `${value}n`;
      try {
        return JSON.stringify(value) ?? String(value);
      } catch {
        return String(value);
      }
    }).join(" ");
  };

  class Console {
    #stdout;
    #stderr;
    #counts = new Map();
    #times = new Map();
    #groups = 0;

    constructor(options = {}) {
      this.#stdout = options?.stdout ?? null;
      this.#stderr = options?.stderr ?? null;
      // Node binds console methods to the instance, so destructuring and
      // `promise.then(console.log)` keep working.
      for (const name of Object.getOwnPropertyNames(Console.prototype)) {
        if (name === "constructor") continue;
        this[name] = this[name].bind(this);
      }
    }

    #write(isError, values) {
      const text = format(...values) + "\n";
      const indent = "  ".repeat(this.#groups);
      const body = indent === "" ? text : text.split("\n").map((line, index, lines) =>
        line === "" && index === lines.length - 1 ? line : indent + line).join("\n");
      const stream = isError ? this.#stderr : this.#stdout;
      if (stream !== null && typeof stream.write === "function") stream.write(body);
      else write(body, isError);
    }

    log(...values) { this.#write(false, values); }
    info(...values) { this.#write(false, values); }
    debug(...values) { this.#write(false, values); }
    warn(...values) { this.#write(true, values); }
    error(...values) { this.#write(true, values); }
    dir(value) { this.#write(false, [value]); }
    trace(...values) {
      // Node renders a synthetic error's default stack, which formats as
      // "Trace: <formatted args>" followed by the captured frames.
      const error = { name: "Trace", message: format(...values) };
      Error.captureStackTrace(error, this.trace);
      this.error(error.stack);
    }

    assert(condition, ...values) {
      if (!condition) this.#write(true, ["Assertion failed:", ...values]);
    }

    count(label = "default") {
      const count = (this.#counts.get(label) ?? 0) + 1;
      this.#counts.set(label, count);
      this.log(`${label}: ${count}`);
    }

    countReset(label = "default") {
      if (!this.#counts.has(label)) {
        process.emitWarning(`Count for '${label}' does not exist`);
        return;
      }
      this.#counts.delete(label);
    }

    time(label = "default") { this.#times.set(label, Date.now()); }

    timeEnd(label = "default") {
      const start = this.#times.get(label);
      if (start === undefined) {
        process.emitWarning(`No such label '${label}' for console.timeEnd()`);
        return;
      }
      this.#times.delete(label);
      this.log(`${label}: ${Date.now() - start}ms`);
    }

    timeLog(label = "default", ...values) {
      const start = this.#times.get(label);
      if (start === undefined) {
        process.emitWarning(`No such label '${label}' for console.timeLog()`);
        return;
      }
      this.log(`${label}: ${Date.now() - start}ms`, ...values);
    }

    group(...values) {
      if (values.length > 0) this.log(...values);
      this.#groups += 1;
    }

    groupCollapsed(...values) { this.group(...values); }
    groupEnd() { this.#groups = Math.max(0, this.#groups - 1); }

    // Deviation from Node: table() prints plain formatted output instead of
    // Node's ASCII table, whose renderer (internal/cli_table plus
    // getStringWidth) is not vendored here.
    table(data) { this.#write(false, [data]); }
    clear() { this.#write(false, ["\x1b[2J\x1b[H"]); }
  }

  const console = new Console();
  console.Console = Console;
  globalThis.console = console;
})();
