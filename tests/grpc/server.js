const fs = require("fs");
const path = require("path");
const grpc = require("@grpc/grpc-js");
const protoLoader = require("@grpc/proto-loader");

const fixtures = path.join(__dirname, "..", "fixtures", "tls");
const definition = protoLoader.loadSync(path.join(__dirname, "echo.proto"));
const proto = grpc.loadPackageDefinition(definition).echo;

const server = new grpc.Server();
server.addService(proto.Echo.service, {
  sayHello(call, callback) {
    if (call.request.message === "fail") {
      callback({ code: 3, details: "bad request" });
      return;
    }
    callback(null, { message: "reply: " + call.request.message });
  },
  sayHelloStream(call) {
    for (const suffix of ["one", "two", "three"]) {
      call.write({ message: `stream ${suffix}: ${call.request.message}` });
    }
    call.end();
  },
});
const address = process.argv[2] === "tls" ? "localhost:0" : "127.0.0.1:0";
const credentials =
  process.argv[2] === "tls"
    ? grpc.ServerCredentials.createSsl(null, [
        {
          private_key: fs.readFileSync(path.join(fixtures, "server.key")),
          cert_chain: fs.readFileSync(path.join(fixtures, "server.pem")),
        },
      ])
    : grpc.ServerCredentials.createInsecure();

server.bindAsync(address, credentials, (error, port) => {
  if (error) throw error;
  console.log(port);
});
