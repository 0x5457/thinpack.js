'use strict';
// Minimal stand-in for Node.js `test/common/index.js`.
//
// Node's own harness reaches deep into the runtime (internal bindings, worker
// introspection, diagnostic channels). The ported tests only need the helpers
// they actually use, so this file grows one helper at a time. Unknown property
// access throws, mirroring Node's harness, so a missing helper names itself
// instead of silently evaluating to `undefined`.

const assert = require('assert');
const { inspect } = require('util');

const platform = process.platform;
const isWindows = platform === 'win32';
const isLinux = platform === 'linux';
const isMacOS = platform === 'darwin';
const isAIX = platform === 'aix';
const isIBMi = platform === 'os400';
const isFreeBSD = platform === 'freebsd';
const isOpenBSD = platform === 'openbsd';
const isSunOS = platform === 'sunos';

const noop = () => {};

const hasCrypto = (() => {
  try {
    require('crypto');
    return !process.env.NODE_SKIP_CRYPTO;
  } catch {
    return false;
  }
})();
const hasIntl = typeof Intl === 'object';
const hasFullICU = (() => {
  try {
    const january = new Date(9e8);
    const spanish = new Intl.DateTimeFormat('es', { month: 'long' });
    return spanish.format(january) === 'enero';
  } catch {
    return false;
  }
})();
const hasTemporal = typeof Temporal !== 'undefined';
const hasInspector = Boolean(process.features?.inspector);
const hasSQLite = (() => {
  try {
    require('sqlite');
    return true;
  } catch {
    return false;
  }
})();
const hasFFI = (() => {
  try {
    require('ffi');
    return true;
  } catch {
    return false;
  }
})();

const mustCallChecks = [];

function runCallChecks(exitCode) {
  if (exitCode !== 0) return;

  const failed = mustCallChecks.filter(function(context) {
    if ('minimum' in context) {
      context.messageSegment = `at least ${context.minimum}`;
      return context.actual < context.minimum;
    }
    context.messageSegment = `exactly ${context.exact}`;
    return context.actual !== context.exact;
  });

  for (const context of failed) {
    console.log('Mismatched %s function calls. Expected %s, actual %d.',
                context.name,
                context.messageSegment,
                context.actual);
    console.log(context.stack.split('\n').slice(2).join('\n'));
  }

  if (failed.length) process.exit(1);
}

process.on('exit', runCallChecks);

function _mustCallInner(fn, criteria = 1, field) {
  if (process._exiting) {
    throw new Error('Cannot use common.mustCall*() in process exit handler');
  }
  if (typeof fn === 'number') {
    criteria = fn;
    fn = noop;
  } else if (fn === undefined) {
    fn = noop;
  }

  if (typeof criteria !== 'number') {
    throw new TypeError(`Invalid ${field} value: ${criteria}`);
  }

  const context = {
    [field]: criteria,
    actual: 0,
    stack: inspect(new Error()),
    name: fn.name || '<anonymous>',
  };

  mustCallChecks.push(context);

  const _return = function(...args) {
    context.actual++;
    return fn.apply(this, args);
  };
  Object.defineProperties(_return, {
    name: {
      value: fn.name,
      writable: false,
      enumerable: false,
      configurable: true,
    },
    length: {
      value: fn.length,
      writable: false,
      enumerable: false,
      configurable: true,
    },
  });
  return _return;
}

function mustCall(fn, exact) {
  return _mustCallInner(fn, exact, 'exact');
}

function mustSucceed(fn, exact) {
  return mustCall(function(err, ...args) {
    assert.ifError(err);
    if (typeof fn === 'function') return fn.apply(this, args);
  }, exact);
}

function mustCallAtLeast(fn, minimum) {
  return _mustCallInner(fn, minimum, 'minimum');
}

function mustNotCall(message) {
  return function mustNotCall(...args) {
    const argsInfo = args.length > 0 ?
      `\ncalled with arguments: ${args.map((arg) => inspect(arg)).join(', ')}` : '';
    assert.fail(`${message || 'function should not have been called'}${argsInfo}`);
  };
}

let catchWarning;

function _expectWarning(name, expected, code) {
  if (typeof expected === 'string') {
    expected = [[expected, code]];
  } else if (!Array.isArray(expected)) {
    expected = Object.entries(expected).map(([a, b]) => [b, a]);
  } else if (expected.length !== 0 && !Array.isArray(expected[0])) {
    expected = [[expected[0], expected[1]]];
  }
  if (name === 'DeprecationWarning') {
    expected.forEach(([, warningCode]) => {
      assert(warningCode, `Missing deprecation code: ${expected}`);
    });
  }
  return mustCall((warning) => {
    const expectedProperties = expected.shift();
    if (!expectedProperties) {
      assert.fail(`Unexpected extra warning received: ${warning}`);
    }
    const [message, warningCode] = expectedProperties;
    assert.strictEqual(warning.name, name);
    if (typeof message === 'string') {
      assert.strictEqual(warning.message, message);
    } else {
      assert.match(warning.message, message);
    }
    assert.strictEqual(warning.code, warningCode);
  }, expected.length);
}

function spawnPromisified(...args) {
  const { spawn } = require('child_process');
  let stderr = '';
  let stdout = '';

  const child = spawn(...args);
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (data) => { stderr += data; });
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (data) => { stdout += data; });

  return new Promise((resolve, reject) => {
    child.on('close', (code, signal) => {
      resolve({ code, signal, stderr, stdout });
    });
    child.on('error', (error) => {
      reject(error);
    });
  });
}

