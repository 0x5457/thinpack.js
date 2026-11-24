use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    QuickJs,
    V8,
    /// The macOS system `JavaScriptCore` framework; never bundles an engine.
    Jsc,
}

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

impl Backend {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::QuickJs => "quickjs",
            Self::V8 => "v8",
            Self::Jsc => "jsc",
        }
    }

    #[must_use]
    pub const fn engine(self) -> &'static str {
        match self {
            Self::QuickJs => "QuickJS-ng via v8x",
            Self::V8 => "V8",
            Self::Jsc => "JavaScriptCore (macOS system framework)",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
