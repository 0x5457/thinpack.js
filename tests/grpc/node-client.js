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

const stream = (message) =>
  new Promise((resolve, reject) => {
    const call = client.sayHelloStream({ message });
    call.on("data", (response) => console.log("STREAM", response.message));
    call.on("error", reject);
    call.on("end", resolve);
  });

client.sayHello({ message: "hello" }, (error, response) => {
  if (error) {
    console.error("CALL ERROR", error.code, error.message);
    process.exitCode = 1;
    return;
  }
  console.log("REPLY", response.message);
  client.sayHello({ message: "fail" }, (failure) => {
    if (!failure) {
      console.error("expected the call to fail");
      process.exitCode = 1;
      return;
    }
    console.log("FAILED", failure.code, failure.details);
    stream("paged")
      .then(() => client.close())
      .catch((error) => {
        console.error("CALL ERROR", error.code, error.message);
        process.exitCode = 1;
      });
  });
});
