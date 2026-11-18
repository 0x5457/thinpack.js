use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::Extension;
use deno_core::ExtensionFileSource;

use crate::Error;

#[derive(Clone)]
pub struct BuiltinSources(Rc<BTreeMap<String, Arc<str>>>);

impl fmt::Debug for BuiltinSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BuiltinSources")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl BuiltinSources {
    #[must_use]
    pub fn get(&self, specifier: &str) -> Option<&str> {
        self.0.get(specifier).map(|source| &**source)
    }

    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.0
            .keys()
            .filter(|name| name.starts_with("node:"))
            .map(String::as_str)
            .collect()
    }

    /// Collects every builtin module source declared by the extensions.
    ///
    /// Each source is decoded once, here, and handed back to the extension as a
    /// `Computed` source: `deno_core` loads every lazy source when the runtime is
    /// created, and a compressed source would otherwise be decoded a second time.
    ///
    /// # Errors
    ///
    /// Returns an error when an extension declares a builtin that is loaded from
    /// disk instead of being embedded in the binary.
    pub fn collect(extensions: &mut [Extension]) -> Result<Self, Error> {
        let mut sources = BTreeMap::new();

        for extension in extensions {
            let mut files = Vec::with_capacity(extension.lazy_loaded_js_files.len());
            for file in extension.lazy_loaded_js_files.iter() {
                if !file.is_runtime_loadable() {
                    return Err(Error::new(format!(
                        "{}: builtin module source must be embedded (thinpack_js::js!), \
                         not loaded from disk",
                        file.specifier
                    )));
                }
                let code = file.load().map_err(Error::from)?;
                let source: Arc<str> = Arc::from(code.as_str());
                sources.insert(file.specifier.to_owned(), Arc::clone(&source));
                files.push(ExtensionFileSource::new_computed(file.specifier, source));
            }
            extension.lazy_loaded_js_files = Cow::Owned(files);
        }

        Ok(Self(Rc::new(sources)))
    }
}
