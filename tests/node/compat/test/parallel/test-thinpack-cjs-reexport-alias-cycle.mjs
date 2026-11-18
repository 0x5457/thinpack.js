import assert from 'node:assert/strict';

let evaluationError;
try {
  await import('../fixtures/thinpack-cjs-reexport-alias-cycle.cjs');
} catch (error) {
  evaluationError = error;
}

assert.ok(evaluationError instanceof Error, 'the CJS module should fail during evaluation');
assert.equal(evaluationError.name, 'ReferenceError');
// V8 says "before initialization"; QuickJS says "a is not initialized".
assert.match(evaluationError.message, /initialized/i);
