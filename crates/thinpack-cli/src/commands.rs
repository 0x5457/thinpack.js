use std::error::Error;
use std::process::ExitStatus;

use crate::bundle;
use crate::capabilities::{analyze_entry, print_capabilities};
use crate::options::parse_options;
use crate::templates::{self, resolve_template};

pub(crate) fn pack_command(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let parsed = parse_options(args, true)?;
    if parsed.help {
        println!("{}", crate::USAGE);
        return Ok(());
    }
    let options = parsed.options;
    let analyzed = analyze_entry(&options, true)?;

    if options.list_capabilities {
        print_capabilities(&analyzed.analysis, &analyzed.capabilities, &analyzed.root);
        return Ok(());
    }

    let template = resolve_template(&options, &analyzed.capabilities)?;
    let bundle = bundle::build(
        &analyzed.root,
        &analyzed.entry,
        &analyzed.capabilities,
        &template.info,
        &analyzed.analysis,
        &options.assets,
    )?;
    let output = options
        .output
        .unwrap_or_else(|| bundle::default_output(&analyzed.entry));
    let (payload_len, total) =
        bundle::write(&template.path, &bundle.manifest, &bundle.blob, &output)?;

    println!(
        "packed {} files, {} capabilities ({} bytes payload, {} bytes total) -> {}",
        bundle.manifest.files.len(),
        bundle.manifest.capabilities.len(),
        payload_len,
        total,
        output.display()
    );
    Ok(())
}

pub(crate) fn run_command(args: Vec<String>) -> Result<i32, Box<dyn Error>> {
    let parsed = parse_options(args, false)?;
    if parsed.help {
        println!("{}", crate::USAGE);
        return Ok(0);
    }
    let options = parsed.options;
    let analyzed = analyze_entry(&options, false)?;
    let template = resolve_template(&options, &analyzed.capabilities)?;
    let status = std::process::Command::new(&template.path)
        .arg(&analyzed.entry)
        .args(&parsed.rest)
        .status()
        .map_err(|error| format!("{}: {error}", template.path.display()))?;
    Ok(exit_code(status))
}

/// Shells report a process killed by a signal as `128 + signal`.
#[cfg(unix)]
fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Windows termination has no signal component to report.
#[cfg(not(unix))]
fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

pub(crate) fn templates_command() -> Result<(), Box<dyn Error>> {
    let entries = templates::Cache::from_env()?.entries()?;
    if entries.is_empty() {
        println!(
            "no templates cached yet; `thinpack pack --rebuild` or `thinpack run --rebuild` builds one"
        );
        return Ok(());
    }
    println!("{:<10} {:<8} {:>9}  path", "version", "backend", "modules");
    for entry in entries {
        println!(
            "{:<10} {:<8} {:>9}  {}",
            entry.version,
            entry.backend,
            entry.capabilities,
            entry.path.display()
        );
    }
    Ok(())
}
