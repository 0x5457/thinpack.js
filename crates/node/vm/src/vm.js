(() => {
  const { ops } = Deno.core;
  const { codes } = require("node:thinpack/internal/errors");

  const contexts = new WeakMap();
  const DEFAULT_FILENAME = "evalmachine.<anonymous>";

  const optionsOf = (options) => {
    if (typeof options === "string") return { filename: options };
    return options ?? {};
  };

  const filenameOf = (options) => optionsOf(options).filename ?? DEFAULT_FILENAME;

  const specifierOf = (filename) => {
    if (filename.startsWith("file://")) return filename;
    const path = filename.startsWith("/") ? filename : `/${filename}`;
    return "file://" + path.split("/").map(encodeURIComponent).join("/");
  };

  const evaluate = (code, options) => {
    const filename = filenameOf(options);
    const specifier = specifierOf(filename);
    const [value, error] = Deno.core.evalContext(String(code), specifier);
    if (error) throw error.thrown;
    return value;
  };

  const invalidContext = (context) =>
    new codes.ERR_INVALID_ARG_TYPE("contextifiedObject", "vm.Context", context);

  const contextId = (context) => {
    const id = contexts.get(context);
    if (id === undefined) throw invalidContext(context);
    return id;
  };

  const runInContext = (code, context, options) => {
    const id = contextId(context);
    const {
      filename = DEFAULT_FILENAME,
      lineOffset = 0,
      columnOffset = 0,
    } = optionsOf(options);
    const [threw, result] = ops.op_vm_run_in_context(
      id,
      String(code),
      String(filename),
      lineOffset | 0,
      columnOffset | 0,
    );
    if (threw) throw result;
    return result;
  };

  const createContext = (object, options) => {
    if (object !== undefined && (object === null || typeof object !== "object")) {
      throw new codes.ERR_INVALID_ARG_TYPE("object", "object", object);
    }
    if (object !== undefined) {
      throw new codes.ERR_NOT_IMPLEMENTED("vm.createContext with a sandbox object");
    }
    const codeGeneration = options?.codeGeneration;
    const [id, global] = ops.op_vm_create_context(codeGeneration?.strings !== false);
    contexts.set(global, id);
    return global;
  };

  const runInNewContext = (code, contextObject, options) => {
    const context = createContext(contextObject, options);
    return runInContext(code, context, options);
  };

  class Script {
    #code;
    #filename;
    #lineOffset;
    #columnOffset;

    constructor(code, options) {
      this.#code = String(code);
      const {
        filename = DEFAULT_FILENAME,
        lineOffset = 0,
        columnOffset = 0,
      } = optionsOf(options);
      this.#filename = filename;
      this.#lineOffset = lineOffset;
      this.#columnOffset = columnOffset;
    }

    runInThisContext(options) {
      return evaluate(this.#code, options ?? this.#runOptions());
    }

    runInContext(context, options) {
      return runInContext(this.#code, context, options ?? this.#runOptions());
    }

    runInNewContext(contextObject, options) {
      return runInNewContext(
        this.#code,
        contextObject,
        options ?? this.#runOptions(),
      );
    }

    #runOptions() {
      return {
        filename: this.#filename,
        lineOffset: this.#lineOffset,
        columnOffset: this.#columnOffset,
      };
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
    createContext,
    runInContext,
    runInNewContext,
    runInThisContext: (code, options) => evaluate(code, options),
    isContext: (object) => contexts.has(object),
    compileFunction,
    SourceTextModule: Module,
    SyntheticModule: Module,
  };
})();
