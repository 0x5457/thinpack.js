(() => {
  const { URL } = globalThis.__thinpack_url;
  const format = (href, fragment, unicode, search, auth) => {
    const url = new URL(href);
    let result = url.href;
    if (!fragment && url.hash !== "") {
      result = result.slice(0, -url.hash.length);
    }
    if (!search && url.search !== "") {
      result = result.slice(0, -url.search.length);
    }
    if (!auth) {
      const userinfo = url.username + (url.password === "" ? "" : ":" + url.password);
      if (userinfo !== "") {
        const hostStart = url.protocol.length + 2;
        result =
          result.slice(0, hostStart) + result.slice(hostStart + userinfo.length + 1);
      }
    }
    return result;
  };

  module.exports = { format };
})();