function expectWarning(nameOrMap, expected, code) {
  if (catchWarning === undefined) {
    catchWarning = {};
    process.on('warning', (warning) => {
      if (!catchWarning[warning.name]) {
        throw new TypeError(
          `"${warning.name}" was triggered without being expected.\n${inspect(warning)}`,
        );
      }
      catchWarning[warning.name](warning);
    });
  }
  if (typeof nameOrMap === 'string') {
    catchWarning[nameOrMap] = _expectWarning(nameOrMap, expected, code);
  } else {
    Object.keys(nameOrMap).forEach((name) => {
      catchWarning[name] = _expectWarning(name, nameOrMap[name]);
    });
  }
}

function expectsError(validator, exact) {
  return mustCall((...args) => {
    if (args.length !== 1) {
      assert.fail(`Expected one argument, got ${inspect(args)}`);
    }
    const error = args.pop();
    assert.strictEqual(Object.prototype.propertyIsEnumerable.call(error, 'message'),
                       false);
    assert.throws(() => { throw error; }, validator);
    return true;
  }, exact);
}

// Mirrors the message suffix Node appends to ERR_INVALID_ARG_TYPE so ported
// tests can spell out the whole expected message.
function invalidArgTypeHelper(input) {
  if (input == null) {
    return ` Received ${input}`;
  }
  if (typeof input === 'function') {
    return ` Received function ${input.name}`;
  }
  if (typeof input === 'object') {
    const name = input.constructor?.name;
    if (typeof name === 'string' && name !== '') {
      return ` Received an instance of ${name}`;
    }
    return ` Received ${inspect(input, { depth: -1 })}`;
  }

  let inspected = inspect(input, { colors: false });
  if (inspected.length > 28) inspected = `${inspected.slice(0, 25)}...`;

  return ` Received type ${typeof input} (${inspected})`;
}

function mustNotMutateObjectDeep(original) {
  if (original === null || typeof original !== 'object') {
    return original;
  }
  const handler = {
    defineProperty() {
      assert.fail('Expected no side effects, got a property defined');
    },
    deleteProperty() {
      assert.fail('Expected no side effects, got a property deleted');
    },
    preventExtensions() {
      assert.fail('Expected no side effects, got extensions prevented');
    },
    set() {
      assert.fail('Expected no side effects, got a property assigned');
    },
    setPrototypeOf() {
      assert.fail('Expected no side effects, got a prototype set');
    },
  };
  return new Proxy(original, handler);
}

function isAlive(pid) {
  try {
    process.kill(pid, 'SIGCONT');
    return true;
  } catch {
    return false;
  }
}

function printSkipMessage(message) {
  console.log(`1..0 # Skipped: ${message}`);
}

function skip(message) {
  printSkipMessage(message);
  process.exit(0);
}

function skipIf32Bits() {
  if (process.arch !== 'x64' && process.arch !== 'arm64') {
    skip('The tested feature is not available in 32bit builds');
  }
}

function canCreateSymLink() {
  return true;
}

function allowGlobals() {
  // The suite does not police leaked globals, so there is nothing to allow.
}

function platformTimeout(milliseconds) {
  return milliseconds;
}

function getTTYfd() {
  const tty = require('tty');
  const ttyFd = [1, 2, 4, 5].find(tty.isatty);
  if (ttyFd === undefined) {
    try {
      return require('fs').openSync('/dev/tty');
    } catch {
      return -1;
    }
  }
  return ttyFd;
}

function getArrayBufferViews(buf) {
  const { buffer, byteOffset, byteLength } = buf;
  const views = [
    Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
    Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array,
    BigUint64Array, DataView,
  ];
  if (typeof Float16Array !== 'undefined') views.splice(7, 0, Float16Array);
  return views.filter((type) => {
    const width = type.BYTES_PER_ELEMENT ?? 1;
    return byteLength % width === 0;
  }).map((type) => {
    const width = type.BYTES_PER_ELEMENT ?? 1;
    return new type(buffer, byteOffset, byteLength / width);
  });
}

function getBufferSources(buf) {
  return [...getArrayBufferViews(buf), new Uint8Array(buf).buffer];
}

const common = {
  platform,
  isWindows,
  isLinux,
  isMacOS,
  isAIX,
  isIBMi,
  isFreeBSD,
  isOpenBSD,
  isSunOS,
  isDebug: false,
  hasCrypto,
  hasIntl,
  hasFullICU,
  hasTemporal,
  hasInspector,
  hasSQLite,
  hasFFI,
  localhostIPv4: '127.0.0.1',
  localIPv6Hosts: ['::1'],
  PORT: 12346,
  mustCall,
  mustCallAtLeast,
  mustNotCall,
  mustSucceed,
  expectsError,
  expectWarning,
  spawnPromisified,
  invalidArgTypeHelper,
  mustNotMutateObjectDeep,
  isAlive,
  skip,
  skipIf32Bits,
  canCreateSymLink,
  allowGlobals,
  platformTimeout,
  getTTYfd,
  getArrayBufferViews,
  getBufferSources,
  printSkipMessage,
  enoughTestMem: () => true,

  get hasIPv6() {
    const interfaces = require('os').networkInterfaces();
    const loopback = isWindows ? /Loopback Pseudo-Interface/ : /lo/;
    return Object.keys(interfaces).some((name) => {
      return loopback.test(name) &&
        interfaces[name].some(({ family }) => family === 'IPv6');
    });
  },
};

const validProperties = new Set(Object.keys(common));
module.exports = new Proxy(common, {
  get(target, property) {
    if (!validProperties.has(property)) {
      throw new Error(`Using invalid common property: '${String(property)}'`);
    }
    return target[property];
  },
});
