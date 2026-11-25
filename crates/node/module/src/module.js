(() => {
  const specifiers = globalThis.__thinpack_builtin_modules ?? [];
  const builtins = specifiers
    .map((name) => name.replace(/^node:/, ""))
    .filter((name) => !name.startsWith("thinpack/"));

  class Module {
    constructor(id = "", parent = null) {
      this.id = id;
      this.filename = id;
      this.path = id;
      this.exports = {};
      this.loaded = false;
      this.parent = parent;
      this.children = [];
    }
  }

  const path = require("node:path");

  Module._nodeModulePaths = function _nodeModulePaths(from) {
    const paths = [];
    let directory = path.resolve(String(from));
    for (;;) {
      if (path.basename(directory) !== "node_modules") {
        paths.push(path.join(directory, "node_modules"));
      }
      const parent = path.dirname(directory);
      if (parent === directory) break;
      directory = parent;
    }
    return paths;
  };

  Module.globalPaths = globalThis.__thinpack_module_global_paths__().slice();
  Module._initPaths = function _initPaths() {
    Module.globalPaths = globalThis.__thinpack_module_init_paths__().slice();
  };

  const isBuiltin = (name) => {
    const bare = String(name).replace(/^node:/, "");
    return bare === "module" || builtins.includes(bare);
  };

  const filenameOf = (value) => {
    const text = String(value);
    if (!text.startsWith("file://")) return text;
    const url = new URL(text);
    return decodeURIComponent(url.pathname);
  };

  const createRequire = (filename) =>
    globalThis.__thinpack_create_require__(filenameOf(filename));
  Module.prototype.require = function require(specifier) {
    return createRequire(this.filename || this.id)(specifier);
  };
  Module._resolveFilename = function _resolveFilename(specifier, parent) {
    const filename = parent && (parent.filename || parent.id);
    if (!filename) throw new Error("module._resolveFilename needs a parent module");
    return createRequire(filename).resolve(specifier);
  };
  Module._cache = globalThis.__thinpack_require_cache ?? Object.create(null);
  Module._extensions = globalThis.__thinpack_module_extensions ?? Object.create(null);
  Module._load = function _load() {
    const error = new Error("module._load is not available");
    error.code = "ERR_NOT_IMPLEMENTED";
    throw error;
  };

  module.exports = Module;
  Module.Module = Module;
  Module.registerHooks = (hooks) => globalThis.__thinpack_esm_hooks__.registerHooks(hooks);
  Module.createRequire = createRequire;
  Module.builtinModules = ["module", ...builtins.filter((name) => name !== "module")];
  Module.isBuiltin = isBuiltin;
  Module.syncBuiltinESMExports = () => {};
  Module.register = (specifier, parentURL, options) =>
    globalThis.__thinpack_esm_hooks__.register(specifier, parentURL, options);
  Module.enableCompileCache = () => ({ enabled: false, path: undefined });
  Module.flushCompileCache = () => {};
  Module.getCompileCacheDir = () => undefined;
})();
