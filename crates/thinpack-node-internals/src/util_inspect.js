(() => {
  const IMPLEMENTATION = "node:thinpack/internal/util/inspect/impl";

  const fallback = {
    inspect: (value, options) => {
      if (typeof value === "string") return "'" + value + "'";
      if (value instanceof Error) {
        return value.stack || value.name + ": " + value.message;
      }
      if (options !== undefined && options !== null && options.depth === -1) {
        const name = value && value.constructor && value.constructor.name;
        return "[" + (name || "Object") + "]";
      }
      try {
        return JSON.stringify(value) ?? String(value);
      } catch {
        return String(value);
      }
    },
    identicalSequenceRange: () => ({ len: 0, offset: 0 }),
  };

  const load = () => {
    try {
      return require(IMPLEMENTATION);
    } catch (error) {
      const missingImplementation = error.code === "ERR_UNKNOWN_BUILTIN_MODULE" &&
        String(error.message).includes(IMPLEMENTATION);
      if (!missingImplementation) throw error;
      return fallback;
    }
  };

  module.exports = load();
})();
