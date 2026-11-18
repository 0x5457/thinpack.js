const os = require("os");

console.log(`platform: ${os.platform()}`);
console.log(`cpus: ${os.cpus().length > 0}`);
console.log(`totalmem: ${os.totalmem() > 0}`);
console.log(`interfaces: ${Object.keys(os.networkInterfaces()).length > 0}`);
