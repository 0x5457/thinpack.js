(() => {
  const {
    AsyncLocalStorage,
    AsyncResource,
    createHook,
    executionAsyncId,
    executionAsyncResource,
    triggerAsyncId,
  } = require("node:thinpack/internal/async_hooks");

  module.exports = {
    AsyncLocalStorage,
    AsyncResource,
    createHook,
    executionAsyncId,
    executionAsyncResource,
    triggerAsyncId,
  };
})();
