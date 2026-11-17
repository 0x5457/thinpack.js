use std::sync::OnceLock;
use std::time::Instant;

use deno_core::{OpDecl, op2};

pub(crate) const OPS: &[OpDecl] = &[op_perf_now()];

fn start() -> &'static Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now)
}

#[op2(fast)]
#[allow(
    clippy::cast_precision_loss,
    reason = "the nanosecond counter only exceeds f64's exact integer range after ~104 days of runtime, and node's clock is a float millisecond value by definition"
)]
fn op_perf_now() -> f64 {
    start().elapsed().as_nanos() as f64 / 1_000_000.0
}
