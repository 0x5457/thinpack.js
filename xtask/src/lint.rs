use std::env;
use std::process::{Command, ExitCode};

use crate::capabilities;

const USAGE: &str = "Usage: cargo xtask lint";

pub(crate) fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    match check_all() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn check_all() -> Result<(), String> {
    cargo("rustfmt", &["fmt", "--all", "--check"])?;
    println!("rustfmt: ok");

    // The default feature set is the release template: QuickJS backend plus every built-in module.
    cargo(
        "clippy (quickjs)",
        &["clippy", "--workspace", "--all-targets"],
    )?;
    println!("clippy (quickjs): ok");

    // The two backends are mutually exclusive, so V8 needs its own run. Core tests are compiled
    // for V8 as well, because their `cfg` branches depend on the backend.
    cargo(
        "clippy (v8, core)",
        &[
            "clippy",
            "--package",
            "thinpack-core",
            "--all-targets",
            "--no-default-features",
            "--features",
            "backend-v8",
        ],
    )?;
    let features = v8_features()?;
    cargo(
        "clippy (v8, template)",
        &[
            "clippy",
            "--package",
            "thinpack-rt",
            "--all-targets",
            "--no-default-features",
            "--features",
            &features,
        ],
    )?;
    println!("clippy (v8): ok");
    Ok(())
}

/// The default feature set cannot cover the V8 backend (the backends are mutually exclusive), so
/// list every release-template module feature and lint the same code again under V8.
fn v8_features() -> Result<String, String> {
    let modules = capabilities::load()?;
    let mut features = vec!["backend-v8".to_owned()];
    features.extend(modules.iter().map(|module| module.feature.clone()));
    Ok(features.join(","))
}

fn cargo(step: &str, args: &[&str]) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args(args)
        .current_dir(crate::repo_root())
        .status()
        .map_err(|error| format!("{step}: cannot run cargo: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{step}: failed ({status})"))
    }
}
