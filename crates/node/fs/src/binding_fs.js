(() => {
  const { ops } = Deno.core;
  const { Buffer } = require("node:buffer");
  const { UVException, codes } = require("node:thinpack/internal/errors");

  const kSep = "\0";
  const ENOENT = -2;
  const ENOTDIR = -20;

  const notImplemented = (api) => new codes.ERR_NOT_IMPLEMENTED(api);
  const kUsePromises = Symbol("kUsePromises");
  class FsError extends Error {
    constructor(message) {
      super(message);
      this.name = "FsError";
    }
  }
  Deno.core.registerErrorClass("FsError", FsError);
  const promise = (run) => Promise.resolve().then(run);

  const toPath = (value) =>
    typeof value === "string"
      ? value
      : Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString("utf8");

  const bytes = (buffer) =>
    new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);

  const position = (value) => (value === null || value === undefined ? -1 : value);

  const uvError = (error, syscall, path) =>
    error?.name === "FsError"
      ? new UVException({ errno: Number(error.message), syscall, path })
      : error;

  const call = (syscall, path, dest, run) => {
    try {
      return run();
    } catch (error) {
      if (error.name !== "FsError") throw error;
      throw new UVException({ errno: Number(error.message), syscall, path, dest });
    }
  };

  const encodePath = (value, encoding) =>
    encoding === "buffer" ? Buffer.from(value) : value;

  const decodeStats = (payload, bigint) => {
    const fields = payload.split(kSep);
    const stats = bigint
      ? new BigInt64Array(fields.length)
      : new Float64Array(fields.length);
    for (let index = 0; index < fields.length; index += 1) {
      stats[index] = bigint ? BigInt(fields[index]) : Number(fields[index]);
    }
    return stats;
  };

  const isNoEntry = (error) => error.errno === ENOENT || error.errno === ENOTDIR;

  const readdirResult = (path, encoding, withFileTypes) => {
    const result = call("scandir", path, undefined, () =>
      ops.op_fs_readdir(toPath(path), Boolean(withFileTypes)));
    const fields = result === "" ? [] : result.split(kSep);
    if (!withFileTypes) return fields.map((name) => encodePath(name, encoding));
    const names = [];
    const types = [];
    for (let index = 0; index < fields.length; index += 2) {
      names.push(encodePath(fields[index], encoding));
      types.push(Number(fields[index + 1]));
    }
    return [names, types];
  };

  const readAll = (fd) => {
    const chunks = [];
    for (;;) {
      const buffer = Buffer.allocUnsafe(65536);
      const read = ops.op_fs_read(fd, buffer, -1);
      if (read === 0) break;
      chunks.push(read === buffer.length ? buffer : buffer.subarray(0, read));
    }
    return chunks.length === 0 ? Buffer.alloc(0) : Buffer.concat(chunks);
  };

  const writeAll = (fd, data) => {
    const buffer = Buffer.isBuffer(data)
      ? data
      : Buffer.from(data.buffer, data.byteOffset, data.byteLength);
    let offset = 0;
    while (offset < buffer.length) {
      offset += ops.op_fs_write(fd, buffer.subarray(offset), -1);
    }
  };
  class ReadFileJob {
    constructor(path, flags) {
      this.flags = flags;
    }

    run(path) {
      const request = typeof path === "number"
        ? ops.op_fs_read_fd_async(path)
        : ops.op_fs_read_file_async(toPath(path), this.flags);
      request.then(
        (buffer) => this.ondone(
          null,
          Buffer.from(buffer.buffer, buffer.byteOffset, buffer.byteLength),
          -1,
        ),
        (error) => this.ondone(uvError(error, "read", path)),
      );
      return undefined;
    }
  }

  class WriteFileJob {
    constructor(path, flags, mode, data) {
      this.flags = flags;
      this.mode = mode;
      this.data = data;
    }

    run(path) {
      const request = typeof path === "number"
        ? null
        : ops.op_fs_write_file_async(toPath(path), this.flags, this.mode, this.data);
      if (request === null) {
        try {
          writeAll(path, this.data);
          this.ondone(null);
        } catch (error) {
          this.ondone(uvError(error, "write", path));
        }
        return undefined;
      }
      request.then(
        () => this.ondone(null),
        (error) => this.ondone(uvError(error, "write", path)),
      );
      return undefined;
    }
  }
  class FileHandle {
    constructor(fd) {
      this.fd = fd;
    }

    getAsyncId() {
      return -1;
    }

    close() {
      return promise(() => ops.op_fs_close(this.fd));
    }

    closeSync() {
      ops.op_fs_close(this.fd);
    }

    ref() {}

    unref() {}
  }

  class FSReqCallback {}
  const mode = (req, run) => {
    if (req === kUsePromises) return promise(run);
    if (req !== undefined) {
      queueMicrotask(() => {
        try {
          req.oncomplete(null, run());
        } catch (error) {
          req.oncomplete(error);
        }
      });
      return undefined;
    }
    return run();
  };

  module.exports = {
    kUsePromises,
    FileHandle,
    ReadFileJob,
    WriteFileJob,
    FSReqCallback,

    access(path, mode_, req) {
      return mode(req, () =>
        call("access", path, undefined, () => ops.op_fs_access(toPath(path), mode_)));
    },

    existsSync(path) {
      return ops.op_fs_exists(toPath(path));
    },

    open(path, flags, mode_, req) {
      return mode(req, () =>
        call("open", path, undefined, () => ops.op_fs_open(toPath(path), flags, mode_)));
    },

    openFileHandle(path, flags, mode_, req) {
      if (req !== kUsePromises) {
        throw notImplemented("fs.open (asynchronous callback)");
      }
      return promise(() =>
        new FileHandle(
          call("open", path, undefined, () => ops.op_fs_open(toPath(path), flags, mode_)),
        ));
    },

    close(fd, req) {
      return mode(req, () => call("close", undefined, undefined, () => ops.op_fs_close(fd)));
    },

    read(fd, buffer, offset, length, position_, req) {
      return mode(req, () =>
        call("read", undefined, undefined, () =>
          ops.op_fs_read(fd, bytes(buffer).subarray(offset, offset + length), position(position_))));
    },

    readFileUtf8(pathOrFd, flags) {
      if (typeof pathOrFd === "number") {
        return call("read", undefined, undefined, () => ops.op_fs_read_all_utf8(pathOrFd));
      }
      const fd = call("open", pathOrFd, undefined, () =>
        ops.op_fs_open(toPath(pathOrFd), flags, 0o666));
      try {
        return call("read", undefined, undefined, () => ops.op_fs_read_all_utf8(fd));
      } finally {
        try {
          ops.op_fs_close(fd);
        } catch {
        }
      }
    },

    write(fd, buffer, offset, length, position_, req) {
      return mode(req, () =>
        call("write", undefined, undefined, () =>
          ops.op_fs_write(fd, bytes(buffer).subarray(offset, offset + length), position(position_))));
    },

    writeBuffer(fd, buffer, offset, length, position_, req) {
      return module.exports.write(fd, buffer, offset, length, position_, req);
    },

    writeString(fd, value, position_, encoding, req) {
      return mode(req, () => {
        const data = Buffer.from(value, Buffer.isEncoding(encoding) ? encoding : "utf8");
        return call("write", undefined, undefined, () =>
          ops.op_fs_write(fd, data, position(position_)));
      });
    },

    writeFileUtf8(pathOrFd, data, flags, mode_) {
      const write = (fd) =>
        call("write", undefined, undefined, () => ops.op_fs_write_all_utf8(fd, data));
      if (typeof pathOrFd === "number") {
        return write(pathOrFd);
      }
      const fd = call("open", pathOrFd, undefined, () =>
        ops.op_fs_open(toPath(pathOrFd), flags, mode_));
      try {
        write(fd);
      } finally {
        try {
          ops.op_fs_close(fd);
        } catch {
        }
      }
    },

    fsync(fd, req) {
      return mode(req, () => call("fsync", undefined, undefined, () => ops.op_fs_fsync(fd)));
    },

    fdatasync(fd, req) {
      return mode(req, () => call("fdatasync", undefined, undefined, () => ops.op_fs_fdatasync(fd)));
    },

    ftruncate(fd, length, req) {
      return mode(req, () =>
        call("ftruncate", undefined, undefined, () => ops.op_fs_ftruncate(fd, length)));
    },

    fstat(fd, bigint, req) {
      return mode(req, () =>
        decodeStats(call("fstat", undefined, undefined, () => ops.op_fs_fstat(fd)), bigint));
    },

    stat(path, bigint, req, throwIfNoEntry) {
      return mode(req, () => {
        try {
          return decodeStats(call("stat", path, undefined, () => ops.op_fs_stat(toPath(path))), bigint);
        } catch (error) {
          if (throwIfNoEntry === false && isNoEntry(error)) return undefined;
          throw error;
        }
      });
    },

    lstat(path, bigint, req, throwIfNoEntry) {
      return mode(req, () => {
        try {
          return decodeStats(call("lstat", path, undefined, () => ops.op_fs_lstat(toPath(path))), bigint);
        } catch (error) {
          if (throwIfNoEntry === false && isNoEntry(error)) return undefined;
          throw error;
        }
      });
    },

    readdir(path, encoding, withFileTypes, req) {
      return mode(req, () => readdirResult(path, encoding, withFileTypes));
    },

    mkdir(path, mode_, recursive, req) {
      return mode(req, () => {
        const created = call("mkdir", path, undefined, () =>
          ops.op_fs_mkdir(toPath(path), mode_, Boolean(recursive)));
        return created === "" ? undefined : created;
      });
    },

    rmdir(path, req) {
      return mode(req, () => call("rmdir", path, undefined, () => ops.op_fs_rmdir(toPath(path))));
    },

    rmSync(path, maxRetries, recursive, retryDelay) {
      call("rm", path, undefined, () => ops.op_fs_rm(toPath(path), Boolean(recursive)));
    },

    unlink(path, req) {
      return mode(req, () => call("unlink", path, undefined, () => ops.op_fs_unlink(toPath(path))));
    },

    rename(oldPath, newPath, req) {
      return mode(req, () =>
        call("rename", oldPath, newPath, () =>
          ops.op_fs_rename(toPath(oldPath), toPath(newPath))));
    },

    copyFile(source, destination, mode_, req) {
      return mode(req, () =>
        call("copyfile", source, destination, () =>
          ops.op_fs_copy_file(toPath(source), toPath(destination), mode_ ?? 0)));
    },

    chmod(path, mode_, req) {
      return mode(req, () =>
        call("chmod", path, undefined, () => ops.op_fs_chmod(toPath(path), mode_)));
    },

    utimes(path, atime, mtime, req) {
      return mode(req, () =>
        call("utime", path, undefined, () => ops.op_fs_utimes(toPath(path), atime, mtime)));
    },

    readlink(path, encoding, req) {
      return mode(req, () =>
        encodePath(
          call("readlink", path, undefined, () => ops.op_fs_readlink(toPath(path))),
          encoding,
        ));
    },

    symlink(target, path, flags, req) {
      return mode(req, () =>
        call("symlink", target, path, () =>
          ops.op_fs_symlink(toPath(target), toPath(path))));
    },

    mkdtemp(prefix, encoding, req) {
      return mode(req, () =>
        encodePath(
          call("mkdtemp", prefix, undefined, () => ops.op_fs_mkdtemp(toPath(prefix))),
          encoding,
        ));
    },

    realpath(path, encoding, req) {
      return mode(req, () =>
        encodePath(
          call("realpath", path, undefined, () => ops.op_fs_realpath(toPath(path))),
          encoding,
        ));
    },
    cpSyncCheckPaths(src, dest, dereference, recursive) {
      const fs = require("node:fs");
      const path = require("node:path");
      const srcStat = dereference ? fs.statSync(src) : fs.lstatSync(src);
      let destStat;
      try {
        destStat = dereference ? fs.statSync(dest) : fs.lstatSync(dest);
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }

      const srcPath = path.resolve(src);
      const destPath = path.resolve(dest);
      const srcIsDir = srcStat.isDirectory() || (dereference && srcStat.isSymbolicLink());
      const sameFile = (left, right) =>
        left !== undefined && right.dev === left.dev && right.ino === left.ino;

      if (destStat !== undefined) {
        if (sameFile(srcStat, destStat)) {
          throw new codes.ERR_FS_CP_EINVAL(`src and dest cannot be the same ${destPath}`);
        }
        if (srcIsDir && !destStat.isDirectory()) {
          throw new codes.ERR_FS_CP_DIR_TO_NON_DIR(
            `Cannot overwrite non-directory ${destPath} with directory ${srcPath}`);
        }
        if (!srcIsDir && destStat.isDirectory()) {
          throw new codes.ERR_FS_CP_NON_DIR_TO_DIR(
            `Cannot overwrite directory ${destPath} with non-directory ${srcPath}`);
        }
      }

      if (srcIsDir && destPath.startsWith(srcPath + path.sep)) {
        throw new codes.ERR_FS_CP_EINVAL(
          `Cannot copy ${srcPath}${path.sep} to a subdirectory of self ${destPath}`);
      }

      if (srcIsDir && !recursive) {
        throw new codes.ERR_FS_EISDIR(
          `Recursive option not enabled, cannot copy a directory: ${srcPath}${path.sep}`);
      }

      if (srcStat.isSocket()) {
        throw new codes.ERR_FS_CP_SOCKET(`Cannot copy a socket file: ${destPath}`);
      }
      if (srcStat.isFIFO()) {
        throw new codes.ERR_FS_CP_FIFO_PIPE(`Cannot copy a FIFO pipe: ${destPath}`);
      }
      if (!srcStat.isFile() && !srcStat.isDirectory() && !srcStat.isSymbolicLink() &&
          !srcStat.isCharacterDevice() && !srcStat.isBlockDevice()) {
        throw new codes.ERR_FS_CP_UNKNOWN(`Cannot copy an unknown file type: ${destPath}`);
      }

      const parent = path.dirname(destPath);
      if (!fs.existsSync(parent)) fs.mkdirSync(parent, { recursive: true });
    },

    cpSyncCopyDir(src, dest, force, dereference, errorOnExist, verbatimSymlinks,
                  preserveTimestamps) {
      require("node:fs").cpSync(src, dest, {
        recursive: true,
        force,
        dereference,
        errorOnExist,
        verbatimSymlinks,
        preserveTimestamps,
        filter: () => true,
      });
    },

    cpSyncOverrideFile(src, dest, mode, preserveTimestamps) {
      const fs = require("node:fs");
      fs.copyFileSync(src, dest);
      if (preserveTimestamps) {
        const stat = fs.statSync(src);
        fs.utimesSync(dest, stat.atime, stat.mtime);
      }
      if (mode !== 0) fs.chmodSync(dest, mode);
    },

    internalModuleStat(path) {
      const { lstatSync } = require("node:fs");
      try {
        return lstatSync(path).isDirectory() ? 1 : 0;
      } catch (error) {
        return -error.errno;
      }
    },

    CpDirJob: class CpDirJob {
      constructor(src, dest, force, dereference, errorOnExist, verbatimSymlinks,
                  preserveTimestamps, mode) {
        this.options = {
          src,
          dest,
          recursive: true,
          force,
          dereference,
          errorOnExist,
          verbatimSymlinks,
          preserveTimestamps,
          mode,
        };
      }

      run() {
        queueMicrotask(() => {
          try {
            require("node:fs").cpSync(this.options.src, this.options.dest, this.options);
            this.ondone(null);
          } catch (error) {
            this.ondone(error);
          }
        });
      }
    },
    handleToFd() {
      throw notImplemented("fs.createReadStream with a Windows handle");
    },

    opendir() {
      throw notImplemented("fs.opendir");
    },

    opendirSync() {
      throw notImplemented("fs.opendirSync");
    },

    statfs(path, bigint, req) {
      return mode(req, () => {
        const fields = call("statfs", path, undefined, () => ops.op_fs_statfs(toPath(path)))
          .split(kSep);
        return fields.map((value) => (bigint ? BigInt(value) : Number(value)));
      });
    },

    readdirRecursive(path, encoding, withFileTypes, req) {
      return mode(req, () => {
        const names = [];
        const types = [];
        const counts = [];
        const dirs = [];
        const queue = [[toPath(path), ""]];
        for (let index = 0; index < queue.length; index += 1) {
          const [directory, prefix] = queue[index];
          const [entries, entryTypes] = readdirResult(directory, encoding, true);
          let count = 0;
          for (let entry = 0; entry < entries.length; entry += 1) {
            const name = entries[entry];
            const relative = prefix === ""
              ? name
              : typeof name === "string"
                ? prefix + "/" + name
                : Buffer.concat([Buffer.from(prefix + "/"), name]);
            names.push(withFileTypes ? name : relative);
            types.push(entryTypes[entry]);
            count += 1;
            if (entryTypes[entry] === 2) {
              const child = typeof name === "string"
                ? directory + "/" + name
                : Buffer.concat([Buffer.from(directory + "/"), name]);
              queue.push([child, relative]);
            }
          }
          counts.push(count);
          dirs.push(prefix);
        }

        dirs[0] = "";
        return withFileTypes ? { 0: names, 1: types, 2: counts, 3: dirs } : names;
      });
    },

    readBuffers(fd, buffers, position, req) {
      return mode(req, () => {
        let total = 0;
        for (const buffer of buffers) {
          const view = bytes(buffer);
          const offset = position === null || position === undefined ? -1 : position + total;
          const read = call("read", undefined, undefined, () => ops.op_fs_read(fd, view, offset));
          total += read;
          if (read < view.length) break;
        }
        return total;
      });
    },

    writeBuffers(fd, buffers, position, req) {
      return mode(req, () => {
        let total = 0;
        for (const buffer of buffers) {
          const view = bytes(buffer);
          const offset = position === null || position === undefined ? -1 : position + total;
          total += call("write", undefined, undefined, () => ops.op_fs_write(fd, view, offset));
        }
        return total;
      });
    },

    link(existingPath, newPath, req) {
      return mode(req, () =>
        call("link", existingPath, undefined, () =>
          ops.op_fs_link(toPath(existingPath), toPath(newPath))));
    },

    chown(path, uid, gid, req) {
      return mode(req, () =>
        call("chown", path, undefined, () => ops.op_fs_chown(toPath(path), uid, gid)));
    },

    lchown(path, uid, gid, req) {
      return mode(req, () =>
        call("lchown", path, undefined, () => ops.op_fs_lchown(toPath(path), uid, gid)));
    },

    fchmod(fd, mode_, req) {
      return mode(req, () =>
        call("fchmod", undefined, undefined, () => ops.op_fs_fchmod(fd, mode_)));
    },

    fchown(fd, uid, gid, req) {
      return mode(req, () =>
        call("fchown", undefined, undefined, () => ops.op_fs_fchown(fd, uid, gid)));
    },

    futimes(fd, atime, mtime, req) {
      return mode(req, () =>
        call("futimes", undefined, undefined, () => ops.op_fs_futimes(fd, atime, mtime)));
    },

    lutimes(path, atime, mtime, req) {
      return mode(req, () =>
        call("lutimes", path, undefined, () => ops.op_fs_lutimes(toPath(path), atime, mtime)));
    },
  };
})();
