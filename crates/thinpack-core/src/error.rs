use std::error::Error as StdError;
use std::fmt;

use deno_core::error::{CoreError, CoreErrorKind};
use deno_core::url::Url;
use deno_error::{JsErrorClass, PropertyValue};

#[derive(Debug)]
pub struct Error(Box<dyn StdError + Send + Sync>);

impl Error {
    pub fn new(error: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self(error.into())
    }

    pub(crate) fn esm_load(specifier: &Url, error: CoreError) -> Self {
        // `module instantiation failed` is synthesized by
        // `vendor/deno_core/modules/map.rs` when the engine reports no exception; the
        // unit test below pins the literal so a vendored bump cannot reword it silently.
        if let CoreErrorKind::Js(exception) = error.as_kind()
            && exception.name.as_deref() == Some("Error")
            && exception.message.as_deref() == Some("module instantiation failed")
            && exception.frames.is_empty()
        {
            return Self::new(format!(
                "{error}\nESM graph entry: {specifier}\nThe JavaScript engine did not report an underlying exception."
            ));
        }
        Self::from(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error) = self.0.downcast_ref::<CoreError>()
            && let CoreErrorKind::JsBox(error) = error.as_kind()
            && let Some(PropertyValue::String(code)) = error
                .get_additional_properties()
                .find_map(|(key, value)| (key == "code").then_some(value))
        {
            return write!(formatter, "{} [{code}]: {error}", error.get_class());
        }
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

impl From<CoreError> for Error {
    fn from(error: CoreError) -> Self {
        Self::new(error)
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use deno_core::error::{CoreError, CoreErrorKind, JsError};
    use deno_core::url::Url;

    fn instantiation_failure() -> CoreError {
        CoreError(Box::new(CoreErrorKind::Js(Box::new(JsError {
            name: Some("Error".to_owned()),
            message: Some("module instantiation failed".to_owned()),
            stack: None,
            cause: None,
            exception_message: "module instantiation failed".to_owned(),
            frames: Vec::new(),
            source_line: None,
            source_line_frame_index: None,
            aggregated: None,
            additional_properties: Vec::new(),
            stack_is_custom: false,
        }))))
    }

    #[test]
    fn degenerate_instantiation_failure_names_the_entry() {
        let specifier = Url::parse("file:///app/main.mjs").expect("valid url");
        let error = Error::esm_load(&specifier, instantiation_failure());
        let text = error.to_string();
        assert!(
            text.contains("ESM graph entry: file:///app/main.mjs"),
            "{text}"
        );
        assert!(
            text.contains("did not report an underlying exception"),
            "{text}"
        );
    }
}
