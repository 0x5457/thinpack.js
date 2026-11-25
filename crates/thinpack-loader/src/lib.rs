mod ops;

mod error;

mod esm;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deno_core::{Extension, ExtensionFileSource, ModuleLoader};
use thinpack_core::{BuiltinSources, file_url};
use thinpack_resolve::{FileSystem, HostFs};
use thinpack_vfs::Vfs;

use crate::ops::OPS;

pub use crate::esm::{EsmLoader, is_esm};

fn missing_builtin_message(specifier: &str) -> String {
    let message = format!("No such built-in module: {specifier}");
    if let Some(module) = thinpack_caps::module_for_builtin(specifier) {
        format!(
            "{message}. This template omits it. Re-pack with --with {}.",
            module.name
        )
    } else {
        message
    }
}

#[derive(Clone, Debug)]
pub enum Main {
    File(PathBuf),
    Eval(String),
}

/// Where the module system finds the files it loads.
///
/// `Host` is the unpacked runtime, and every path is the machine's. `Packed`
/// mounts the executable's payload at `/app` and keeps the host file system for
/// everything else, the split the `fs` module already exposes. A packed app
/// therefore loads its own modules from the payload and files that exist on the
/// machine (a project `ESLint` config, say) from their real paths, as Node would.
#[derive(Clone, Debug)]
pub enum ModuleFs {
    Host,
    Packed(Rc<Vfs>),
}

impl FileSystem for ModuleFs {
    fn is_file(&self, path: &Path) -> bool {
        self.files(path).is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files(path).is_dir(path)
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.files(path).read(path)
    }

    fn format_override(&self, path: &Path) -> Option<thinpack_resolve::Format> {
        self.files(path).format_override(path)
    }

    fn custom_conditions(&self) -> &[String] {
        match self {
            Self::Host => &[],
            Self::Packed(vfs) => vfs.custom_conditions(),
        }
    }
}

impl ModuleFs {
    /// The file system that owns `path`: the payload below its root, the host
    /// elsewhere.
    fn files(&self, path: &Path) -> &dyn FileSystem {
        match self {
            Self::Packed(vfs) if path.starts_with(thinpack_vfs::ROOT) => vfs.as_ref(),
            Self::Host | Self::Packed(_) => &HostFs,
        }
    }

    pub(crate) fn conditions(&self, import: bool) -> thinpack_resolve::Conditions {
        thinpack_resolve::Conditions {
            import,
            custom: self.custom_conditions().to_vec(),
        }
    }

    /// Renders a resolved module path the way JS names it.
    ///
    /// Payload paths are POSIX paths rooted at `/app` on every platform, and JS
    /// receives them as module ids. Windows would print the native separators a
    /// payload never records, so a packed module would report a filename the
    /// manifest does not contain and lose the `/app` root in `__dirname`. Host
    /// paths keep the platform's rendering, as they do in an unpacked runtime.
    pub(crate) fn js_path(&self, path: &Path) -> String {
        let rendered = path.display().to_string();
        match self {
            Self::Packed(_) if path.starts_with(thinpack_vfs::ROOT) => posix_separators(rendered),
            Self::Host | Self::Packed(_) => rendered,
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

/// The engine name and wrapped source for a `CommonJS` module.
///
/// The loader and `--warm-cache` must produce the same `(eval flags, source,
/// resource name)` key, so both build it here; a hashbang becomes a line comment once.
#[must_use]
pub fn commonjs_script(path: &Path, source: &str) -> Option<(String, String)> {
    let name = file_url(path)?.to_string();
    let source = strip_hashbang(source);
    Some((
        name,
        format!("(function (exports, require, module, __filename, __dirname) {{{source}\n}})"),
    ))
}

fn strip_hashbang(source: &str) -> Cow<'_, str> {
    source.strip_prefix("#!").map_or_else(
        || Cow::Borrowed(source),
        |rest| Cow::Owned(format!("//{rest}")),
    )
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

#[cfg(test)]
mod tests {
    use super::commonjs_script;
    use std::path::Path;

    #[test]
    fn commonjs_script_strips_the_hashbang_and_wraps_once() {
        let (name, script) = commonjs_script(
            Path::new("/app/main.js"),
            "#!/usr/bin/env node\nconsole.log(1);\n",
        )
        .expect("the path becomes a file URL");
        assert_eq!(name, "file:///app/main.js");
        assert!(
            script.starts_with("(function (exports, require, module, __filename, __dirname) {//")
        );
        assert!(!script.contains("#!"));
        assert!(script.ends_with("\n})"));
    }
}
