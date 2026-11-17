(() => {
  const { Buffer } = require("node:buffer");
  const { codes } = require("node:thinpack/internal/errors");

  const UV_DIRENT_FILE = 1;
  const UV_DIRENT_DIR = 2;
  const UV_DIRENT_LINK = 3;
  const UV_DIRENT_FIFO = 4;
  const UV_DIRENT_SOCKET = 5;
  const UV_DIRENT_CHAR = 6;
  const UV_DIRENT_BLOCK = 7;

  const toPath = (value) =>
    typeof value === "string"
      ? value
      : Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString("utf8");

  const typeOf = (dirent) => {
    if (dirent.isFile()) return UV_DIRENT_FILE;
    if (dirent.isDirectory()) return UV_DIRENT_DIR;
    if (dirent.isSymbolicLink()) return UV_DIRENT_LINK;
    if (dirent.isFIFO()) return UV_DIRENT_FIFO;
    if (dirent.isSocket()) return UV_DIRENT_SOCKET;
    if (dirent.isCharacterDevice()) return UV_DIRENT_CHAR;
    if (dirent.isBlockDevice()) return UV_DIRENT_BLOCK;
    return 0;
  };

  const readEntries = (path) => {
    const { readdirSync } = require("node:fs");
    return readdirSync(path, { withFileTypes: true }).map((dirent) => [
      dirent.name,
      typeOf(dirent),
    ]);
  };

  class DirHandle {
    #entries;
    #closed = false;

    constructor(path) {
      this.#entries = readEntries(path);
    }

    read(encoding, bufferSize, req) {
      if (this.#closed) throw new codes.ERR_DIR_CLOSED();
      const result = [];
      while (result.length < bufferSize * 2 && this.#entries.length > 0) {
        const [name, type] = this.#entries.shift();
        result.push(encoding === "buffer" ? Buffer.from(name) : name, type);
      }
      const value = result.length === 0 ? null : result;
      if (req === undefined) return value;
      queueMicrotask(() => req.oncomplete(null, value));
      return undefined;
    }

    close(req) {
      if (this.#closed) {
        const error = new codes.ERR_DIR_CLOSED();
        if (req === undefined) throw error;
        queueMicrotask(() => req.oncomplete(error));
        return;
      }
      this.#closed = true;
      this.#entries.length = 0;
      if (req !== undefined) queueMicrotask(() => req.oncomplete(null));
    }
  }

  module.exports = {
    opendirSync(path) {
      return new DirHandle(toPath(path));
    },

    opendir(path, encoding, req) {
      if (req === undefined) return new DirHandle(toPath(path));
      queueMicrotask(() => {
        try {
          req.oncomplete(null, new DirHandle(toPath(path)));
        } catch (error) {
          req.oncomplete(error);
        }
      });
      return undefined;
    },
  };
})();
