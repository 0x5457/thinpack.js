// `util.throttle`'s token math needs a monotonic clock: a wall-clock step
// would skew the schedule and could not be mocked away. `op_perf_now` is the
// same monotonically increasing millisecond source `performance.now()` uses.
const { op_perf_now } = Deno.core.ops;

module.exports = {
  getLibuvNow: () => Math.round(op_perf_now()),
};
