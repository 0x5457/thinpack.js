(() => {
  const deno = Deno.core.loadExtScript("ext:deno_ffi/00_ffi.js");
  const { UnsafeCallback, UnsafePointer, UnsafePointerView } = deno;

  const types = Object.freeze({
    VOID: "void",
    POINTER: "pointer",
    BUFFER: "buffer",
    ARRAY_BUFFER: "arraybuffer",
    FUNCTION: "function",
    BOOL: "bool",
    CHAR: "char",
    STRING: "string",
    FLOAT: "float",
    DOUBLE: "double",
    INT_8: "int8",
    UINT_8: "uint8",
    INT_16: "int16",
    UINT_16: "uint16",
    INT_32: "int32",
    UINT_32: "uint32",
    INT_64: "int64",
    UINT_64: "uint64",
    FLOAT_32: "float32",
    FLOAT_64: "float64",
  });

  const typeName = (type) => {
    if (typeof type !== "string") {
      throw new TypeError("FFI types must be strings");
    }
    switch (type) {
      case "void": case "pointer": case "ptr": case "buffer":
      case "arraybuffer": case "function": case "bool": case "char":
      case "string": case "cstring": case "float": case "double":
      case "int8": case "i8": case "uint8": case "u8":
      case "int16": case "i16": case "uint16": case "u16":
      case "int32": case "i32": case "uint32": case "u32":
      case "int64": case "i64": case "uint64": case "u64":
      case "usize": case "isize": case "float32": case "f32":
      case "float64": case "f64":
        return type === "ptr" ? "pointer" : type;
      default:
        throw new TypeError("Unsupported FFI type: " + String(type));
    }
  };

  const denoDefinition = (definition) => ({
    parameters: (definition.arguments ?? definition.args ?? []).map(typeName),
    result: typeName(definition.return ?? definition.returns ?? "void"),
  });

  const isPointerType = (type) => {
    const name = typeName(type);
    return name === "pointer" || name === "function" || name === "buffer" ||
      name === "arraybuffer";
  };

  const toDenoArg = (value, type) => {
    if (!isPointerType(type)) return value;
    if (value === null || value === undefined) return null;
    if (typeof value === "bigint" || typeof value === "number") {
      return UnsafePointer.create(BigInt(value));
    }
    if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) {
      return UnsafePointer.of(value);
    }
    if (value instanceof UnsafeCallback) return value.pointer;
    return value;
  };

  const fromDenoValue = (value, type) => {
    if (typeName(type) === "pointer" || typeName(type) === "function") {
      if (value === null || value === undefined) return 0n;
      return BigInt(UnsafePointer.value(value));
    }
    return value;
  };

  const wrapSymbol = (fn, definition) => {
    const args = definition.arguments ?? definition.args ?? [];
    return (...parameters) => {
      const converted = parameters.map((value, index) =>
        toDenoArg(value, args[index] ?? "pointer")
      );
      return fromDenoValue(fn(...converted), definition.return ?? definition.returns ?? "void");
    };
  };

  const dlopen = (path, definitions) => {
    const denoDefinitions = {};
    for (const [name, definition] of Object.entries(definitions)) {
      denoDefinitions[name] = denoDefinition(definition);
    }
    const library = deno.dlopen(path, denoDefinitions);
    const functions = {};
    for (const [name, definition] of Object.entries(definitions)) {
      functions[name] = wrapSymbol(library.symbols[name], definition);
    }
    const callbacks = new Map();
    const lib = {
      registerCallback(definition, callback) {
        const normalized = denoDefinition(definition);
        const args = definition.arguments ?? definition.args ?? [];
        const wrapped = (...parameters) => {
          const converted = parameters.map((value, index) => {
            const type = args[index];
            if (type !== undefined && isPointerType(type)) {
              return value === null ? 0n : BigInt(UnsafePointer.value(value));
            }
            return value;
          });
          const result = callback(...converted);
          const returnType = definition.return ?? definition.returns ?? "void";
          if (returnType === "pointer" || returnType === "function") {
            return UnsafePointer.create(BigInt(result ?? 0));
          }
          return result;
        };
        const callbackHandle = new UnsafeCallback(normalized, wrapped);
        const pointerValue = BigInt(UnsafePointer.value(callbackHandle.pointer));
        callbacks.set(pointerValue, callbackHandle);
        return pointerValue;
      },
      unregisterCallback(pointer) {
        const value = BigInt(pointer);
        const callback = callbacks.get(value);
        if (callback !== undefined) {
          callbacks.delete(value);
          callback.close();
        }
      },
      close() {
        library.close();
        for (const callback of callbacks.values()) callback.close();
        callbacks.clear();
      },
    };
    return { lib, functions };
  };

  const getRawPointer = (value) => BigInt(UnsafePointer.value(UnsafePointer.of(value)));
  const ptr = (value) => BigInt(UnsafePointer.value(UnsafePointer.of(value)));
  const toArrayBuffer = (pointer, length, _copy = true) =>
    UnsafePointerView.getArrayBuffer(UnsafePointer.create(BigInt(pointer)), length);

  const suffix = process.platform === "win32" ? ".dll" :
    process.platform === "darwin" ? ".dylib" : ".so";

  module.exports = {
    dlopen,
    getRawPointer,
    ptr,
    suffix,
    toArrayBuffer,
    types,
  };
})();
