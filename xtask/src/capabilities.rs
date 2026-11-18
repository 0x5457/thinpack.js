use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

const MANIFEST: &str = "capabilities.toml";
const TABLE: &str = "crates/thinpack-caps/src/table.rs";
const REGISTRY: &str = "crates/thinpack-rt/src/registry.rs";
const RT_MANIFEST: &str = "crates/thinpack-rt/Cargo.toml";
const FEATURES_BEGIN: &str = "# BEGIN GENERATED FEATURES";
const FEATURES_END: &str = "# END GENERATED FEATURES";
const BACKEND_FEATURES: [&str; 2] = ["backend-quickjs", "backend-v8"];

fn repo_path(relative: &str) -> PathBuf {
    crate::repo_root().join(relative)
}

#[derive(Debug)]
pub(crate) struct Module {
    pub name: String,
    pub krate: String,
    pub feature: String,
    pub builtins: Vec<String>,
    pub deps: Vec<String>,
    pub optional_deps: Vec<String>,
    pub ambient: bool,
    pub optional: bool,
}

impl Module {
    fn new(name: String, entry: Entry) -> Self {
        let slug = name.replace('_', "-");
        Self {
            krate: format!("thinpack-node-{slug}"),
            feature: format!("node-{slug}"),
            builtins: entry.builtins,
            deps: entry.deps,
            optional_deps: entry.optional_deps,
            ambient: entry.ambient,
            optional: entry.optional,
            name,
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    modules: BTreeMap<String, Entry>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    builtins: Vec<String>,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default)]
    optional_deps: Vec<String>,
    #[serde(default)]
    ambient: bool,
    #[serde(default)]
    optional: bool,
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => {
            eprintln!("Usage: cargo xtask gen [--check]");
            return ExitCode::from(2);
        }
    };

    match generate(check) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn load() -> Result<Vec<Module>, String> {
    let text =
        fs::read_to_string(repo_path(MANIFEST)).map_err(|error| format!("{MANIFEST}: {error}"))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("{MANIFEST}: {error}"))?;

    let mut modules: Vec<Module> = manifest
        .modules
        .into_iter()
        .map(|(name, entry)| Module::new(name, entry))
        .collect();
    modules.sort_by(|left, right| left.name.cmp(&right.name));

    let runtime_manifest = fs::read_to_string(repo_path(RT_MANIFEST))
        .map_err(|error| format!("{RT_MANIFEST}: {error}"))?;
    validate(&modules, &runtime_manifest)?;
    Ok(modules)
}

fn generate(check: bool) -> Result<(), String> {
    let modules = load()?;
    let order = dependency_order(&modules)?;
    let manifest = fs::read_to_string(repo_path(RT_MANIFEST))
        .map_err(|error| format!("{RT_MANIFEST}: {error}"))?;
    let sources = [
        (TABLE, render_table(&modules)),
        (REGISTRY, render_registry(&order)),
        (RT_MANIFEST, render_manifest(&manifest, &modules)?),
    ];

    let mut stale = Vec::new();
    for (name, expected) in &sources {
        if !sync(&repo_path(name), expected, check)? {
            stale.push((*name).to_owned());
        }
    }

    if stale.is_empty() {
        return Ok(());
    }

    let details = format!(
        "generated files disagree with {MANIFEST}: {}\nrun `cargo xtask gen` to regenerate.",
        stale.join(", ")
    );
    if check {
        return Err(details);
    }

    println!("regenerated: {}", stale.join(", "));
    Ok(())
}

