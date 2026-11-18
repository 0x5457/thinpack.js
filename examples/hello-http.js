const http = require("node:http");

const guard = setTimeout(() => {
  console.log("TIMEOUT");
  process.exit(1);
}, 10_000);

const server = http.createServer((request, response) => {
  response.end(`hello ${request.method} ${request.url}\n`);
});

server.on("error", (error) => {
  console.log(`server error: ${error}`);
  process.exit(1);
});

server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  console.log(`listening on 127.0.0.1:${port}`);
  http
    .get(`http://127.0.0.1:${port}/demo`, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => {
        clearTimeout(guard);
        process.stdout.write(body);
        server.close(() => process.exit(0));
      });
    })
    .on("error", (error) => {
      console.log(`client error: ${error}`);
      process.exit(1);
    });
});
