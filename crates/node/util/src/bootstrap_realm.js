const BuiltinModule = {
  exists: (specifier) => (globalThis.__thinpack_builtin_modules ?? []).includes(specifier),
};

module.exports = { BuiltinModule };
