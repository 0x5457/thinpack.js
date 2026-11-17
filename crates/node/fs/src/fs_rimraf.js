(() => {
  const fs = require("node:fs");

  const rimrafPromises = (path, options) =>
    Promise.resolve().then(() => {
      fs.rmSync(path, options);
    });

  const rimraf = (path, options, callback) => {
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
