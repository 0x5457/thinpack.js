'use strict';
const fs = require('fs');
const os = require('os');
const path = require('path');

const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'thinpack-cluster-'));

module.exports = {
  path: directory,
  refresh() {
    fs.rmSync(directory, { recursive: true, force: true });
    fs.mkdirSync(directory, { recursive: true });
    return directory;
  },
};
