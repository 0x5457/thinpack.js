(() => {
  const { codes } = require("node:thinpack/internal/errors");
  const notImplemented = (name) => new codes.ERR_NOT_IMPLEMENTED(`inspector.${name}`);

  class Session {
    connect() { throw notImplemented("Session.connect"); }
    connectToMainThread() { throw notImplemented("Session.connectToMainThread"); }
    disconnect() {}
    post(method, params, callback) {
      const error = notImplemented(`Session.post(${method})`);
      if (typeof callback === "function") {
        queueMicrotask(() => callback(error));
        return;
      }
      Promise.reject(error);
    }
  }

  module.exports = {
    Session,
    console,
    open() { throw notImplemented("inspector.open"); },
    close() {},
    url: () => undefined,
    waitForDebugger() {},
  };
})();