fn sync(path: &Path, expected: &str, check: bool) -> Result<bool, String> {
    if fs::read_to_string(path).ok().as_deref() == Some(expected) {
        return Ok(true);
    }
    if !check {
        fs::write(path, expected).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(false)
}

fn validate(modules: &[Module], runtime_manifest: &str) -> Result<(), String> {
    let mut features: BTreeMap<&str, &str> = BACKEND_FEATURES
        .into_iter()
        .map(|feature| (feature, "thinpack-rt backend"))
        .collect();
    let mut builtins = BTreeMap::new();

    for module in modules {
        if !is_module_name(&module.name) {
            return Err(format!(
                "module name `{}` must be a lowercase identifier (a letter followed by lowercase letters, digits or underscores)",
                module.name
            ));
        }
        if let Some(other) = features.insert(&module.feature, &module.name) {
            return Err(format!(
                "feature `{}` is declared twice: by `{other}` and by `{}`",
                module.feature, module.name
            ));
        }
        if module.builtins.is_empty() {
            return Err(format!("module `{}` declares no builtin", module.name));
        }
        for builtin in &module.builtins {
            if let Some(other) = builtins.insert(builtin, &module.name) {
                return Err(format!(
                    "builtin `{builtin}` is declared twice: by `{other}` and by `{}`",
                    module.name
                ));
            }
        }
        check_crate_dir(&module.krate)?;
        check_optional_dependency(runtime_manifest, &module.krate)?;
    }

    for module in modules {
        for dep in &module.deps {
            if dep == &module.name {
                return Err(format!("module `{}` depends on itself", module.name));
            }
            if !modules.iter().any(|candidate| &candidate.name == dep) {
                return Err(format!(
                    "module `{}` depends on undeclared module `{dep}`",
                    module.name
                ));
            }
        }
        for dep in &module.optional_deps {
            if dep == &module.name {
                return Err(format!(
                    "module `{}` optionally depends on itself",
                    module.name
                ));
            }
            if !modules.iter().any(|candidate| &candidate.name == dep) {
                return Err(format!(
                    "module `{}` optionally depends on undeclared module `{dep}`",
                    module.name
                ));
            }
        }
    }

    dependency_order(modules).map(|_| ())
}

fn is_module_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_lowercase = chars.next().is_some_and(|first| first.is_ascii_lowercase());
    starts_lowercase && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn dependency_order(modules: &[Module]) -> Result<Vec<&Module>, String> {
    let mut ordered: Vec<&Module> = Vec::with_capacity(modules.len());
    let mut remaining: Vec<&Module> = modules.iter().collect();

    while !remaining.is_empty() {
        let ready: Vec<&Module> = remaining
            .iter()
            .copied()
            .filter(|module| {
                module
                    .deps
                    .iter()
                    .all(|dep| ordered.iter().any(|done| done.name == *dep))
            })
            .collect();

        if ready.is_empty() {
            let names: Vec<&str> = remaining
                .iter()
                .map(|module| module.name.as_str())
                .collect();
            return Err(format!("dependency cycle: {}", names.join(", ")));
        }

        remaining.retain(|module| !ready.iter().any(|before| before.name == module.name));
        ordered.extend(ready);
    }

    Ok(ordered)
}

fn check_crate_dir(krate: &str) -> Result<(), String> {
    let relative = if let Some(name) = krate.strip_prefix("thinpack-node-") {
        format!("crates/node/{name}")
    } else if let Some(name) = krate.strip_prefix("thinpack-") {
        format!("crates/{name}")
    } else {
        return Err(format!("package `{krate}` must start with thinpack-"));
    };

    if !repo_path(&relative).join("Cargo.toml").is_file() {
        return Err(format!(
            "no crate directory {relative} for package `{krate}`"
        ));
    }
    Ok(())
}

fn check_optional_dependency(manifest: &str, krate: &str) -> Result<(), String> {
    let declaration = format!("{krate} =");
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with(&declaration))
        .ok_or_else(|| format!("{RT_MANIFEST} has no dependency on `{krate}`"))?;

    if !line.contains("optional = true") {
        return Err(format!(
            "`{krate}` must be an optional dependency, otherwise it is linked even when disabled"
        ));
    }
    Ok(())
}

fn render_table(modules: &[Module]) -> String {
    // `optional_deps` stays in the manifest: xtask validates it and documents
    // the lazy dependencies, but no generated consumer reads it back.
    let mut source = String::from(
        r"#[derive(Debug)]
pub struct Module {
    pub name: &'static str,
    pub feature: &'static str,
    pub builtins: &'static [&'static str],
    pub deps: &'static [&'static str],
    pub ambient: bool,
    pub optional: bool,
}

#[rustfmt::skip]
pub const MODULES: &[Module] = &[
",
    );

    for module in modules {
        source.push_str("    Module {\n");
        let _ = writeln!(source, "        name: {:?},", module.name);
        let _ = writeln!(source, "        feature: {:?},", module.feature);
        let _ = writeln!(
            source,
            "        builtins: &{},",
            string_slice(&module.builtins)
        );
        let _ = writeln!(source, "        deps: &{},", string_slice(&module.deps));
        let _ = writeln!(source, "        ambient: {},", module.ambient);
        let _ = writeln!(source, "        optional: {},", module.optional);
        source.push_str("    },\n");
    }

    source.push_str("];\n");
    source
}

