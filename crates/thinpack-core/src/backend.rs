use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    QuickJs,
    V8,
}

#[cfg(not(any(feature = "backend-quickjs", feature = "backend-v8")))]
compile_error!("enable exactly one backend: `backend-quickjs` or `backend-v8`");

#[cfg(all(feature = "backend-quickjs", feature = "backend-v8"))]
compile_error!("`backend-quickjs` and `backend-v8` are mutually exclusive");

#[cfg(feature = "backend-quickjs")]
pub const BACKEND: Backend = Backend::QuickJs;

#[cfg(feature = "backend-v8")]
pub const BACKEND: Backend = Backend::V8;

impl Backend {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::QuickJs => "quickjs",
            Self::V8 => "v8",
        }
    }

    #[must_use]
    pub const fn engine(self) -> &'static str {
        match self {
            Self::QuickJs => "QuickJS-ng via v8x",
            Self::V8 => "V8",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
