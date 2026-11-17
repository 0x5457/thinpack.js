(() => {
  const subscribers = new Uint32Array(4096);

  module.exports = {
    subscribers,
    notifyChannelActive() {},
    notifyChannelInactive() {},
    linkNativeChannel() {},
  };
})();
