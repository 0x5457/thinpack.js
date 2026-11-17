(() => {
  const { ops } = Deno.core;

  const keys = () => {
    const raw = ops.op_sea_asset_keys();
    return raw === "" ? [] : raw.split("\0");
  };

  module.exports = {
    isSea: () => ops.op_sea_is_sea(),
    getAsset(key) {
      if (!keys().includes(key)) return undefined;
      return ops.op_sea_asset(key);
    },
    getAssetKeys: () => keys(),
  };
})();
