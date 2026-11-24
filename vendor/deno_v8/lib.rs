// Copyright 2018-2026 the Deno authors. MIT license.

#[cfg(any(
  all(feature = "v8", feature = "quickjs"),
  all(feature = "v8", feature = "jsc"),
  all(feature = "quickjs", feature = "jsc"),
))]
compile_error!("features `v8`, `quickjs` and `jsc` are mutually exclusive");

#[cfg(not(any(feature = "v8", feature = "quickjs", feature = "jsc")))]
compile_error!("one of features `v8`, `quickjs` or `jsc` must be enabled");

#[cfg(all(feature = "v8", not(any(feature = "quickjs", feature = "jsc"))))]
pub use rusty_v8::*;
#[cfg(all(
  feature = "quickjs",
  not(any(feature = "v8", feature = "jsc"))
))]
pub use v8x_backend::*;
#[cfg(all(feature = "jsc", not(any(feature = "v8", feature = "quickjs"))))]
pub use v8x_backend::*;