fn render_registry(ordered: &[&Module]) -> String {
    // Generated code goes through the same lints. The three suppressions only hold for
    // specific feature sets, so they are `allow` rather than `expect`.
    let mut source = String::from(
        r#"#[rustfmt::skip]
#[allow(
    clippy::vec_init_then_push,
    reason = "one push per enabled capability mirrors the `cfg` list one to one"
)]
#[allow(
    clippy::missing_const_for_fn,
    reason = "the body is only const when every capability is compiled out"
)]
pub(crate) fn extensions() -> Vec<deno_core::Extension> {
    #[allow(unused_mut, reason = "nothing is pushed when every capability is compiled out")]
    let mut extensions = Vec::new();
"#,
    );
    for module in ordered {
        let _ = writeln!(source, "    #[cfg(feature = {:?})]", module.feature);
        let _ = writeln!(
            source,
            "    extensions.push({}::init());",
            module.krate.replace('-', "_")
        );
    }
    source.push_str("    extensions\n}\n\n");

    source.push_str(
        r#"#[rustfmt::skip]
#[allow(
    clippy::vec_init_then_push,
    reason = "one push per enabled capability mirrors the `cfg` list one to one"
)]
#[allow(
    clippy::missing_const_for_fn,
    reason = "the body is only const when every capability is compiled out"
)]
pub(crate) fn enabled() -> Vec<&'static str> {
    #[allow(unused_mut, reason = "nothing is pushed when every capability is compiled out")]
    let mut names = Vec::new();
"#,
    );
    for module in ordered {
        let _ = writeln!(source, "    #[cfg(feature = {:?})]", module.feature);
        let _ = writeln!(source, "    names.push({:?});", module.name);
    }
    source.push_str("    names\n}\n");
    source
}

fn render_manifest(manifest: &str, modules: &[Module]) -> Result<String, String> {
    let begin = manifest
        .find(FEATURES_BEGIN)
        .ok_or_else(|| format!("{RT_MANIFEST} lacks `{FEATURES_BEGIN}`"))?;
    let end = manifest
        .find(FEATURES_END)
        .ok_or_else(|| format!("{RT_MANIFEST} lacks `{FEATURES_END}`"))?;

    let mut default = vec![format!("{:?}", "backend-quickjs")];
    default.extend(modules.iter().map(|module| format!("{:?}", module.feature)));

    let mut block = format!(
        "{FEATURES_BEGIN}\n[features]\ndefault = [{}]\n",
        default.join(", ")
    );
    for feature in BACKEND_FEATURES {
        let target = feature.replace("backend-", "thinpack-core/backend-");
        let _ = writeln!(block, "{feature} = [\"{target}\"]");
    }
    for module in modules {
        let mut entries = vec![format!("\"dep:{}\"", module.krate)];
        entries.extend(
            module
                .deps
                .iter()
                .map(|dep| format!("\"{}\"", feature_of(modules, dep))),
        );
        let _ = writeln!(block, "{} = [{}]", module.feature, entries.join(", "));
    }
    block.push_str(FEATURES_END);

    let mut rendered = String::with_capacity(manifest.len() + block.len());
    rendered.push_str(&manifest[..begin]);
    rendered.push_str(&block);
    rendered.push_str(&manifest[end + FEATURES_END.len()..]);
    Ok(rendered)
}

fn feature_of<'a>(modules: &'a [Module], name: &str) -> &'a str {
    modules
        .iter()
        .find(|module| module.name == name)
        .map(|module| module.feature.as_str())
        .expect("validate() rejects unknown dependencies")
}

fn string_slice(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|value| format!("{value:?}")).collect();
    format!("[{}]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str, deps: &[&str]) -> Module {
        Module::new(
            name.to_owned(),
            Entry {
                builtins: vec![name.to_owned()],
                deps: deps.iter().map(ToString::to_string).collect(),
                ..Entry::default()
            },
        )
    }

    #[test]
    fn accepts_an_acyclic_graph() {
        let modules = [module("buffer", &[]), module("stream", &["buffer"])];
        assert_eq!(dependency_order(&modules).map(|order| order.len()), Ok(2));
    }

    #[test]
    fn rejects_a_cycle() {
        let modules = [module("stream", &["fs"]), module("fs", &["stream"])];
        let error = dependency_order(&modules).expect_err("dependency cycle must be rejected");
        assert!(error.contains("dependency cycle"), "{error}");
    }

    #[test]
    fn manifest_features_pull_in_dependencies() {
        let manifest = "# BEGIN GENERATED FEATURES\n# END GENERATED FEATURES\n";
        let modules = [module("buffer", &[]), module("stream", &["buffer"])];
        let rendered = render_manifest(manifest, &modules).expect("markers exist");

        assert!(rendered.contains("node-stream = [\"dep:thinpack-node-stream\", \"node-buffer\"]"));
        assert!(
            rendered.contains("default = [\"backend-quickjs\", \"node-buffer\", \"node-stream\"]")
        );
    }
}
