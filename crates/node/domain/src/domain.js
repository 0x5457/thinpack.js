// Node's deprecated `domain` module, reduced to the synchronous subset PM2 and
// older Serverless builds use. It follows Node's lib/domain.js and Deno's
// ext/node/polyfills/domain.ts for add/remove/bind/intercept/run/enter/exit;
// async context propagation needs an AsyncHook-backed store and is not part of
// this subset.
const { EventEmitter } = require('node:events');

const stack = [];
let active = null;

process.domain = null;
process.emitWarning(
  'The `domain` module is deprecated and should not be used.',
  'DeprecationWarning',
  'DEP0032',
);

function emitError(error) {
  this.emit('error', error);
}

function annotate(error, domain, { thrown, bound }) {
  if (typeof error !== 'object' || error === null) return;
  error.domainThrown = thrown;
  if (bound !== undefined) error.domainBound = bound;
  Object.defineProperty(error, 'domain', {
    configurable: true,
    enumerable: false,
    value: domain,
    writable: true,
  });
}

class Domain extends EventEmitter {
  members = [];

  add(emitter) {
    if (emitter.domain === this) return;
    if (emitter.domain) emitter.domain.remove(emitter);
    Object.defineProperty(emitter, 'domain', {
      configurable: true,
      enumerable: false,
      value: this,
      writable: true,
    });
    this.members.push(emitter);
  }

  remove(emitter) {
    emitter.domain = null;
    const index = this.members.indexOf(emitter);
    if (index !== -1) this.members.splice(index, 1);
  }

  bind(fn) {
    const domain = this;
    return function bound(...args) {
      domain.enter();
      try {
        const result = Reflect.apply(fn, this, args);
        domain.exit();
        return result;
      } catch (error) {
        domain.exit();
        annotate(error, domain, { thrown: false, bound: fn });
        Reflect.apply(emitError, domain, [error]);
      }
    };
  }

  intercept(fn) {
    const domain = this;
    return function intercepted(error, ...args) {
      if (error) {
        annotate(error, domain, { thrown: false, bound: fn });
        Reflect.apply(emitError, domain, [error]);
        return undefined;
      }
      return domain.bind(fn).apply(this, args);
    };
  }

  run(fn, ...args) {
    this.enter();
    try {
      const result = Reflect.apply(fn, this, args);
      this.exit();
      return result;
    } catch (error) {
      this.exit();
      annotate(error, this, { thrown: true });
      Reflect.apply(emitError, this, [error]);
      return undefined;
    }
  }

  enter() {
    active = this;
    process.domain = this;
    stack.push(this);
    return this;
  }

  exit() {
    const index = stack.lastIndexOf(this);
    if (index !== -1) stack.splice(index);
    active = stack.length === 0 ? null : stack[stack.length - 1];
    process.domain = active;
    return this;
  }

  dispose() {
    this.removeAllListeners();
    return this;
  }
}

function create() {
  return new Domain();
}

module.exports = {
  Domain,
  create,
  createDomain: create,
};
