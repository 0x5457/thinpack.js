// Vendored from Node.js v26.13.1 lib/internal/file.js (MIT, https://github.com/nodejs/node).
// Patched: require("internal/...") -> require("node:thinpack/internal/...").

'use strict';

const {
  DateNow,
  FunctionPrototypeCall,
  NumberIsNaN,
  ObjectDefineProperties,
  ObjectSetPrototypeOf,
  StringPrototypeToWellFormed,
  Symbol,
  SymbolToStringTag,
} = primordials;

const {
  Blob,
  TransferableBlob,
} = require('node:thinpack/internal/blob');

const {
  customInspectSymbol: kInspect,
  kEnumerableProperty,
  kEmptyObject,
} = require('node:thinpack/internal/util');

const {
  codes: {
    ERR_INVALID_THIS,
    ERR_MISSING_ARGS,
  },
} = require('node:thinpack/internal/errors');

const {
  inspect,
} = require('node:thinpack/internal/util/inspect');

const {
  kClone,
  kDeserialize,
} = require('node:thinpack/internal/worker/js_transferable');

const kState = Symbol('state');

function isFile(object) {
  return object?.[kState] !== undefined;
}

class FileState {
  name;
  lastModified;

  /**
   * @param {string} name
   * @param {number} lastModified
   */
  constructor(name, lastModified) {
    this.name = name;
    this.lastModified = lastModified;
  }
}

class File extends Blob {
  constructor(fileBits, fileName, options = kEmptyObject) {
    if (arguments.length < 2) {
      throw new ERR_MISSING_ARGS('fileBits', 'fileName');
    }

    super(fileBits, options);

    let { lastModified } = options ?? kEmptyObject;

    if (lastModified !== undefined) {
      // Using Number(...) will not throw an error for bigints.
      lastModified = +lastModified;

      if (NumberIsNaN(lastModified)) {
        lastModified = 0;
      }
    } else {
      lastModified = DateNow();
    }

    this[kState] = new FileState(StringPrototypeToWellFormed(`${fileName}`), lastModified);
  }

  get name() {
    if (!isFile(this))
      throw new ERR_INVALID_THIS('File');

    return this[kState].name;
  }

  get lastModified() {
    if (!isFile(this))
      throw new ERR_INVALID_THIS('File');

    return this[kState].lastModified;
  }

  [kInspect](depth, options) {
    if (depth < 0) {
      return this;
    }

    const opts = {
      ...options,
      depth: options.depth == null ? null : options.depth - 1,
    };

    return `File ${inspect({
      size: this.size,
      type: this.type,
      name: this[kState].name,
      lastModified: this[kState].lastModified,
    }, opts)}`;
  }

  [kClone]() {
    return {
      data: { ...super[kClone]().data, ...this[kState] },
      deserializeInfo: 'node:thinpack/internal/file:TransferableFile',
    };
  }

  [kDeserialize](data) {
    super[kDeserialize](data);

    this[kState] = new FileState(data.name, data.lastModified);
  }
}

function TransferableFile(handle, length, type = '') {
  FunctionPrototypeCall(TransferableBlob, this, handle, length, type);
  ObjectSetPrototypeOf(this, File.prototype);
}

ObjectSetPrototypeOf(TransferableFile.prototype, File.prototype);
ObjectSetPrototypeOf(TransferableFile, File);

ObjectDefineProperties(File.prototype, {
  name: kEnumerableProperty,
  lastModified: kEnumerableProperty,
  [SymbolToStringTag]: {
    __proto__: null,
    configurable: true,
    value: 'File',
  },
});

module.exports = {
  File,
  TransferableFile,
};
