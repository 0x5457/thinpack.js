mod bundle;

mod capabilities;

mod commands;

mod options;

mod templates;

const USAGE: &str = "\
Usage: thinpack pack <entry.js> [-o <output>] [--template <thinpack-rt>] [--rebuild]
                      [--backend <quickjs|v8|jsc>] [--source <checkout>]
                      [--asset <key>=<path>]... [--with <module[,module...]>]...
                      [--without <module[,module...]>]... [--allow-missing <package[,package...]>]...\n                      [--include <path>]... [--strict] [--list-capabilities]
       thinpack run <entry.js> [--template <thinpack-rt>] [--rebuild]
                     [--backend <quickjs|v8|jsc>] [--source <checkout>]
                     [--with <module[,module...]>]... [--without <module[,module...]>]... [-- <args>...]
       thinpack templates
       thinpack capabilities
       thinpack --version

Packages the entry's module graph into a single executable based on a runtime
template. Templates come from `--template`, or from the cache
($THINPACK_CACHE/templates, deduplicated by version, backend and capability set);
`--rebuild` builds one with cargo. `thinpack run` executes an entry with a matching
template without packing. `jsc` is the macOS system JavaScriptCore framework and
has no bundled engine. Assets are exposed through node:sea;
`--list-capabilities` prints the derived capability set and exits. `--include` packs
additional files (data, declarations) and analyzes them when they are JavaScript;
its paths are relative to the project root (the entry's nearest package.json), and
it is how files loaded only by computed code reach the payload.";

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return std::process::ExitCode::from(2);
    };

    let result = match command.as_str() {
        "pack" => commands::pack_command(args.collect()).map(|()| std::process::ExitCode::SUCCESS),
        "run" => commands::run_command(args.collect())
            .map(|code| std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))),
        "templates" => commands::templates_command().map(|()| std::process::ExitCode::SUCCESS),
        "capabilities" => {
            for module in thinpack_caps::MODULES {
                println!("{}", module.name);
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
        "-V" | "--version" => {
            println!("thinpack {}", env!("CARGO_PKG_VERSION"));
            Ok(std::process::ExitCode::SUCCESS)
        }
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(std::process::ExitCode::SUCCESS)
        }
        other => {
            eprintln!("unknown command `{other}`\n{USAGE}");
            Ok(std::process::ExitCode::from(2))
        }
    };

    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            if error.downcast_ref::<options::Usage>().is_some() {
                std::process::ExitCode::from(2)
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}
