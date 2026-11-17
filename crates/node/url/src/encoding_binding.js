(() => {
  const { ops } = Deno.core;

  const toASCII = (domain) => ops.op_url_domain_to_ascii(`${domain}`);

  module.exports = { toASCII };
})();
