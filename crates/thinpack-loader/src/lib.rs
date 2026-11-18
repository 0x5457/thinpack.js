mod ops;

mod cjs_exports;

mod esm;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deno_core::{Extension, ExtensionFileSource, ModuleLoader};
use thinpack_core::BuiltinSources;
use thinpack_resolve::{FileSystem, HostFs};
use thinpack_vfs::Vfs;

use crate::ops::OPS;

pub use crate::esm::{EsmLoader, is_esm};

#[derive(Clone, Debug)]
pub enum Main {
    File(PathBuf),
    Eval(String),
}

#[derive(Clone, Debug)]
pub enum ModuleFs {
    Host,
    Virtual(Rc<Vfs>),
}

impl FileSystem for ModuleFs {
    fn is_file(&self, path: &std::path::Path) -> bool {
        match self {
            Self::Host => HostFs.is_file(path),
            Self::Virtual(vfs) => vfs.is_file(path),
        }
    }

    fn is_dir(&self, path: &std::path::Path) -> bool {
        match self {
            Self::Host => HostFs.is_dir(path),
            Self::Virtual(vfs) => vfs.is_dir(path),
        }
    }

    fn read(&self, path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Host => HostFs.read(path),
            Self::Virtual(vfs) => vfs.read(path),
        }
    }
}

impl ModuleFs {
    /// Renders a resolved module path the way JS names it.
    ///
    /// Payload paths are POSIX paths rooted at `/app` on every platform, and JS
    /// receives them as module ids. Windows would print the native separators a
    /// payload never records, so a packed module would report a filename the
    /// manifest does not contain and lose the `/app` root in `__dirname`.
    pub(crate) fn js_path(&self, path: &Path) -> String {
        let rendered = path.display().to_string();
        match self {
            Self::Host => rendered,
            Self::Virtual(_) => posix_separators(rendered),
        }
    }
}

/// Host paths already use the platform separator; payload paths are POSIX.
#[cfg(windows)]
fn posix_separators(path: String) -> String {
    path.replace('\\', "/")
}

#[cfg(not(windows))]
const fn posix_separators(path: String) -> String {
    path
}

#[must_use]
pub fn esm_loader(fs: ModuleFs, builtins: BuiltinSources) -> Rc<dyn ModuleLoader> {
    Rc::new(EsmLoader::new(fs, builtins))
}

#[must_use]
pub fn init(main: Main, fs: ModuleFs) -> Extension {
    const JS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "thinpack:loader",
        thinpack_js::js!("module.js"),
    )];

    Extension {
        name: "thinpack_loader",
        ops: Cow::Borrowed(OPS),
        js_files: Cow::Borrowed(JS),
        op_state_fn: Some(Box::new(move |state| {
            state.put(main);
            state.put(fs);
        })),
        ..Default::default()
    }
}

/// Runs the entry registered with [`init`].
///
/// # Errors
///
/// Returns the error raised by the entry script or by its event loop.
pub fn run(runtime: &mut thinpack_core::Runtime) -> Result<(), thinpack_core::Error> {
    runtime.execute_script("<thinpack:main>", "__thinpack_run_main()".to_owned())
}
