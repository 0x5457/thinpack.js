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
