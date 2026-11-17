(() => {
  const { codes } = require("node:thinpack/internal/errors");
  const unsupported = (name) => () => {
    throw new codes.ERR_NOT_IMPLEMENTED(`vm.${name}`);
  };

  const filenameOf = (options) => {
    if (typeof options === "string") return options;
    return options?.filename ?? "evalmachine.<anonymous>";
  };

  const specifierOf = (filename) => {
    if (filename.startsWith("file://")) return filename;
    const path = filename.startsWith("/") ? filename : `/${filename}`;
    return "file://" + path.split("/").map(encodeURIComponent).join("/");
  };

  const evaluate = (code, options) => {
    const filename = filenameOf(options);
    const specifier = specifierOf(filename);
    const [value, error] = Deno.core.evalContext(String(code), specifier);
    if (error !== undefined && error !== null) throw error;
    return value;
  };

  class Script {
    #code;
    #filename;

    constructor(code, options) {
      this.#code = String(code);
      this.#filename = filenameOf(options);
    }

    runInThisContext(options) {
      return evaluate(this.#code, options ?? this.#filename);
    }

    runInContext() {
      throw new codes.ERR_NOT_IMPLEMENTED("vm.Script.runInContext");
    }

    runInNewContext() {
      throw new codes.ERR_NOT_IMPLEMENTED("vm.Script.runInNewContext");
    }

    get [Symbol.toStringTag]() {
      return "Script";
    }

    static createCachedData() {
      return Buffer.alloc(0);
    }
  }

  class Module {
    constructor() {
      throw new codes.ERR_NOT_IMPLEMENTED("vm.Module");
    }
  }

  const compileFunction = (code, params = [], options = {}) => {
    return new Function(...params, String(code));
  };

  const constants = Object.freeze({
    USE_MAIN_CONTEXT_DEFAULT_LOADER: Symbol("vm_dynamic_import_main_context_default"),
    DONT_CONTEXTIFY: Symbol("vm_context_no_contextify"),
  });

  module.exports = {
    constants,
    Script,
    Module,
    createContext: unsupported("createContext"),
    runInContext: unsupported("runInContext"),
    runInNewContext: unsupported("runInNewContext"),
    runInThisContext: (code, options) => evaluate(code, options),
    isContext: () => false,
    compileFunction,
    SourceTextModule: Module,
    SyntheticModule: Module,
  };
})();
