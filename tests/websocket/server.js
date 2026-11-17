const { WebSocketServer } = require("ws");

const server = new WebSocketServer({ host: "127.0.0.1", port: 0 });
server.on("listening", () => console.log(server.address().port));
server.on("connection", (socket) => {
  socket.on("message", (message) => socket.send("echo:" + message));
});
