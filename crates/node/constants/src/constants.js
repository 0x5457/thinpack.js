const constants = require("node:thinpack/internal/constants");

module.exports = {
  ...constants.os.errno,
  ...constants.os.signals,
  ...constants.fs,
};
