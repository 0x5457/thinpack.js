import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const fixtures = require('./fixtures.js');

export const { fixturesDir, path, fileURL, readSync } = fixtures;
export default fixtures;
