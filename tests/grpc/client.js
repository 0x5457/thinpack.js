const fs = require("fs");
const path = require("path");
const grpc = require("@grpc/grpc-js");
const protoLoader = require("@grpc/proto-loader");

const fixtures = path.join(__dirname, "..", "fixtures", "tls");
const definition = protoLoader.loadSync(path.join(__dirname, "echo.proto"));
const proto = grpc.loadPackageDefinition(definition).echo;

const target = process.argv[2];
const credentials =
  process.argv[3] === "tls"
    ? grpc.credentials.createSsl(fs.readFileSync(path.join(fixtures, "ca.pem")))
    : grpc.credentials.createInsecure();
const client = new proto.Echo(target, credentials);

const call = (message) =>
  new Promise((resolve, reject) => {
    client.sayHello({ message }, (error, response) => {
      if (error) reject(error);
      else resolve(response.message);
    });
  });

const stream = (message) =>
  new Promise((resolve, reject) => {
    const call = client.sayHelloStream({ message });
    call.on("data", (response) => console.log("STREAM", response.message));
    call.on("error", reject);
    call.on("end", resolve);
  });

const failing = () =>
  new Promise((resolve, reject) => {
    client.sayHello({ message: "fail" }, (error) => {
      if (error) resolve(`${error.code} ${error.details}`);
      else reject(new Error("expected the call to fail"));
    });
  });

call("first")
  .then((reply) => {
    console.log("REPLY", reply);
    return call("second");
  })
  .then((reply) => {
    console.log("REPLY", reply);
    return failing();
  })
  .then((details) => {
    console.log("FAILED", details);
    return stream("paged");
  })
  .then(() => client.close())
  .catch((error) => {
    console.error("CALL ERROR", error.code, error.message);
    process.exitCode = 1;
  });
