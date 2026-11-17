(() => {
  const { codes } = require("node:thinpack/internal/errors");
  class SigintWatchdog {
    start() {
      throw new codes.ERR_NOT_IMPLEMENTED("util.setTraceSigInt(true)");
    }
    stop() {}
  }
  module.exports = { SigintWatchdog };
})();
