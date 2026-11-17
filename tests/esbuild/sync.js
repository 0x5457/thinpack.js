const esbuild = require("esbuild");

const transformed = esbuild.transformSync("const x: number = 1;", { loader: "ts" });
console.log("transform", transformed.code.trim());

const built = esbuild.buildSync({
  stdin: { contents: "export const y = 2;", loader: "ts" },
  write: false,
  format: "esm",
});
console.log("build", built.outputFiles[0].text.trim().replace(/\n/g, "|"));
