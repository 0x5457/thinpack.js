(() => {
  const url = globalThis.__thinpack_url;
  const {
    codes: {
      ERR_INVALID_ARG_TYPE,
      ERR_INVALID_FILE_URL_HOST,
      ERR_INVALID_FILE_URL_PATH,
      ERR_INVALID_URL_SCHEME,
    },
  } = require("node:thinpack/internal/errors");

  const isURL = (self) =>
    Boolean(self?.href && self.protocol && self.auth === undefined && self.path === undefined);

  const getPathFromURLPosix = (fileURL) => {
    if (fileURL.hostname !== "") {
      throw new ERR_INVALID_FILE_URL_HOST(process.platform);
    }
    const pathname = fileURL.pathname;
    for (let index = 0; index < pathname.length; index += 1) {
      if (pathname[index] === "%") {
        const third = pathname.codePointAt(index + 2) | 0x20;
        if (pathname[index + 1] === "2" && third === 102) {
          throw new ERR_INVALID_FILE_URL_PATH(
            "must not include encoded / characters",
            fileURL,
          );
        }
      }
    }
    return pathname.includes("%") ? decodeURIComponent(pathname) : pathname;
  };
  const fileURLToPath = (path, options = { __proto__: null }) => {
    if (options?.windows === true) {
      throw new Error("url.fileURLToPath() does not implement Windows paths");
    }
    let fileURL = path;
    if (typeof fileURL === "string") {
      fileURL = new URL(fileURL);
    } else if (!isURL(fileURL)) {
      throw new ERR_INVALID_ARG_TYPE("path", ["string", "URL"], fileURL);
    }
    if (fileURL.protocol !== "file:") {
      throw new ERR_INVALID_URL_SCHEME("file");
    }
    return getPathFromURLPosix(fileURL);
  };
  const pathToFileURL = (filepath) => {
    const { resolve } = require("node:path");
    let pathname = resolve(String(filepath)).replace(/\\/g, "/");
    if (!pathname.startsWith("/")) pathname = "/" + pathname;
    pathname = pathname
      .replace(/%/g, "%25")
      .replace(/\?/g, "%3F")
      .replace(/#/g, "%23")
      .replace(/\n/g, "%0A")
      .replace(/\r/g, "%0D")
      .replace(/\t/g, "%09");
    const result = new URL("file://");
    result.pathname = pathname;
    return result;
  };
  const toPathIfFileURL = (fileURLOrPath) =>
    isURL(fileURLOrPath) ? fileURLToPath(fileURLOrPath) : fileURLOrPath;

  module.exports = { ...url, fileURLToPath, pathToFileURL, toPathIfFileURL };
})();
