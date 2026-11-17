use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub struct Error(Box<dyn StdError + Send + Sync>);

impl Error {
    pub fn new(error: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self(error.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0.source()
    }
}

impl From<Box<deno_core::error::JsError>> for Error {
    fn from(error: Box<deno_core::error::JsError>) -> Self {
        Self::new(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(error)
    }
}

impl From<deno_core::error::CoreError> for Error {
    fn from(error: deno_core::error::CoreError) -> Self {
        Self::new(error)
    }
}
