import assert from 'node:assert/strict';
import defaultTest, {
    after,
    afterEach,
    before,
    beforeEach,
    describe,
    getTestContext,
    it,
    mock,
    test,
} from 'node:test';

const events = [];
const target = {
    read(value) {
        return value;
    },
};

describe('node:test', () => {
    before(() => events.push('before'));
    after(() => events.push('after'));
    beforeEach(() => events.push('beforeEach'));
    afterEach(() => events.push('afterEach'));

    it('runs synchronous tests and suite hooks', () => {
        events.push('sync');
    });

    it('runs async tests and nested subtests', async (context) => {
        events.push('parent');
        assert.strictEqual(context.name, 'runs async tests and nested subtests');
        assert.strictEqual(getTestContext(), context);
        await context.test('nested test', async (nested) => {
            events.push('nested');
            assert.strictEqual(nested.fullName, `${context.fullName} > nested test`);
            assert.strictEqual(getTestContext(), nested);
        });
    });
});

test('suite hooks wrap their tests and nested subtests', () => {
    assert.deepStrictEqual(events, [
        'before',
        'beforeEach',
        'sync',
        'afterEach',
        'beforeEach',
        'parent',
        'beforeEach',
        'nested',
        'afterEach',
        'afterEach',
        'after',
    ]);
});

test('mock.method records calls and restores the original method', () => {
    const read = mock.method(target, 'read', (value) => `mocked:${value}`);
    assert.strictEqual(target.read('value'), 'mocked:value');
    assert.strictEqual(read.mock.callCount(), 1);
    assert.deepStrictEqual(read.mock.calls[0].arguments, ['value']);
    read.mock.restore();
    assert.strictEqual(target.read('value'), 'value');
});

test('mock.fn records calls', () => {
    const fn = mock.fn((value) => value + 1);
    assert.strictEqual(fn(4), 5);
    assert.strictEqual(fn.mock.callCount(), 1);
    assert.deepStrictEqual(fn.mock.calls[0].arguments, [4]);
});

test('default export keeps the Node test function aliases', () => {
    assert.strictEqual(defaultTest, test);
    assert.strictEqual(test.test, test);
    assert.strictEqual(test.it, it);
    assert.strictEqual(test.describe, describe);
});

test.skip('skip options do not run the test body', () => {
    assert.fail('skipped test body ran');
});

test.todo('todo tests execute and do not fail the run', () => {
    events.push('todo option continued');
    throw new Error('todo failure is suppressed');
});

test('context.skip marks the test without interrupting its body', (context) => {
    context.skip('skipped from the test context');
    events.push('context skip continued');
});

test('context.todo marks the test without interrupting its body', (context) => {
    context.todo('todo from the test context');
    events.push('context todo continued');
    throw new Error('context todo failure is suppressed');
});

test('todo and skip bodies ran as expected', () => {
    assert.ok(events.includes('todo option continued'));
    assert.ok(events.includes('context skip continued'));
    assert.ok(events.includes('context todo continued'));
});

afterEach(() => mock.restoreAll());
