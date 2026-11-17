const socket = new WebSocket(process.argv[2]);
socket.addEventListener("open", () => socket.send("hello"));
socket.addEventListener("message", (event) => {
  console.log("message", event.data);
  socket.close();
});
socket.addEventListener("close", (event) => {
  console.log("close", event.code, event.wasClean);
});
socket.addEventListener("error", (event) => console.log("error", event.message));
