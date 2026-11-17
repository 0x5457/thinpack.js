module.exports = {
  kClone: Symbol("kClone"),
  kDeserialize: Symbol("kDeserialize"),
  kTransfer: Symbol("kTransfer"),
  kTransferList: Symbol("kTransferList"),
  kSerializer: Symbol("kSerializer"),
  markTransferMode() {},
  structuredClone: globalThis.__thinpack_structured_clone,
};
