//! The loader's structured errors: a JavaScript `Error` with a `code` property
//! and, where one exists, the URL or path the failure names.

use std::borrow::Cow;
use std::fmt;

use deno_error::{AdditionalProperties, JsErrorClass, PropertyValue};

#[derive(Debug)]
pub(crate) struct LoaderError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) url: Option<String>,
}

impl LoaderError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            url: None,
        }
    }
}

impl fmt::Display for LoaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LoaderError {}

impl JsErrorClass for LoaderError {
    fn get_class(&self) -> Cow<'static, str> {
        "Error".into()
    }

    fn get_message(&self) -> Cow<'static, str> {
        self.message.clone().into()
    }

    fn get_additional_properties(&self) -> AdditionalProperties {
        let code = ("code".into(), PropertyValue::from(self.code));
        let url = self.url.clone().map(|url| ("url".into(), url.into()));
        Box::new(std::iter::once(code).chain(url))
    }

    fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
}

/// A failure raised by an ESM loader hook, in the fields a rebuilt `Error` needs.
///
/// The hook runs in another isolate, so the error crosses the loader bridge as data
/// instead of a live `v8` exception. Node hands a hook's failure to the app as it was
/// thrown, so keep the class, message, `code` and stack.
#[derive(Clone, Debug)]
pub(crate) struct HookError {
    pub(crate) class: String,
    pub(crate) message: String,
    pub(crate) code: Option<String>,
    pub(crate) stack: Option<String>,
}

impl HookError {
    pub(crate) fn new(class: &str, message: impl Into<String>) -> Self {
        Self {
            class: class.to_owned(),
            message: message.into(),
            code: None,
            stack: None,
        }
    }

    /// Encodes the failure for the JS side of the bridge, which rebuilds the error
    /// with the class, message, `code` and stack the hook threw.
    #[cfg(feature = "esm-hooks")]
    pub(crate) fn encode(&self) -> String {
        format!(
            "err\0{}\0{}\0{}\0{}",
            self.class,
            self.message,
            self.code.as_deref().unwrap_or_default(),
            self.stack.as_deref().unwrap_or_default(),
        )
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HookError {}

impl JsErrorClass for HookError {
    fn get_class(&self) -> Cow<'static, str> {
        Cow::Owned(self.class.clone())
    }

    fn get_message(&self) -> Cow<'static, str> {
        Cow::Owned(self.message.clone())
    }

    fn get_additional_properties(&self) -> AdditionalProperties {
        let code = self.code.clone().map(|code| ("code".into(), code.into()));
        let stack = self
            .stack
            .clone()
            .map(|stack| ("stack".into(), stack.into()));
        Box::new(code.into_iter().chain(stack))
    }

    fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
}
