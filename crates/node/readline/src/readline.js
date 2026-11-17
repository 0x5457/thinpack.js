(() => {
  const { EventEmitter } = require("node:events");
  const { codes } = require("node:thinpack/internal/errors");

  const notImplemented = (name) =>
    new codes.ERR_NOT_IMPLEMENTED(`readline.${name}`);

  const invalidArgument = (name, expected) => {
    const error = new TypeError(
      'The "' + name + '" argument must be of type ' + expected,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };

  const invalidArgumentValue = (name, value, reason) => {
    const error = new TypeError(
      "The argument '" + name + "' " + reason + ". Received " +
        JSON.stringify(value),
    );
    error.code = "ERR_INVALID_ARG_VALUE";
    return error;
  };
  const kEscape = "\x1b[";
  const kClearToLineBeginning = kEscape + "1K";
  const kClearToLineEnd = kEscape + "0K";
  const kClearLine = kEscape + "2K";
  const kClearScreenDown = kEscape + "0J";

  const deferred = (callback) => {
    if (typeof callback === "function") process.nextTick(callback, null);
    return true;
  };

  const cursorTo = (stream, x, y, callback) => {
    if (typeof y === "function") {
      callback = y;
      y = undefined;
    }
    if (Number.isNaN(x)) throw invalidArgumentValue("x", x, "is invalid");
    if (Number.isNaN(y)) throw invalidArgumentValue("y", y, "is invalid");
    if (stream === null || stream === undefined ||
        (typeof x !== "number" && typeof y !== "number")) {
      return deferred(callback);
    }
    if (typeof x !== "number") {
      const error = new TypeError(
        "Cannot set cursor row without setting its column",
      );
      error.code = "ERR_INVALID_CURSOR_POS";
      throw error;
    }
    const data = typeof y !== "number"
      ? kEscape + (x + 1) + "G"
      : kEscape + (y + 1) + ";" + (x + 1) + "H";
    return stream.write(data, callback);
  };

  const moveCursor = (stream, dx, dy, callback) => {
    if (stream === null || stream === undefined || !(dx || dy)) {
      return deferred(callback);
    }
    let data = "";
    if (dx < 0) data += kEscape + -dx + "D";
    else if (dx > 0) data += kEscape + dx + "C";
    if (dy < 0) data += kEscape + -dy + "A";
    else if (dy > 0) data += kEscape + dy + "B";
    return stream.write(data, callback);
  };

  const clearLine = (stream, dir, callback) => {
    if (stream === null || stream === undefined) return deferred(callback);
    const type = dir < 0
      ? kClearToLineBeginning
      : dir > 0
        ? kClearToLineEnd
        : kClearLine;
    return stream.write(type, callback);
  };

  const clearScreenDown = (stream, callback) => {
    if (stream === null || stream === undefined) return deferred(callback);
    return stream.write(kClearScreenDown, callback);
  };
  const kCsiPattern = /^\x1b\[([0-9;]*)([A-Za-z~])/;
  const kSs3Pattern = /^\x1bO([A-Za-z])/;
  const kIncompleteEscape = /^\x1b(\[[0-9;]*|O)?$/;

  const kCsiNames = {
    A: "up", B: "down", C: "right", D: "left",
    H: "home", F: "end", G: "column", J: "clearScreen", K: "clearLine",
  };
  const kTildeNames = {
    1: "home", 2: "insert", 3: "delete", 4: "end",
    5: "pageup", 6: "pagedown", 7: "home", 8: "end",
  };
  const kSs3Names = {
    A: "up", B: "down", C: "right", D: "left", H: "home", F: "end",
  };
  const parseEscape = (input, index) => {
    const rest = input.slice(index);
    const csi = kCsiPattern.exec(rest);
    if (csi !== null) {
      const name = csi[2] === "~"
        ? (kTildeNames[csi[1]] ?? "")
        : (kCsiNames[csi[2]] ?? "");
      return { name, length: csi[0].length };
    }
    const ss3 = kSs3Pattern.exec(rest);
    if (ss3 !== null) {
      return { name: kSs3Names[ss3[1]] ?? "", length: ss3[0].length };
    }
    if (kIncompleteEscape.test(rest)) return null;
    return { name: "", length: 1 };
  };

  const kEnter = "enter";
  const kBackspace = "backspace";

  const kControlKeys = {
    "\r": kEnter,
    "\n": kEnter,
    "\x7f": kBackspace,
    "\x08": kBackspace,
    "\x03": "sigint",
    "\x04": "eof",
    "\x01": "home",
    "\x05": "end",
    "\x02": "left",
    "\x06": "right",
    "\x0b": "deleteToEnd",
    "\x15": "deleteToStart",
    "\x17": "deleteWordLeft",
    "\x0c": "clearScreen",
    "\x10": "historyPrev",
    "\x0e": "historyNext",
  };

  const isControl = (character) => {
    const code = character.charCodeAt(0);
    return code < 0x20 || code === 0x7f;
  };

  const wordStartBefore = (line, cursor) => {
    let index = cursor;
    while (index > 0 && /\s/.test(line[index - 1])) index -= 1;
    while (index > 0 && !/\s/.test(line[index - 1])) index -= 1;
    return index;
  };

  class Interface extends EventEmitter {
    #input;
    #output;
    #buffer = "";
    #closed = false;
    #questionWaiters = [];
    #prompt = "> ";
    #oldPrompt = "> ";
    #onData;
    #onEnd;
    #pendingEscape = "";
    #historyIndex = -1;
    #draft = "";

    constructor(input, output = null, options = {}) {
      super();
      if (input?.input !== undefined) {
        const source = input;
        options = {
          terminal: source.terminal,
          history: source.history,
          historySize: source.historySize,
          prompt: source.prompt,
          completer: source.completer,
        };
        input = source.input;
        output = source.output ?? null;
      }
      this.#input = input;
      this.#output = output;
      this.terminal = options.terminal !== undefined
        ? Boolean(options.terminal)
        : Boolean(input.isTTY && output?.isTTY);
      this.line = "";
      this.cursor = 0;
      this.history = Array.isArray(options.history) ? options.history : [];
      this.historySize = options.historySize ?? 30;
      if (options.prompt !== undefined) this.#prompt = String(options.prompt);
      if (this.terminal && typeof options.completer === "function") {
        throw notImplemented("tab completion (completer)");
      }

      if (this.terminal) {
        this.#onData = (chunk) => this.#keys(String(chunk));
        this.#onEnd = () => this.#finish();
        input.setEncoding("utf8");
        if (typeof input.setRawMode === "function") input.setRawMode(true);
      } else {
        this.#onData = (chunk) => this.#consume(chunk);
        this.#onEnd = () => this.#finish();
        input.setEncoding?.("utf8");
      }

      input.on("data", this.#onData);
      input.on("end", this.#onEnd);
      input.on("error", (error) => this.emit("error", error));
      input.resume?.();
    }
    #consume(chunk) {
      this.#buffer += chunk;
      for (;;) {
        const newline = this.#buffer.indexOf("\n");
        if (newline === -1) break;
        let line = this.#buffer.slice(0, newline);
        this.#buffer = this.#buffer.slice(newline + 1);
        if (line.endsWith("\r")) line = line.slice(0, -1);
        this.emit("line", line);
        if (this.#questionWaiters.length > 0) {
          this.#questionWaiters.shift()(line);
        }
      }
    }
    #write(text) {
      if (this.#output !== null) this.#output.write(text);
    }
    #refreshLine() {
      const column = this.#prompt.length + this.cursor + 1;
      this.#write(
        kEscape + "1G" + kClearScreenDown + this.#prompt + this.line +
          kEscape + column + "G",
      );
    }

    #setLine(line) {
      this.line = line;
      this.cursor = line.length;
      this.#refreshLine();
    }

    #keys(chunk) {
      const input = this.#pendingEscape + chunk;
      this.#pendingEscape = "";
      for (let index = 0; index < input.length; index += 1) {
        const character = input[index];
        if (character === "\x1b") {
          const parsed = parseEscape(input, index);
          if (parsed === null) {
            this.#pendingEscape = input.slice(index);
            return;
          }
          index += parsed.length - 1;
          if (parsed.name !== "") this.#key(parsed.name, "");
          continue;
        }
        if (isControl(character)) {
          this.#key(kControlKeys[character] ?? "", character);
        } else {
          this.#key("insert", character);
        }
      }
    }

    #key(name, text) {
      switch (name) {
        case "insert":
          this.line = this.line.slice(0, this.cursor) + text +
            this.line.slice(this.cursor);
          this.cursor += text.length;
          this.#refreshLine();
          break;
        case kEnter:
          this.#accept();
          break;
        case kBackspace:
          if (this.cursor > 0) {
            this.line = this.line.slice(0, this.cursor - 1) +
              this.line.slice(this.cursor);
            this.cursor -= 1;
            this.#refreshLine();
          }
          break;
        case "delete":
          if (this.cursor < this.line.length) {
            this.line = this.line.slice(0, this.cursor) +
              this.line.slice(this.cursor + 1);
            this.#refreshLine();
          }
          break;
        case "left":
          if (this.cursor > 0) {
            this.cursor -= 1;
            this.#refreshLine();
          }
          break;
        case "right":
          if (this.cursor < this.line.length) {
            this.cursor += 1;
            this.#refreshLine();
          }
          break;
        case "home":
          this.cursor = 0;
          this.#refreshLine();
          break;
        case "end":
          this.cursor = this.line.length;
          this.#refreshLine();
          break;
        case "deleteToStart":
          this.line = this.line.slice(this.cursor);
          this.cursor = 0;
          this.#refreshLine();
          break;
        case "deleteToEnd":
          this.line = this.line.slice(0, this.cursor);
          this.#refreshLine();
          break;
        case "deleteWordLeft": {
          const start = wordStartBefore(this.line, this.cursor);
          this.line = this.line.slice(0, start) + this.line.slice(this.cursor);
          this.cursor = start;
          this.#refreshLine();
          break;
        }
        case "historyPrev":
        case "up":
          this.#historyPrev();
          break;
        case "historyNext":
        case "down":
          this.#historyNext();
          break;
        case "sigint":
          if (this.listenerCount("SIGINT") > 0) this.emit("SIGINT");
          else this.close();
          break;
        case "eof":
          if (this.line.length === 0) this.close();
          else this.#key("delete", "");
          break;
        case "clearScreen":
          this.#write(kEscape + "2J" + kEscape + "1;1H");
          this.#refreshLine();
          break;
        default:
          break;
      }
    }

    #accept() {
      this.#write("\r\n");
      const line = this.line;
      this.line = "";
      this.cursor = 0;
      this.#pendingEscape = "";
      this.#historyIndex = -1;
      this.#draft = "";
      if (line.length > 0) {
        this.history.push(line);
        if (this.history.length > this.historySize) {
          this.history.splice(0, this.history.length - this.historySize);
        }
      }
      if (this.#questionWaiters.length > 0) {
        const waiter = this.#questionWaiters.shift();
        this.#prompt = this.#oldPrompt;
        waiter(line);
      } else {
        this.emit("line", line);
      }
    }

    #historyPrev() {
      if (this.history.length === 0) return;
      if (this.#historyIndex === -1) {
        this.#draft = this.line;
        this.#historyIndex = this.history.length - 1;
      } else if (this.#historyIndex > 0) {
        this.#historyIndex -= 1;
      } else {
        return;
      }
      this.#setLine(this.history[this.#historyIndex]);
    }

    #historyNext() {
      if (this.#historyIndex === -1) return;
      if (this.#historyIndex < this.history.length - 1) {
        this.#historyIndex += 1;
        this.#setLine(this.history[this.#historyIndex]);
      } else {
        this.#historyIndex = -1;
        const draft = this.#draft;
        this.#draft = "";
        this.#setLine(draft);
      }
    }
    #finish() {
      if (this.#closed) return;
      if (!this.terminal && this.#buffer.length > 0) {
        const line = this.#buffer.endsWith("\r")
          ? this.#buffer.slice(0, -1)
          : this.#buffer;
        this.#buffer = "";
        this.emit("line", line);
        if (this.#questionWaiters.length > 0) {
          this.#questionWaiters.shift()(line);
        }
      }
      this.#closed = true;
      this.#input.pause?.();
      this.#input.removeListener("data", this.#onData);
      this.#input.removeListener("end", this.#onEnd);
      if (this.terminal) {
        if (typeof this.#input.setRawMode === "function") {
          this.#input.setRawMode(false);
        }
      }
      this.emit("close");
    }

    pause() {
      this.#input.pause?.();
      return this;
    }

    resume() {
      this.#input.resume?.();
      return this;
    }

    close() {
      this.#finish();
      return this;
    }

    question(query, callback) {
      const text = String(query);
      if (this.terminal) {
        this.#oldPrompt = this.#prompt;
        this.#prompt = text;
        this.line = "";
        this.cursor = 0;
        this.#refreshLine();
      } else {
        this.#output?.write(text);
      }
      const answer = new Promise((resolve) => this.#questionWaiters.push(
        resolve,
      ));
      if (typeof callback === "function") {
        answer.then(callback);
        return undefined;
      }
      return answer;
    }

    prompt(preserveCursor) {
      if (this.terminal) {
        if (!preserveCursor) this.cursor = 0;
        this.#refreshLine();
        return;
      }
      if (this.#output !== null && !this.#closed) this.#output.write(this.#prompt);
    }

    setPrompt(prompt) {
      this.#prompt = String(prompt);
    }

    getPrompt() {
      return this.#prompt;
    }

    write(data) {
      if (typeof data !== "string") {
        throw invalidArgument("data", "string");
      }
      this.#output?.write(data);
    }

    [Symbol.asyncIterator]() {
      const queue = [];
      const waiters = [];
      let done = false;
      const push = (line) => {
        const waiter = waiters.shift();
        if (waiter) waiter({ value: line, done: false });
        else queue.push(line);
      };
      this.on("line", push);
      this.on("close", () => {
        done = true;
        for (const waiter of waiters) waiter({ value: undefined, done: true });
      });
      return {
        next: () => {
          if (queue.length > 0) {
            return Promise.resolve({ value: queue.shift(), done: false });
          }
          if (done) return Promise.resolve({ value: undefined, done: true });
          return new Promise((resolve) => waiters.push(resolve));
        },
        return: () => Promise.resolve({ value: undefined, done: true }),
      };
    }
  }
  const createInterface = (input, output, completer, terminal) =>
    new Interface(input, output ?? null, { completer, terminal });

  const unsupported = (name) => () => {
    throw notImplemented(name);
  };

  module.exports = {
    Interface,
    createInterface,
    clearLine,
    clearScreenDown,
    cursorTo,
    moveCursor,
    emitKeypressEvents: unsupported("emitKeypressEvents"),
    promises: {
      createInterface,
      Interface,
    },
  };
})();
