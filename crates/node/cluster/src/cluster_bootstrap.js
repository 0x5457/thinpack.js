(() => {
  if (!Object.hasOwn(process.env, "NODE_UNIQUE_ID") ||
      (!Object.hasOwn(process.env, "NODE_CHANNEL_FD") &&
       !Object.hasOwn(process.env, "NODE_CHANNEL_PORT"))) {
    return;
  }
  // The loader cannot resolve builtins while extensions are still initializing, so defer the
  // actual require until the runtime calls this hook after installing the module sources.
  Object.defineProperty(globalThis, "__thinpack_cluster_bootstrap__", {
    configurable: true,
    value: () => {
      const cluster = globalThis.__thinpack_require__("node:cluster");
      cluster._setupWorker();
    },
  });
})();
