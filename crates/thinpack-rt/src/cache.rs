use std::env;
use std::path::PathBuf;

pub(crate) fn prepare_bytecode_cache() {
    if !matches!(thinpack_core::BACKEND, thinpack_core::Backend::QuickJs) {
        return;
    }
    if env::var_os("V82JSC_BC_CACHE_DIR").is_some() || env::var_os("V82JSC_NO_BC_CACHE").is_some() {
        return;
    }
    let Ok(root) = thinpack_pack::cache_root() else {
        return;
    };
    let dir: PathBuf = root.join("bytecode");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // SAFETY: the cache is provisioned before this process creates a tokio runtime, a V8
    // isolate or a worker thread, so nothing else reads or writes the environment yet.
    unsafe { env::set_var("V82JSC_BC_CACHE_DIR", &dir) };
}
