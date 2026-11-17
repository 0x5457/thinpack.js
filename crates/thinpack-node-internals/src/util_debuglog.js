(() => {
  const {
    format,
    formatWithOptions,
    inspect,
  } = require("node:thinpack/internal/util/inspect");
  const {
    validateFunction,
    validateString,
  } = require("node:thinpack/internal/validators");

  const debugEnvironment = process.env.NODE_DEBUG;
  let testEnabled = () => false;
  if (debugEnvironment) {
    const pattern = debugEnvironment
      .replace(/[|\\{}()[\]^$+?.]/g, "\\$&")
      .replaceAll("*", ".*")
      .replaceAll(",", "$|^");
    const enabledSections = new RegExp("^" + pattern + "$", "i");
    testEnabled = (section) => enabledSections.test(section);
  }
  const warnAboutSensitiveSections = (section) => {
    if (section !== "HTTP" && section !== "HTTP2") return;
    process.emitWarning(
      "Setting the NODE_DEBUG environment variable to '" +
        section.toLowerCase() +
        "' can expose sensitive data (such as passwords, tokens and " +
        "authentication headers) in the resulting log.",
    );
  };

  const createDebug = (section) => (...args) => {
    process.stderr.write(
      format(
        "%s %s: %s\n",
        section,
        inspect(process.pid),
        formatWithOptions({ colors: false }, ...args),
      ),
    );
  };

  const debuglog = (section, callback) => {
    validateString(section, "section");
    if (callback !== undefined) validateFunction(callback, "callback");

    const name = section.toUpperCase();
    let enabled;
    let debug;
    const initialize = () => {
      if (enabled !== undefined) return;
      enabled = testEnabled(name);
      if (enabled) {
        warnAboutSensitiveSections(name);
        debug = createDebug(name);
      } else {
        debug = () => {};
      }
      if (callback !== undefined) callback(debug);
    };

    const logger = (...args) => {
      initialize();
      debug(...args);
    };
    Object.defineProperty(logger, "enabled", {
      configurable: true,
      enumerable: true,
      get() {
        initialize();
        return enabled;
      },
    });
    return logger;
  };

  module.exports = { debuglog };
})();
