(() => {
  const { Buffer } = require("node:buffer");

  const collect = async (stream) => {
    const chunks = [];
    for await (const chunk of stream) {
      chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : Buffer.from(chunk));
    }
    return Buffer.concat(chunks);
  };

  const buffer = (stream) => collect(stream);
  const arrayBuffer = async (stream) => {
    const bytes = await collect(stream);
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  };
  const text = async (stream) => (await collect(stream)).toString("utf8");
  const json = async (stream) => JSON.parse(await text(stream));
  const blob = async (stream) => new Blob([await collect(stream)]);

  module.exports = { buffer, arrayBuffer, text, json, blob };
})();
