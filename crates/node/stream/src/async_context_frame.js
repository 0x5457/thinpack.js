(() => {
  module.exports = {
    current: () => Deno.core.getAsyncContext(),
  };
})();
