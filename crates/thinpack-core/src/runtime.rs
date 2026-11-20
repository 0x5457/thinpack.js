use std::fmt;
use std::path::Path;
use std::rc::Rc;

use deno_core::{
    Extension, JsRuntime, ModuleLoader, PollEventLoopOptions, RuntimeOptions, url::Url,
};

use crate::worker::WorkerContext;
use crate::{Assets, BuiltinSources, Error, Invocation, js_string_literal};

pub struct Runtime {
    inner: JsRuntime,
}

impl fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Runtime").finish_non_exhaustive()
    }
}

impl Runtime {
    /// # Errors
    ///
    /// Returns an error when the engine cannot be initialised with the given
    /// extensions and module loader.
    pub fn new(
        extensions: Vec<Extension>,
        builtins: BuiltinSources,
        module_loader: Rc<dyn ModuleLoader>,
    ) -> Result<Self, Error> {
        let inner = JsRuntime::try_new(RuntimeOptions {
            extensions,
            module_loader: Some(module_loader),
            shared_array_buffer_store: Some(crate::worker::shared_array_buffer_store()),
            ..Default::default()
        })?;
        inner.op_state().borrow_mut().put(builtins);
        Ok(Self { inner })
    }

    pub fn put_state<T: 'static>(&self, value: T) {
        self.inner.op_state().borrow_mut().put(value);
    }

    /// # Errors
    ///
    /// Returns an error when the invocation script fails to execute.
    pub fn install_invocation(&mut self, invocation: &Invocation) -> Result<(), Error> {
        let script = format!(
            "if (typeof process !== \"undefined\" && Array.isArray(process.argv)) {{\n  process.argv.splice(1, 0, {});\n}}",
            js_string_literal(&invocation.0)
        );
        self.inner.execute_script("<invocation>", script)?;
        Ok(())
    }

    pub fn install_assets(&self, assets: Assets) {
        self.inner.op_state().borrow_mut().put(assets);
    }

    pub fn install_worker_context(&self, context: WorkerContext) {
        self.inner.op_state().borrow_mut().put(context);
    }

    /// # Errors
    ///
    /// Returns an error when the script fails to compile or throws.
    pub fn execute_script(&mut self, name: &str, source: String) -> Result<(), Error> {
        self.inner.execute_script(name.to_owned(), source)?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when the file cannot be turned into a specifier, fails
    /// to load, or rejects while evaluating.
    pub async fn run_esm_file(&mut self, path: &Path) -> Result<(), Error> {
        let specifier = esm_specifier(path)?;
        let module = self
            .inner
            .load_main_es_module(&specifier)
            .await
            .map_err(|error| Error::esm_load(&specifier, error))?;
        let evaluated = self.inner.mod_evaluate(module);
        self.inner
            .run_event_loop(PollEventLoopOptions::default())
            .await?;
        evaluated.await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when the file cannot be turned into a specifier or fails
    /// to compile.
    pub async fn precompile_esm(&mut self, path: &Path) -> Result<(), Error> {
        let specifier = esm_specifier(path)?;
        self.inner
            .load_side_es_module(&specifier)
            .await
            .map_err(|error| Error::esm_load(&specifier, error))?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error when a pending op fails or a module rejects.
    pub async fn run_event_loop(&mut self) -> Result<(), Error> {
        self.inner
            .run_event_loop(PollEventLoopOptions::default())
            .await?;
        Ok(())
    }
}

fn esm_specifier(path: &Path) -> Result<Url, Error> {
    file_url(path).ok_or_else(|| Error::new(format!("{} is not a valid file path", path.display())))
}

/// Converts a host path or a drive-less virtual payload path to a file URL.
#[must_use]
pub fn file_url(path: &Path) -> Option<Url> {
    Url::from_file_path(path)
        .ok()
        .or_else(|| virtual_file_url(path))
}

#[cfg(windows)]
fn virtual_file_url(path: &Path) -> Option<Url> {
    if !matches!(
        path.components().next(),
        Some(std::path::Component::RootDir)
    ) {
        return None;
    }
    // Use a drive to encode the path for Windows and then restore the portable URL.
    let mut prefixed = std::ffi::OsString::from("C:");
    prefixed.push(path.as_os_str());
    let url = Url::from_file_path(Path::new(&prefixed)).ok()?;
    let encoded_path = url.path().strip_prefix("/C:")?;
    Url::parse(&format!("file://{encoded_path}")).ok()
}

#[cfg(not(windows))]
const fn virtual_file_url(_path: &Path) -> Option<Url> {
    None
}
