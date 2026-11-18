#![cfg_attr(
    test,
    allow(
        unused_crate_dependencies,
        reason = "the dev-dependencies belong to the node integration test target"
    )
)]

use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use thinpack_core::{Assets, BACKEND};
use thinpack_loader::{Main, ModuleFs};
use thinpack_pack::Pack;

mod bootstrap;

mod cache;

mod registry;

mod runtime;

const USAGE: &str = "\
Usage: thinpack-rt <script.js>
       thinpack-rt -e <javascript>
       thinpack-rt --capabilities
       thinpack-rt --template-info
       thinpack-rt --warm-cache";

enum Command {
    Run(Main),
    WarmCache,
    Capabilities,
    TemplateInfo,
    Help,
    Version,
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let packed = match packed_payload() {
        Ok(packed) => packed,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(pack) = &packed
        && args.first().and_then(|argument| argument.to_str()) == Some("--warm-cache")
    {
        return match runtime::warm(pack) {
            Ok(compiled) => {
                println!("warmed {compiled} modules");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(pack) = &packed {
        return runtime::run_packed(pack);
    }
    if args.is_empty() {
        eprintln!("missing script\n{USAGE}");
        return ExitCode::from(2);
    }

    let command = match parse(args.into_iter()) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match command {
        Command::Run(main) => {
            // Host paths from argv become absolute; packed entries are virtual
            // `/app/...` paths and must keep their root on Windows too.
            let main = match main {
                Main::File(path) => match std::path::absolute(path) {
                    Ok(path) => Main::File(path),
                    Err(error) => {
                        eprintln!("{error}");
                        return ExitCode::FAILURE;
                    }
                },
                Main::Eval(source) => Main::Eval(source),
            };
            match runtime::run(main, ModuleFs::Host, Assets::host(), None) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::WarmCache => {
            eprintln!("--warm-cache works on a packed executable");
            ExitCode::from(2)
        }
        Command::Capabilities => {
            print_capabilities();
            ExitCode::SUCCESS
        }
        Command::TemplateInfo => {
            print_template_info();
            ExitCode::SUCCESS
        }
        Command::Help => {
            let _ = writeln!(std::io::stdout(), "{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            let _ = writeln!(
                std::io::stdout(),
                "thinpack-rt {} ({})",
                env!("CARGO_PKG_VERSION"),
                BACKEND.engine()
            );
            ExitCode::SUCCESS
        }
    }
}

fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Command, String> {
    let Some(first) = args.next() else {
        return Err("missing script".to_owned());
    };

    match first.to_str() {
        Some("-h" | "--help") => Ok(Command::Help),
        Some("-v" | "--version") => Ok(Command::Version),
        Some("--capabilities") => Ok(Command::Capabilities),
        Some("--warm-cache") => Ok(Command::WarmCache),
        Some("--template-info") => Ok(Command::TemplateInfo),
        Some("-e" | "--eval") => {
            let code = args
                .next()
                .ok_or_else(|| "missing argument for --eval".to_owned())?;
            Ok(Command::Run(Main::Eval(
                code.to_string_lossy().into_owned(),
            )))
        }
        _ => Ok(Command::Run(Main::File(PathBuf::from(first)))),
    }
}

fn packed_payload() -> Result<Option<Pack>, Box<dyn std::error::Error>> {
    Ok(thinpack_pack::read_from_executable(&env::current_exe()?)?)
}

fn print_template_info() {
    let hash = thinpack_caps::capabilities_hash(&registry::enabled());
    println!("version {}", env!("CARGO_PKG_VERSION"));
    println!("backend {BACKEND}");
    println!("capabilities-hash {hash:#018x}");
}

fn print_capabilities() {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if writeln!(out, "backend {BACKEND}").is_err() {
        return;
    }
    for name in registry::enabled() {
        if writeln!(out, "module {name}").is_err() {
            return;
        }
        let module =
            thinpack_caps::module(name).expect("registry names come from the capability table");
        for builtin in module.builtins {
            if writeln!(out, "builtin {builtin}").is_err() {
                return;
            }
        }
    }
}
