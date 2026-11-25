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
const BACKEND_FEATURES: [&str; 3] = [
    thinpack_pack::Backend::QuickJs.feature(),
    thinpack_pack::Backend::V8.feature(),
    thinpack_pack::Backend::Jsc.feature(),
];

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
    pub globals: Vec<String>,
    pub layers: Vec<String>,
    pub per_isolate: bool,
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
            globals: entry.globals,
            layers: entry.layers,
            per_isolate: entry.per_isolate,
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
    globals: Vec<String>,
    #[serde(default)]
    layers: Vec<String>,
    #[serde(default)]
    per_isolate: bool,
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
    if let Some(name) = crate::llhttp::sync(check)? {
        stale.push(name);
    }

    if stale.is_empty() {
        return Ok(());
    }

    let details = format!(
        "generated files disagree with their source: {}\nrun `cargo xtask gen` to regenerate.",
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
    let mut globals = BTreeMap::new();

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
        for global in &module.globals {
            if let Some(other) = globals.insert(global, &module.name) {
                return Err(format!(
                    "global `{global}` is declared twice: by `{other}` and by `{}`",
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
        for layer in &module.layers {
            if !is_layer_name(layer) {
                return Err(format!(
                    "layer `{layer}` must be a package name starting with `thinpack-`"
                ));
            }
            check_crate_dir(layer)?;
            check_optional_dependency(runtime_manifest, layer)?;
        }
    }

    dependency_order(modules).map(|_| ())
}

fn is_module_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_lowercase = chars.next().is_some_and(|first| first.is_ascii_lowercase());
    starts_lowercase && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_layer_name(name: &str) -> bool {
    name.strip_prefix("thinpack-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
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

/// Checks that `krate` has a crate directory: `crates/node/<name>` for a module crate and
/// `crates/<package>` for anything else.
fn check_crate_dir(krate: &str) -> Result<(), String> {
    let relative = if let Some(name) = krate.strip_prefix("thinpack-node-") {
        format!("crates/node/{name}")
    } else if krate.starts_with("thinpack-") {
        format!("crates/{krate}")
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
    pub globals: &'static [&'static str],
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
        let _ = writeln!(
            source,
            "        globals: &{},",
            string_slice(&module.globals)
        );
        let _ = writeln!(source, "        ambient: {},", module.ambient);
        let _ = writeln!(source, "        optional: {},", module.optional);
        source.push_str("    },\n");
    }

    source.push_str("];\n");
    source
}

fn render_registry(ordered: &[&Module]) -> String {
    // Generated code goes through the same lints. The suppressions only hold for specific
    // feature sets, so they are `allow` rather than `expect`.
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
#[allow(
    unused_variables,
    reason = "only a per-isolate capability reads the inputs"
)]
pub(crate) fn extensions(isolate: &thinpack_core::IsolateInputs) -> Vec<deno_core::Extension> {
    #[allow(unused_mut, reason = "nothing is pushed when every capability is compiled out")]
    let mut extensions = Vec::new();
"#,
    );
    for (layer, users) in layers(ordered) {
        let condition = if let [only] = users.as_slice() {
            format!("#[cfg(feature = {:?})]", only.feature)
        } else {
            let features: Vec<String> = users
                .iter()
                .map(|module| format!("feature = {:?}", module.feature))
                .collect();
            format!("#[cfg(any({}))]", features.join(", "))
        };
        let _ = writeln!(source, "    {condition}");
        let _ = writeln!(
            source,
            "    extensions.push({}::init());",
            layer.replace('-', "_")
        );
    }
    for module in ordered {
        let _ = writeln!(source, "    #[cfg(feature = {:?})]", module.feature);
        let krate = module.krate.replace('-', "_");
        if module.per_isolate {
            let _ = writeln!(source, "    extensions.extend({krate}::init(isolate));");
        } else {
            let _ = writeln!(source, "    extensions.push({krate}::init());");
        }
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

    let all: Vec<String> = modules
        .iter()
        .map(|module| format!("{:?}", module.feature))
        .collect();
    let mut block =
        format!("{FEATURES_BEGIN}\n[features]\ndefault = [\"backend-quickjs\", \"node-all\"]\n");
    for feature in BACKEND_FEATURES {
        let target = feature.replace("backend-", "thinpack-core/backend-");
        let _ = writeln!(block, "{feature} = [\"{target}\"]");
    }
    let _ = writeln!(block, "node-all = [{}]", all.join(", "));
    for module in modules {
        let mut entries = vec![format!("\"dep:{}\"", module.krate)];
        entries.extend(
            module
                .deps
                .iter()
                .map(|dep| format!("\"{}\"", feature_of(modules, dep))),
        );
        entries.extend(module.layers.iter().map(|layer| format!("\"dep:{layer}\"")));
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

/// The shared layer crates in first-appearance order, each with the modules that list it.
fn layers<'a>(ordered: &[&'a Module]) -> Vec<(&'a str, Vec<&'a Module>)> {
    let mut layers: Vec<(&str, Vec<&Module>)> = Vec::new();
    for module in ordered {
        for layer in &module.layers {
            match layers.iter_mut().find(|(name, _)| name == layer) {
                Some((_, users)) => users.push(module),
                None => layers.push((layer, vec![module])),
            }
        }
    }
    layers
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

    fn layered_module(name: &str, layers: &[&str]) -> Module {
        Module::new(
            name.to_owned(),
            Entry {
                builtins: vec![name.to_owned()],
                layers: layers.iter().map(ToString::to_string).collect(),
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
        assert!(rendered.contains("default = [\"backend-quickjs\", \"node-all\"]"));
        assert!(rendered.contains("node-all = [\"node-buffer\", \"node-stream\"]"));
    }

    #[test]
    fn manifest_features_enable_the_layers_a_module_uses() {
        let manifest = "# BEGIN GENERATED FEATURES\n# END GENERATED FEATURES\n";
        let modules = [
            layered_module("child_process", &["thinpack-child"]),
            layered_module("cluster", &["thinpack-child"]),
        ];
        let rendered = render_manifest(manifest, &modules).expect("markers exist");

        assert!(
            rendered
                .contains("node-cluster = [\"dep:thinpack-node-cluster\", \"dep:thinpack-child\"]")
        );
    }

    #[test]
    fn a_per_isolate_module_receives_the_isolate_inputs() {
        let modules = [Module::new(
            "isolate".to_owned(),
            Entry {
                builtins: vec!["isolate".to_owned()],
                per_isolate: true,
                ..Entry::default()
            },
        )];
        let order = dependency_order(&modules).expect("the graph is acyclic");
        let rendered = render_registry(&order);

        assert!(rendered.contains("thinpack_node_isolate::init(isolate)"));
        assert!(!rendered.contains("thinpack_node_isolate::init()"));
    }

    #[test]
    fn a_shared_layer_is_registered_once_before_the_modules_that_use_it() {
        let modules = [
            layered_module("child_process", &["thinpack-child"]),
            layered_module("cluster", &["thinpack-child"]),
        ];
        let order = dependency_order(&modules).expect("the graph is acyclic");
        let rendered = render_registry(&order);

        assert_eq!(rendered.matches("thinpack_child::init()").count(), 1);
        let layer = rendered
            .find("thinpack_child::init()")
            .expect("the layer is registered");
        let module = rendered
            .find("thinpack_node_child_process::init()")
            .expect("the module is registered");
        assert!(layer < module, "a layer is registered before its users");
        assert!(
            rendered.contains(
                "#[cfg(any(feature = \"node-child-process\", feature = \"node-cluster\"))]"
            )
        );
    }
}
