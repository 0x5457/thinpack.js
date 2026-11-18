'use strict';

const assert = require('node:assert/strict');

const headers = new Headers([['X-First', '1'], ['x-second', '2'], ['x-first', '3']]);
assert.equal(headers.get('X-FIRST'), '1, 3');
assert.equal(headers.has('x-second'), true);
assert.equal(headers.has('x-third'), false);
assert.equal(headers.get('x-third'), null);

// Iteration order is first-appearance order with the values folded together.
assert.deepEqual([...headers], [['x-first', '1, 3'], ['x-second', '2']]);
assert.deepEqual([...headers.keys()], ['x-first', 'x-second']);
assert.deepEqual([...headers.values()], ['1, 3', '2']);

// `set` replaces the values in place, `append` adds at the end.
headers.set('X-First', '9');
headers.append('x-third', '4');
assert.deepEqual([...headers], [['x-first', '9'], ['x-second', '2'], ['x-third', '4']]);

headers.delete('x-first');
assert.deepEqual([...headers], [['x-second', '2'], ['x-third', '4']]);

const visited = [];
headers.forEach((value, name, source) => {
  assert.equal(source, headers);
  visited.push([name, value]);
});
assert.deepEqual(visited, [['x-second', '2'], ['x-third', '4']]);

const cookies = new Headers();
cookies.append('set-cookie', 'a=1');
cookies.append('set-cookie', 'b=2');
assert.deepEqual(cookies.getSetCookie(), ['a=1', 'b=2']);

assert.throws(() => headers.append('bad name', 'x'), TypeError);
assert.throws(() => headers.append('x', 'bad\nvalue'), TypeError);
assert.equal(new Headers({ 'X-Record': 'value' }).get('x-record'), 'value');
assert.equal(new Headers(headers).get('x-second'), '2');

const { fetch, Request, Response, FormData } = globalThis;
assert.equal(typeof fetch, 'function');
assert.equal(typeof Request, 'function');
assert.equal(typeof Response, 'function');
assert.equal(typeof FormData, 'function');
