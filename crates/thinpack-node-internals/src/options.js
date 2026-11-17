(() => {
  const { codes } = require("node:thinpack/internal/errors");

  const optionValues = {
    "--eval": "",
    "--experimental-stream-iter": false,
    "--enable-source-maps": false,
    "--no-deprecation": false,
    "--pending-deprecation": false,
  };

  const getOptionValue = (name) => {
    if (!Object.hasOwn(optionValues, name)) {
      throw new codes.ERR_INVALID_ARG_VALUE("name", name, "is not a valid option");
    }
    return optionValues[name];
  };

  module.exports = { getOptionValue };
})();
