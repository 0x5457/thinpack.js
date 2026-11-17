(() => {
  const parseAddress = (value) => {
    const index = value.lastIndexOf(":");
    if (index === -1) {
      return { address: value, port: undefined, family: undefined };
    }
    const address = value.slice(0, index).replace(/^\[|\]$/g, "");
    return {
      address,
      port: Number(value.slice(index + 1)),
      family: address.includes(":") ? "IPv6" : "IPv4",
    };
  };

  module.exports = { parseAddress };
})();
