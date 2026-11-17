(() => {
  const { Buffer } = require("node:buffer");
  const http = require("node:http");
  const https = require("node:https");
  const { createHash, randomBytes } = require("node:crypto");

  const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
  const CONNECTING = 0;
  const OPEN = 1;
  const CLOSING = 2;
  const CLOSED = 3;

  const acceptFor = (key) => createHash("sha1").update(key + GUID).digest("base64");

  const mask = (payload, key) => {
    const output = Buffer.allocUnsafe(payload.length);
    for (let index = 0; index < payload.length; index += 1) {
      output[index] = payload[index] ^ key[index & 3];
    }
    return output;
  };

  const encodeFrame = (opcode, payload, fin = true) => {
    const length = payload.length;
    let header;
    if (length < 126) {
      header = Buffer.from([(fin ? 0x80 : 0) | opcode, 0x80 | length]);
    } else if (length < 65536) {
      header = Buffer.alloc(4);
      header[0] = (fin ? 0x80 : 0) | opcode;
      header[1] = 0x80 | 126;
      header.writeUInt16BE(length, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = (fin ? 0x80 : 0) | opcode;
      header[1] = 0x80 | 127;
      header.writeBigUInt64BE(BigInt(length), 2);
    }
    const key = randomBytes(4);
    return Buffer.concat([header, key, mask(payload, key)]);
  };

  class WebSocket extends EventTarget {
    static CONNECTING = CONNECTING;
    static OPEN = OPEN;
    static CLOSING = CLOSING;
    static CLOSED = CLOSED;

    #socket = null;
    #request = null;
    #buffer = Buffer.alloc(0);
    #fragments = [];
    #fragmentOpcode = 0;
    #closeSent = false;
    #closed = false;

    constructor(url, protocols) {
      super();
      let parsed;
      try {
        parsed = new URL(String(url));
      } catch {
        throw new DOMException("Invalid URL", "SyntaxError");
      }
      if (parsed.protocol !== "ws:" && parsed.protocol !== "wss:") {
        throw new DOMException("URL scheme must be ws or wss", "SyntaxError");
      }
      const requested = typeof protocols === "string"
        ? protocols.split(",").map((value) => value.trim()).filter(Boolean)
        : Array.isArray(protocols) ? protocols.map(String) : [];
      this.url = parsed.href;
      this.readyState = CONNECTING;
      this.binaryType = "blob";
      this.protocol = "";
      this.extensions = "";
      this.bufferedAmount = 0;
      this.onopen = null;
      this.onmessage = null;
      this.onclose = null;
      this.onerror = null;

      const secure = parsed.protocol === "wss:";
      const key = randomBytes(16).toString("base64");
      const headers = {
        connection: "Upgrade",
        upgrade: "websocket",
        "sec-websocket-version": "13",
        "sec-websocket-key": key,
      };
      if (requested.length > 0) headers["sec-websocket-protocol"] = requested.join(", ");

      const options = {
        hostname: parsed.hostname,
        port: parsed.port === "" ? (secure ? 443 : 80) : Number(parsed.port),
        path: parsed.pathname + parsed.search,
        method: "GET",
        headers,
        agent: false,
      };
      const request = (secure ? https : http).request(options);
      this.#request = request;
      request.on("upgrade", (response, socket, head) => {
        if (String(response.headers.upgrade ?? "").toLowerCase() !== "websocket") {
          this.#fail("Invalid Upgrade response");
          socket.destroy();
          return;
        }
        if (response.headers["sec-websocket-accept"] !== acceptFor(key)) {
          this.#fail("Invalid Sec-WebSocket-Accept header");
          socket.destroy();
          return;
        }
        this.#socket = socket;
        this.protocol = response.headers["sec-websocket-protocol"] ?? "";
        this.readyState = OPEN;
        socket.on("data", (chunk) => this.#feed(chunk));
        socket.on("close", () => this.#closedByServer());
        socket.on("error", (error) => this.#fail(error.message));
        this.#emit("open", new Event("open"));
        if (head.length > 0) this.#feed(head);
      });
      request.on("response", (response) => {
        this.#fail(`Unexpected server response: ${response.statusCode}`);
        response.resume?.();
      });
      request.on("error", (error) => this.#fail(error.message));
      request.end();
    }

    #emit(type, event) {
      event.target = this;
      this.dispatchEvent(event);
      const handler = this["on" + type];
      if (typeof handler === "function") handler.call(this, event);
    }

    #fail(message) {
      if (this.readyState === CLOSED) return;
      this.readyState = CLOSED;
      const event = new Event("error");
      event.message = String(message);
      event.error = new Error(String(message));
      this.#emit("error", event);
      this.#finishClose(1006, "", false);
    }

    #finishClose(code, reason, clean) {
      this.readyState = CLOSED;
      if (this.#closed) return;
      this.#closed = true;
      const event = new Event("close");
      event.code = code;
      event.reason = reason;
      event.wasClean = clean;
      this.#emit("close", event);
    }

    #closedByServer() {
      this.#finishClose(1006, "", false);
    }

    send(data) {
      if (this.readyState !== OPEN) {
        throw new DOMException("WebSocket is not open", "InvalidStateError");
      }
      if (typeof data === "string") {
        this.#sendFrame(1, Buffer.from(data, "utf8"));
        return;
      }
      if (data instanceof Blob) {
        data.arrayBuffer().then((buffer) => {
          if (this.readyState === OPEN) this.#sendFrame(2, Buffer.from(buffer));
        });
        return;
      }
      if (data instanceof ArrayBuffer) {
        this.#sendFrame(2, Buffer.from(data));
        return;
      }
      if (ArrayBuffer.isView(data)) {
        this.#sendFrame(2, Buffer.from(data.buffer, data.byteOffset, data.byteLength));
        return;
      }
      throw new TypeError("WebSocket.send expects a string, Blob, ArrayBuffer or view");
    }

    #sendFrame(opcode, payload) {
      if (this.#socket === null || this.#socket.destroyed) return;
      this.#socket.write(encodeFrame(opcode, payload));
    }

    close(code = 1000, reason = "") {
      if (this.readyState === CLOSING || this.readyState === CLOSED) return;
      if (code !== 1000 && (code < 3000 || code > 4999)) {
        throw new DOMException("Invalid close code", "InvalidAccessError");
      }
      this.readyState = CLOSING;
      const payload = Buffer.alloc(2 + Buffer.byteLength(reason));
      payload.writeUInt16BE(code, 0);
      payload.write(reason, 2, "utf8");
      this.#closeSent = true;
      this.#sendFrame(8, payload);
    }

    #feed(chunk) {
      this.#buffer = Buffer.concat([this.#buffer, chunk]);
      for (;;) {
        if (this.#buffer.length < 2) return;
        const first = this.#buffer[0];
        const second = this.#buffer[1];
        const fin = (first & 0x80) !== 0;
        const opcode = first & 0x0f;
        const masked = (second & 0x80) !== 0;
        let length = second & 0x7f;
        let offset = 2;
        if (length === 126) {
          if (this.#buffer.length < 4) return;
          length = this.#buffer.readUInt16BE(2);
          offset = 4;
        } else if (length === 127) {
          if (this.#buffer.length < 10) return;
          length = Number(this.#buffer.readBigUInt64BE(2));
          offset = 10;
        }
        let key = null;
        if (masked) {
          if (this.#buffer.length < offset + 4) return;
          key = this.#buffer.subarray(offset, offset + 4);
          offset += 4;
        }
        if (this.#buffer.length < offset + length) return;
        let payload = this.#buffer.subarray(offset, offset + length);
        this.#buffer = this.#buffer.subarray(offset + length);
        if (key !== null) payload = mask(payload, key);

        if (opcode === 8) {
          const code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
          const reason = payload.length > 2 ? payload.subarray(2).toString("utf8") : "";
          if (!this.#closeSent) {
            this.#closeSent = true;
            this.#sendFrame(8, payload.subarray(0, Math.min(2, payload.length)));
          }
          this.#socket?.end();
          this.#finishClose(code, reason, true);
          return;
        }
        if (opcode === 9) {
          this.#sendFrame(10, payload);
          continue;
        }
        if (opcode === 10) continue;
        if (opcode === 0) {
          this.#fragments.push(payload);
          if (!fin) continue;
          const combined = Buffer.concat(this.#fragments);
          const messageOpcode = this.#fragmentOpcode;
          this.#fragments = [];
          this.#fragmentOpcode = 0;
          this.#deliver(messageOpcode, combined);
          continue;
        }
        if (opcode === 1 || opcode === 2) {
          if (!fin) {
            this.#fragments = [payload];
            this.#fragmentOpcode = opcode;
            continue;
          }
          this.#deliver(opcode, payload);
          continue;
        }
      }
    }

    #deliver(opcode, payload) {
      const event = new Event("message");
      if (opcode === 1) {
        event.data = payload.toString("utf8");
      } else if (this.binaryType === "arraybuffer") {
        event.data = payload.buffer.slice(payload.byteOffset, payload.byteOffset + payload.byteLength);
      } else {
        event.data = new Blob([payload]);
      }
      this.#emit("message", event);
    }
  }

  module.exports = { WebSocket, CONNECTING, OPEN, CLOSING, CLOSED };
})();
