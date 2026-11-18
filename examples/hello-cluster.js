const cluster = require("cluster");

console.log(`cluster start: primary=${cluster.isPrimary} unique=${process.env.NODE_UNIQUE_ID} port=${process.env.NODE_CHANNEL_PORT}`);

const guard = setTimeout(() => {
  console.log("TIMEOUT");
  process.exit(1);
}, 15_000);

if (cluster.isPrimary) {
  let worker;
  try {
    worker = cluster.fork();
  } catch (error) {
    console.log(`fork error: ${error}`);
    process.exit(1);
  }
  console.log("forked");
  worker.on("message", (message) => {
    console.log(`primary: ${message}`);
    worker.kill();
  });
  worker.on("error", (error) => {
    console.log(`worker error: ${error}`);
  });
  worker.on("exit", () => {
    clearTimeout(guard);
    process.exit(0);
  });
} else {
  console.log("worker branch");
  process.send("worker online");
  console.log("worker sent");
}
