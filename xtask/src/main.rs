mod apps;

mod build;

mod capabilities;

mod dist;

mod lint;

mod llhttp;

mod measure;

mod size;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
Usage: cargo xtask gen [--check]
       cargo xtask lint
       cargo xtask size [--backend <quickjs|v8|jsc>] [--profile <release|small|all>]
                        [--modules <all|core|name,...>]
       cargo xtask dist [--backend <quickjs|v8|jsc>] [--target <triple>] [--out <dir>]
       cargo xtask apps [--only <name,...>] [--baseline <path>] [--write-baseline]
                        [--summary <path>]";

/// The workspace root: the parent directory of the `xtask` package.
///
/// # Panics
///
/// Panics when `xtask` is not nested in a workspace directory.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is a workspace member")
        .to_path_buf()
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest: Vec<String> = args.collect();

    match command.as_str() {
        "gen" => capabilities::run(&rest),
        "lint" => lint::run(&rest),
        "size" => size::run(&rest),
        "dist" => dist::run(&rest),
        "apps" => apps::run(&rest),
        _ => {
            eprintln!("unknown command `{command}`\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
