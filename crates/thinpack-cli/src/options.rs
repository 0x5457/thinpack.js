use std::path::PathBuf;

use thinpack_pack::Backend;

pub(crate) const DEFAULT_BACKEND: Backend = Backend::QuickJs;

#[derive(Default)]
pub(crate) struct Options {
    pub entry: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub template: Option<PathBuf>,
    pub assets: Vec<AssetInput>,
    pub with: Vec<String>,
    pub without: Vec<String>,
    pub backend: Option<Backend>,
    pub source: Option<PathBuf>,
    pub rebuild: bool,
    pub strict: bool,
    pub include: Vec<PathBuf>,
    pub allow_missing: Vec<String>,
    pub conditions: Vec<String>,
    pub list_capabilities: bool,
}

#[derive(Debug)]
pub(crate) struct Usage(pub String);

impl std::fmt::Display for Usage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Usage {}

pub(crate) struct Parsed {
    pub options: Options,
    pub rest: Vec<String>,
    pub help: bool,
}

pub(crate) struct AssetInput {
    pub key: String,
    pub path: PathBuf,
}

pub(crate) fn parse_options(args: Vec<String>, pack: bool) -> Result<Parsed, Usage> {
    let mut options = Options::default();
    let mut rest = Vec::new();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !pack => {
                rest.extend(args);
                break;
            }
            "-o" | "--output" if pack => {
                options.output = Some(PathBuf::from(value(&mut args, &arg)?));
            }
            "--template" => options.template = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--asset" if pack => options.assets.push(parse_asset(&value(&mut args, &arg)?)?),
            "--with" => options
                .with
                .extend(module_list(&value(&mut args, &arg)?, &arg)?),
            "--without" => options
                .without
                .extend(module_list(&value(&mut args, &arg)?, &arg)?),
            "--allow-missing" => options
                .allow_missing
                .extend(module_list(&value(&mut args, &arg)?, &arg)?),
            "--conditions" => options
                .conditions
                .extend(module_list(&value(&mut args, &arg)?, &arg)?),
            "--backend" => {
                let backend = value(&mut args, &arg)?;
                options.backend = Some(Backend::parse(&backend).ok_or_else(|| {
                    Usage(format!(
                        "unknown backend `{backend}`; expected quickjs, v8 or jsc"
                    ))
                })?);
            }
            "--source" => options.source = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--rebuild" => options.rebuild = true,
            "--strict" if pack => options.strict = true,
            "--include" if pack => options.include.push(PathBuf::from(value(&mut args, &arg)?)),
            "--list-capabilities" if pack => options.list_capabilities = true,
            "-h" | "--help" => {
                return Ok(Parsed {
                    options,
                    rest,
                    help: true,
                });
            }
            other if other.starts_with('-') => {
                return Err(Usage(format!("unknown option `{other}`")));
            }
            other if options.entry.is_none() => options.entry = Some(PathBuf::from(other)),
            other => {
                return Err(Usage(format!(
                    "unexpected argument `{other}`; pass application arguments after `--`"
                )));
            }
        }
    }

    if options.entry.is_none() {
        return Err(Usage("missing entry file".to_owned()));
    }
    options.conditions.sort_unstable();
    options.conditions.dedup();
    Ok(Parsed {
        options,
        rest,
        help: false,
    })
}

fn parse_asset(value: &str) -> Result<AssetInput, Usage> {
    let (key, path) = value
        .split_once('=')
        .ok_or_else(|| Usage(format!("`--asset` expects <key>=<path>, got `{value}`")))?;
    if key.is_empty() {
        return Err(Usage("asset key must not be empty".to_owned()));
    }
    Ok(AssetInput {
        key: key.to_owned(),
        path: PathBuf::from(path),
    })
}

fn value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, Usage> {
    args.next()
        .ok_or_else(|| Usage(format!("`{option}` needs a value")))
}

fn module_list(value: &str, option: &str) -> Result<Vec<String>, Usage> {
    value
        .split(',')
        .map(str::trim)
        .map(|module| {
            if module.is_empty() {
                Err(Usage(format!(
                    "`{option}` contains an empty module name in `{value}`"
                )))
            } else {
                Ok(module.to_owned())
            }
        })
        .collect()
}
