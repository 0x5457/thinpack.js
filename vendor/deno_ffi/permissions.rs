// thinpack has no permission model for host calls; FFI is an explicit capability of
// the module and the runtime runs only application code. Keep the Deno API shape
// so the vendored implementation stays close to upstream.
use std::borrow::Cow;
use std::path::Path;

#[derive(Debug)]
pub struct PermissionCheckError;

impl std::fmt::Display for PermissionCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("permission denied")
    }
}

impl std::error::Error for PermissionCheckError {}

impl deno_error::JsErrorClass for PermissionCheckError {
    fn get_class(&self) -> std::borrow::Cow<'static, str> {
        "Error".into()
    }
    fn get_message(&self) -> std::borrow::Cow<'static, str> {
        "permission denied".into()
    }
    fn get_additional_properties(&self) -> deno_error::AdditionalProperties {
        Box::new(std::iter::empty())
    }
    fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PermissionsContainer;

impl PermissionsContainer {
    pub fn check_ffi_partial_with_path<'a>(
        &self,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, PermissionCheckError> {
        Ok(path)
    }

    pub fn check_ffi_partial_no_path(&self) -> Result<(), PermissionCheckError> {
        Ok(())
    }
}
