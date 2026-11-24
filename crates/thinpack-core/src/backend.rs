pub use thinpack_pack::Backend;

#[cfg(not(any(
    feature = "backend-quickjs",
    feature = "backend-v8",
    feature = "backend-jsc"
)))]
compile_error!("enable exactly one backend: `backend-quickjs`, `backend-v8` or `backend-jsc`");

#[cfg(any(
    all(feature = "backend-quickjs", feature = "backend-v8"),
    all(feature = "backend-quickjs", feature = "backend-jsc"),
    all(feature = "backend-v8", feature = "backend-jsc"),
))]
compile_error!("`backend-quickjs`, `backend-v8` and `backend-jsc` are mutually exclusive");

#[cfg(all(feature = "backend-jsc", not(target_os = "macos")))]
compile_error!("the `backend-jsc` template uses the macOS system JavaScriptCore framework");

#[cfg(feature = "backend-quickjs")]
pub const BACKEND: Backend = Backend::QuickJs;

#[cfg(feature = "backend-v8")]
pub const BACKEND: Backend = Backend::V8;

#[cfg(feature = "backend-jsc")]
pub const BACKEND: Backend = Backend::Jsc;
