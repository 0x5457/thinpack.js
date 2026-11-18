(() => {
  const fs = require("node:fs");

  const rimrafPromises = (path, options) =>
    // `rmSync` runs to completion inside the microtask, so `fs.rm` blocks the
    // event loop; an async rm op is not implemented.
    Promise.resolve().then(() => {
      fs.rmSync(path, options);
    });

  const rimraf = (path, options, callback) => {
    // Same as above: the callback form blocks the loop while the removal runs.
    queueMicrotask(() => {
      try {
        fs.rmSync(path, options);
        callback(null);
      } catch (error) {
        callback(error);
      }
    });
  };

  module.exports = { rimraf, rimrafPromises };
})();
