(() => {
  // The loader cannot resolve builtins while extensions are still initializing, so defer the
  // actual require until the runtime calls this hook after installing the module sources.
  Object.defineProperty(globalThis, "__thinpack_child_ipc_bootstrap__", {
    configurable: true,
    value: () => {
      const port = process.platform === "win32" && process.env.NODE_CHANNEL_PORT;
      if (!process.env.NODE_CHANNEL_FD && !port) return;
      globalThis.__thinpack_require__("node:thinpack/internal/child_process/channel")
        .setup();
    },
  });
})();
