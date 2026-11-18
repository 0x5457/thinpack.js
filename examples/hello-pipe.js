const net = require("net");

const path = process.platform === "win32"
  ? `\\\\.\\pipe\\thinpack-smoke-${process.pid}`
  : `/tmp/thinpack-smoke-${process.pid}`;

const guard = setTimeout(() => {
  console.log("TIMEOUT");
  process.exit(1);
}, 10_000);

const server = net.createServer((socket) => {
  console.log("server connection");
  socket.end("pipe ok");
});

server.on("error", (error) => {
  console.log(`server error: ${error}`);
  process.exit(1);
});

server.listen(path, () => {
  console.log("listening");
  const client = net.connect(path, () => {
    console.log("connected");
    let data = "";
    client.setEncoding("utf8");
    client.on("data", (chunk) => { data += chunk; });
    client.on("end", () => {
      console.log(`client end: ${JSON.stringify(data)}`);
      clearTimeout(guard);
      server.close();
      process.exit(data === "pipe ok" ? 0 : 1);
    });
  });
  client.on("error", (error) => {
    console.log(`client error: ${error}`);
    process.exit(1);
  });
});
