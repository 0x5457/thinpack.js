(() => {
  const childOrPrimary = Object.hasOwn(process.env, "NODE_UNIQUE_ID") ? "child" : "primary";
  module.exports = require(`node:thinpack/internal/cluster/${childOrPrimary}`);
})();
