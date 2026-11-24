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

    // The three backends are mutually exclusive, so each needs its own run. Core tests are
    // compiled for every backend as well, because their `cfg` branches depend on it.
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
    let features = backend_features("backend-v8")?;
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

    // JSC is the macOS system framework; no other host can compile it.
    if cfg!(target_os = "macos") {
        cargo(
            "clippy (jsc, core)",
            &[
                "clippy",
                "--package",
                "thinpack-core",
                "--all-targets",
                "--no-default-features",
                "--features",
                "backend-jsc",
            ],
        )?;
        let features = backend_features("backend-jsc")?;
        cargo(
            "clippy (jsc, template)",
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
        println!("clippy (jsc): ok");
    } else {
        println!("clippy (jsc): skipped (macOS-only backend)");
    }
    Ok(())
}

/// The default feature set cannot cover a non-default backend (the backends are mutually
/// exclusive), so list every release-template module feature and lint the same code again.
fn backend_features(backend: &str) -> Result<String, String> {
    let modules = capabilities::load()?;
    let mut features = vec![backend.to_owned()];
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
