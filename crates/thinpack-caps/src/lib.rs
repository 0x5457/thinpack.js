mod table;

use std::collections::BTreeSet;

pub use table::{MODULES, Module};

#[must_use]
pub fn module(name: &str) -> Option<&'static Module> {
    MODULES.iter().find(|module| module.name == name)
}

pub fn baseline() -> impl Iterator<Item = &'static Module> {
    MODULES.iter().filter(|module| module.ambient)
}

#[must_use]
pub fn module_for_builtin(builtin: &str) -> Option<&'static Module> {
    let name = builtin.strip_prefix("node:").unwrap_or(builtin);
    MODULES
        .iter()
        .find(|module| module.builtins.contains(&name))
}

#[must_use]
pub fn module_for_global(global: &str) -> Option<&'static Module> {
    MODULES
        .iter()
        .find(|module| module.globals.contains(&global))
}

/// # Errors
///
/// Returns the name of a module that has no entry in the capability table.
pub fn closure(roots: &[String]) -> Result<Vec<&'static Module>, String> {
    let mut included: BTreeSet<&'static str> = BTreeSet::default();
    let mut pending: Vec<String> = roots.to_vec();

    while let Some(name) = pending.pop() {
        let module = module(&name).ok_or_else(|| format!("unknown module `{name}`"))?;
        if included.insert(module.name) {
            pending.extend(module.deps.iter().map(|dep| (*dep).to_owned()));
        }
    }

    Ok(MODULES
        .iter()
        .filter(|module| included.contains(module.name))
        .collect())
}

#[must_use]
pub fn capabilities_hash(names: &[&str]) -> u64 {
    let mut sorted: Vec<&str> = names.to_vec();
    sorted.sort_unstable();

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for name in sorted {
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= u64::from(b'\n');
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
