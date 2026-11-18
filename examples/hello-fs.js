const fs = require("fs");

const path = "thinpack-smoke.txt";
fs.writeFileSync(path, "hello from fs");
console.log(fs.readFileSync(path, "utf8"));
console.log(`isFile: ${fs.statSync(path).isFile()}`);
fs.utimesSync(path, 1000, 2000);
console.log(`mtimeMs: ${fs.statSync(path).mtimeMs}`);
fs.unlinkSync(path);
