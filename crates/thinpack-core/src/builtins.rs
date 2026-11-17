use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use deno_core::Extension;

use crate::Error;

#[derive(Clone)]
pub struct BuiltinSources(Rc<BTreeMap<String, String>>);

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
        self.0.get(specifier).map(String::as_str)
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
    /// # Errors
    ///
    /// Returns an error when an extension declares a builtin that is loaded from
    /// disk instead of being embedded in the binary.
    pub fn collect(extensions: &[Extension]) -> Result<Self, Error> {
        let mut sources = BTreeMap::new();

        for extension in extensions {
            for file in &*extension.lazy_loaded_js_files {
                if !file.is_runtime_loadable() {
                    return Err(Error::new(format!(
                        "{}: builtin module source must be embedded (ascii_str_include!), \
                         not loaded from disk",
                        file.specifier
                    )));
                }
                let source = file.load().map_err(Error::from)?;
                sources.insert(file.specifier.to_owned(), source.as_str().to_owned());
            }
        }

        Ok(Self(Rc::new(sources)))
    }
}
