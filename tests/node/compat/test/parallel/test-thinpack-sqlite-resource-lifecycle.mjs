import assert from 'node:assert/strict';
import { DatabaseSync, backup } from 'node:sqlite';

const db = new DatabaseSync(':memory:');
db.function('close_during_step', () => {
  assert.throws(
    () => db.close(),
    {
      code: 'ERR_INVALID_STATE',
      message: 'database cannot be closed while in a callback',
    },
  );
  return 1;
});

const guarded = db.prepare('SELECT close_during_step() AS value');
assert.deepEqual({ ...guarded.get() }, { value: 1 });

const statement = db.prepare('SELECT 1 AS value');
const session = db.createSession();
const closedSession = db.createSession();
closedSession.close();
assert.throws(
  () => closedSession.changeset(),
  { code: 'ERR_INVALID_STATE', message: 'session is not open' },
);

db.close();
assert.throws(
  () => statement.get(),
  { code: 'ERR_INVALID_STATE', message: 'statement has been finalized' },
);
assert.throws(() => session.changeset(), { code: 'ERR_INVALID_STATE' });
let backupError;
try {
  await backup(db, 'unreachable.db');
} catch (error) {
  backupError = error;
}
assert.equal(backupError?.code, 'ERR_INVALID_STATE');
assert.equal(backupError?.message, 'database is not open');

db.open();
assert.deepEqual({ ...db.prepare('SELECT 2 AS value').get() }, { value: 2 });
db.close();
