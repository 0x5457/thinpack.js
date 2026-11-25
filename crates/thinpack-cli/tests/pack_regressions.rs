#![allow(
    unused_crate_dependencies,
    reason = "these integration cases invoke the CLI binary and inspect packs; other dependencies belong to the CLI implementation"
)]

#[cfg(unix)]
mod unix {
    use std::fmt::Write as _;
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

    // A concurrent fork can keep a fixture executable's writer open until exec,
    // causing ETXTBSY when another test tries to run that executable.
    static EXECUTABLE_FIXTURES: Mutex<()> = Mutex::new(());

    #[test]
    fn cjs_top_level_return_can_be_packed() {
        let directory = TempDir::new("cjs-top-level-return");
        let entry = directory.path().join("main.cjs");
        let output = directory.path().join("packed-app");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let cli = Path::new(env!("CARGO_BIN_EXE_thinpack"));
        let source = "return;\nmodule.exports = 'after return';\n";
        fs::write(&entry, source).expect("write CJS entry");

        let mut command = Command::new(cli);
        command
            .current_dir(directory.path())
            .env("THINPACK_CACHE", directory.path().join("cache"))
            .args(["pack"])
            .arg(&entry)
            .arg("--template")
            .arg(&template)
            .arg("--output")
            .arg(&output);
        let result = run_with_timeout(&mut command);
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert_eq!(pack.manifest.entry, "/app/main.cjs");
        let entry = pack
            .manifest
            .files
            .iter()
            .find(|file| file.path == "/app/main.cjs")
            .expect("packed CJS entry");
        assert_eq!(pack.slice_of(entry), Some(source.as_bytes()));
    }

    #[test]
    fn esm_syntax_in_an_explicit_commonjs_entry_fails_during_packing() {
        let directory = TempDir::new("esm-syntax-in-commonjs");
        let entry = directory.path().join("main.js");
        fs::write(
            directory.path().join("package.json"),
            r#"{ "name": "esm-bundle", "type": "commonjs" }"#,
        )
        .expect("write an explicit CommonJS package manifest");
        fs::write(
            &entry,
            "import { createRequire } from 'node:module';\n\
             const require = createRequire(import.meta.url);\n",
        )
        .expect("write ESM entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities"],
        );
        assert!(
            !result.status.success(),
            "packed a CommonJS entry that uses ES module syntax"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("main.js: uses ES module syntax but is loaded as CommonJS"),
            "{error}"
        );
        assert!(error.contains("rename it to .mjs"), "{error}");
        assert!(error.contains("add \"type\": \"module\""), "{error}");
        assert!(
            error.contains("Packing does not run loader hooks"),
            "{error}"
        );
    }

    #[test]
    fn a_format_assigned_by_a_loader_hook_is_reported_at_packing() {
        let directory = TempDir::new("hook-assigned-format");
        fs::write(
            directory.path().join("package.json"),
            r#"{ "name": "hook-format", "type": "commonjs" }"#,
        )
        .expect("write an explicit CommonJS package manifest");
        let entry = directory.path().join("main.mjs");
        fs::write(
            &entry,
            "import Module from 'node:module';\n\
             Module.register('./hook.mjs', import.meta.url);\n\
             const { answer } = await import('./data.js');\n\
             console.log(answer);\n",
        )
        .expect("write the entry registering a hook");
        fs::write(
            directory.path().join("hook.mjs"),
            "export async function load(url, context, nextLoad) {\n\
             \x20 if (url.endsWith('/data.js')) return nextLoad(url, { ...context, format: 'module' });\n\
             \x20 return nextLoad(url, context);\n\
             }\n",
        )
        .expect("write the loader hook");
        fs::write(
            directory.path().join("data.js"),
            "import { sep } from 'node:path';\nexport const answer = sep;\n",
        )
        .expect("write the ES module the hook assigns a format to");

        // Node runs this: the hook assigns `module` before the default loader reads
        // data.js. Packing reads the on-disk format, so it fails and says so.
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities"],
        );
        assert!(!result.status.success(), "packed a hook-assigned format");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("data.js: uses ES module syntax but is loaded as CommonJS"),
            "{error}"
        );
        assert!(
            error.contains("Packing does not run loader hooks"),
            "{error}"
        );
    }

    #[test]
    fn a_js_entry_in_a_module_package_analyzes_as_esm() {
        let directory = TempDir::new("esm-entry-in-module-package");
        let entry = directory.path().join("main.js");
        fs::write(
            directory.path().join("package.json"),
            r#"{ "type": "module" }"#,
        )
        .expect("write package manifest");
        fs::write(
            &entry,
            "import { sep } from 'node:path';\nconsole.log(sep);\n",
        )
        .expect("write ESM entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities"],
        );
        assert!(
            result.status.success(),
            "rejected a JS entry in a module package:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let listed = String::from_utf8_lossy(&result.stdout);
        assert!(
            listed.lines().any(|line| line.starts_with("path ")),
            "{listed}"
        );
    }

    #[test]
    fn a_dependency_resolved_above_the_project_root_fails_packing() {
        let directory = TempDir::new("hoisted-dependency");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("create project directory");
        fs::write(project.join("package.json"), r#"{ "name": "app" }"#)
            .expect("write project manifest");
        let entry = project.join("main.js");
        fs::write(&entry, "require('hoisted');\n").expect("write entry");
        let dependency = directory.path().join("node_modules/hoisted");
        fs::create_dir_all(&dependency).expect("create hoisted dependency");
        fs::write(dependency.join("index.js"), "module.exports = 1;\n")
            .expect("write hoisted module");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            !result.status.success(),
            "packed a dependency the artifact cannot resolve"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("resolves outside the project root"),
            "{error}"
        );
        assert!(error.contains("hoisted/index.js"), "{error}");
        assert!(
            !output.exists(),
            "pack wrote an executable that cannot load its dependency"
        );
    }

    #[test]
    fn a_guarded_dependency_above_the_project_root_is_left_out() {
        let directory = TempDir::new("guarded-hoisted-dependency");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("create project directory");
        fs::write(project.join("package.json"), r#"{ "name": "app" }"#)
            .expect("write project manifest");
        let entry = project.join("main.js");
        fs::write(&entry, "try { require('hoisted'); } catch {}\n").expect("write entry");
        let dependency = directory.path().join("node_modules/hoisted");
        fs::create_dir_all(&dependency).expect("create hoisted dependency");
        fs::write(dependency.join("index.js"), "module.exports = 1;\n")
            .expect("write hoisted module");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("optional module resolves outside the project root"),
            "{error}"
        );
        assert!(error.contains("hoisted/index.js"), "{error}");

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            !pack
                .manifest
                .files
                .iter()
                .any(|file| file.path.contains("hoisted")),
            "packed an unreachable optional module: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_optional_dependency_above_the_project_root_is_left_out() {
        let directory = TempDir::new("optional-hoisted-dependency");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("create project directory");
        fs::write(
            project.join("package.json"),
            r#"{ "name": "app", "optionalDependencies": { "hoisted": "1" } }"#,
        )
        .expect("write project manifest");
        let entry = project.join("main.js");
        fs::write(&entry, "require('hoisted');\n").expect("write entry");
        let dependency = directory.path().join("node_modules/hoisted");
        fs::create_dir_all(&dependency).expect("create hoisted dependency");
        fs::write(dependency.join("index.js"), "module.exports = 1;\n")
            .expect("write hoisted module");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("optional module resolves outside the project root"),
            "{error}"
        );
        assert!(error.contains("hoisted/index.js"), "{error}");

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            !pack
                .manifest
                .files
                .iter()
                .any(|file| file.path.contains("hoisted")),
            "packed an unreachable optional module: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_typeless_js_file_with_esm_syntax_is_packed_as_esm() {
        let directory = TempDir::new("detect-module");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "export const answer = 42;\nconsole.log(answer);\n")
            .expect("write ESM entry without a package type");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("without \"type\""), "{error}");

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        let format = pack
            .manifest
            .formats
            .iter()
            .find(|record| record.path == "/app/main.js")
            .expect("the detected format is recorded");
        assert_eq!(format.format, thinpack_resolve::Format::Esm);
    }

    #[test]
    fn typescript_is_stripped_into_the_payload() {
        let directory = TempDir::new("typescript");
        let entry = directory.path().join("main.ts");
        fs::write(
            directory.path().join("package.json"),
            r#"{ "type": "module" }"#,
        )
        .expect("write package manifest");
        fs::write(
            &entry,
            "import { greet } from './helper.ts';\n\
             const name: string = 'world';\n\
             console.log(greet(name));\n",
        )
        .expect("write TypeScript entry");
        fs::write(
            directory.path().join("helper.ts"),
            "export function greet(name: string): string {\n\
             \x20 return `hello ${name}`;\n\
             }\n",
        )
        .expect("write TypeScript helper");
        fs::write(
            directory.path().join("types.d.ts"),
            "declare const answer: number;\n",
        )
        .expect("write declaration file");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
                "--include",
                "types.d.ts",
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        let contents = |path: &str| {
            let file = pack
                .manifest
                .files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("{path} is packed"));
            std::str::from_utf8(pack.slice_of(file).expect("the file has contents"))
                .expect("packed JavaScript is UTF-8")
        };

        assert_eq!(pack.manifest.entry, "/app/main.ts");
        assert!(!contents("/app/main.ts").contains(": string"));
        assert!(contents("/app/main.ts").contains("greet(name)"));
        assert!(!contents("/app/helper.ts").contains(": string"));
        assert!(contents("/app/helper.ts").contains("return `hello ${name}`;"));
        // Declaration files are data the app may read, not modules: keep them
        // byte for byte even though their last extension is TypeScript.
        assert_eq!(
            contents("/app/types.d.ts"),
            "declare const answer: number;\n"
        );
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/package.json"),
            "the package scope is packed"
        );
    }

    #[test]
    fn typescript_transform_only_syntax_fails_packing() {
        let directory = TempDir::new("typescript-enum");
        let entry = directory.path().join("main.ts");
        fs::write(&entry, "enum Mode { A }\nconsole.log(Mode.A);\n").expect("write TS entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities"],
        );
        assert!(!result.status.success(), "packed a TypeScript enum");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("main.ts: 1:1: TypeScript enum is not supported in strip-only mode"),
            "{error}"
        );
    }

    #[test]
    fn an_allowed_missing_package_packs_with_a_warning() {
        let directory = TempDir::new("allow-missing");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "require('spdy');\n").expect("write entry requiring a missing package");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");

        let blocked = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(!blocked.status.success(), "packed an unresolved package");

        let allowed = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
                "--allow-missing",
                "spdy",
            ],
        );
        assert!(
            allowed.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&allowed.stderr)
        );
        let error = String::from_utf8_lossy(&allowed.stderr);
        assert!(error.contains("excluded by --allow-missing"), "{error}");

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            !pack
                .manifest
                .files
                .iter()
                .any(|file| file.path.contains("spdy")),
            "packed an excluded package"
        );
    }

    #[test]
    fn a_dirname_relative_asset_is_packed() {
        let directory = TempDir::new("static-asset");
        let entry = directory.path().join("main.js");
        fs::write(
            &entry,
            "const fs = require('node:fs');\nconst path = require('node:path');\nfs.readFileSync(path.join(__dirname, 'data.css'), 'utf8');\n",
        )
        .expect("write entry reading a sibling asset");
        fs::write(directory.path().join("data.css"), "body {}\n").expect("write the asset");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/data.css"),
            "asset was not packed: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn custom_conditions_select_the_export_target() {
        let directory = TempDir::new("custom-conditions");
        let project = directory.path().join("project");
        fs::create_dir_all(project.join("node_modules/pkg")).expect("create package");
        fs::write(project.join("package.json"), r#"{ "name": "app" }"#)
            .expect("write project manifest");
        let entry = project.join("main.js");
        fs::write(&entry, "require('pkg');\n").expect("write entry");
        fs::write(
            project.join("node_modules/pkg/package.json"),
            r#"{ "exports": { ".": { "custom": "./custom.js", "default": "./default.js" } } }"#,
        )
        .expect("write package exports");
        fs::write(
            project.join("node_modules/pkg/custom.js"),
            "module.exports = 'custom';\n",
        )
        .expect("write custom target");
        fs::write(
            project.join("node_modules/pkg/default.js"),
            "module.exports = 'default';\n",
        )
        .expect("write default target");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
                "--conditions",
                "custom",
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert_eq!(pack.manifest.conditions, vec!["custom".to_owned()]);
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/node_modules/pkg/custom.js"),
            "{:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
        assert!(
            !pack
                .manifest
                .files
                .iter()
                .any(|file| file.path == "/app/node_modules/pkg/default.js"),
            "packed the default condition"
        );
    }

    #[test]
    fn a_symlinked_package_manifest_is_packed() {
        let directory = TempDir::new("symlinked-package-manifest");
        let project = directory.path().join("project");
        let package = directory.path().join("packages/linked");
        fs::create_dir_all(&project).expect("create project directory");
        fs::create_dir_all(&package).expect("create linked package");
        fs::write(project.join("package.json"), r#"{ "name": "app" }"#)
            .expect("write project manifest");
        let entry = project.join("main.mjs");
        fs::write(&entry, "import 'linked';\n").expect("write ESM entry");
        fs::write(package.join("package.json"), r#"{ "type": "module" }"#)
            .expect("write linked manifest");
        fs::write(package.join("index.js"), "export const value = 1;\n")
            .expect("write linked module");
        fs::create_dir(project.join("node_modules")).expect("create node_modules");
        std::os::unix::fs::symlink(&package, project.join("node_modules/linked"))
            .expect("link the package into node_modules");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/node_modules/linked/package.json"),
            "missing linked manifest: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_directory_manifest_with_an_escaping_main_is_packed() {
        let directory = TempDir::new("escaping-directory-main");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "require('./lib');\n").expect("write entry");
        fs::create_dir(directory.path().join("lib")).expect("create lib");
        fs::write(
            directory.path().join("lib/package.json"),
            r#"{ "main": "../shared/entry.js" }"#,
        )
        .expect("write directory manifest");
        fs::create_dir(directory.path().join("shared")).expect("create shared");
        fs::write(
            directory.path().join("shared/entry.js"),
            "module.exports = 1;\n",
        )
        .expect("write main module");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/lib/package.json"),
            "missing directory manifest: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_nested_target_manifest_is_packed() {
        let directory = TempDir::new("nested-target-manifest");
        let entry = directory.path().join("main.mjs");
        fs::write(&entry, "import './lib/target/x.js';\n").expect("write ESM entry");
        let nested = directory.path().join("lib/target");
        fs::create_dir_all(&nested).expect("create nested target directory");
        fs::write(nested.join("package.json"), r#"{ "type": "module" }"#)
            .expect("write nested manifest");
        fs::write(nested.join("x.js"), "export const x = 1;\n").expect("write ESM module");

        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(
            result.status.success(),
            "pack failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );

        let pack = thinpack_pack::read_from_executable(&output)
            .expect("read packed executable")
            .expect("packed payload");
        assert!(
            pack.manifest
                .files
                .iter()
                .any(|file| file.path == "/app/lib/target/package.json"),
            "missing nested manifest: {:?}",
            pack.manifest
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn finite_conditional_require_is_complete_in_strict_mode() {
        let directory = TempDir::new("finite-conditional-require");
        let entry = directory.path().join("main.js");
        fs::write(
            &entry,
            r"
const loadBuiltin = require;
const networkModule = 'node:dns';
module.exports = function loadSystemModule(networkEnabled) {
  var selected = networkEnabled ? networkModule : 'node:os';
  return loadBuiltin(selected);
};
",
        )
        .expect("write conditional require entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities", "--strict"],
        );
        assert!(
            result.status.success(),
            "strict capability listing failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let listed = String::from_utf8_lossy(&result.stdout);
        assert!(
            listed.lines().any(|line| line.starts_with("dns ")),
            "{listed}"
        );
        assert!(
            listed.lines().any(|line| line.starts_with("os ")),
            "{listed}"
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("non-literal argument"),
            "unexpected warning: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn truly_dynamic_require_remains_a_strict_error() {
        let directory = TempDir::new("dynamic-require-strict");
        let entry = directory.path().join("main.js");
        fs::write(
            &entry,
            "const selected = process.argv[2];\nrequire(selected);\n",
        )
        .expect("write dynamic require entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &["--list-capabilities", "--strict"],
        );
        assert!(
            !result.status.success(),
            "dynamic require passed strict mode"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("non-literal argument cannot be analyzed"),
            "{error}"
        );
        assert!(
            error.contains("--strict turns this warning into an error"),
            "{error}"
        );
    }

    #[test]
    fn comma_delimited_with_accumulates_capabilities() {
        let directory = TempDir::new("comma-with");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--with",
                "zlib,os",
                "--with",
                "vm,worker_threads",
                "--with",
                "async_hooks",
            ],
        );
        assert!(
            result.status.success(),
            "capability listing failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let listed = String::from_utf8_lossy(&result.stdout);
        for module in ["zlib", "os", "vm", "worker_threads", "async_hooks"] {
            assert!(
                listed
                    .lines()
                    .any(|line| line.starts_with(&format!("{module} "))),
                "missing {module}:\n{listed}"
            );
        }
    }

    #[test]
    fn comma_delimited_overrides_report_unknown_and_empty_names() {
        let directory = TempDir::new("comma-override-diagnostics");
        let entry = directory.path().join("main.js");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");
        let cli = Path::new(env!("CARGO_BIN_EXE_thinpack"));

        let unknown_with = run_pack(
            cli,
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--with",
                "dns,unknown_with_module",
                "--with",
                "os",
            ],
        );
        assert!(!unknown_with.status.success());
        let error = String::from_utf8_lossy(&unknown_with.stderr);
        assert!(
            error.contains("unknown module") && error.contains("unknown_with_module"),
            "{error}"
        );
        assert!(!error.contains("dns,unknown_with_module"), "{error}");
        assert!(error.contains("thinpack capabilities"), "{error}");

        let unknown_without = run_pack(
            cli,
            directory.path(),
            &entry,
            &[
                "--list-capabilities",
                "--without",
                "unknown_without_module,dns",
                "--without",
                "os,ffi",
            ],
        );
        assert!(!unknown_without.status.success());
        let error = String::from_utf8_lossy(&unknown_without.stderr);
        assert!(
            error.contains("unknown module") && error.contains("unknown_without_module"),
            "{error}"
        );
        assert!(!error.contains("unknown_without_module,dns"), "{error}");
        assert!(error.contains("thinpack capabilities"), "{error}");

        for (option, value) in [("--with", "dns,,os"), ("--without", ",dns")] {
            let result = run_pack(
                cli,
                directory.path(),
                &entry,
                &["--list-capabilities", option, value],
            );
            assert!(
                !result.status.success(),
                "accepted empty item in {option} {value}"
            );
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(error.contains(option), "{error}");
            assert!(error.contains("empty module name"), "{error}");
            assert!(error.contains(value), "{error}");
        }
    }

    #[test]
    fn capabilities_command_lists_the_cli_catalog() {
        let directory = TempDir::new("capabilities-catalog");
        let result = run_cli(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &["capabilities"],
        );
        assert!(result.status.success());
        let listed = String::from_utf8_lossy(&result.stdout);
        let names: Vec<_> = listed.lines().collect();
        let expected: Vec<_> = thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn explicit_template_backend_must_match_request() {
        let directory = TempDir::new("explicit-template-backend");
        let entry = directory.path().join("main.js");
        let template = fake_template(
            &directory.path().join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
        );
        let output = directory.path().join("packed-app");
        fs::write(&entry, "module.exports = 1;\n").expect("write entry");

        let result = run_pack(
            Path::new(env!("CARGO_BIN_EXE_thinpack")),
            directory.path(),
            &entry,
            &[
                "--template",
                template.to_str().expect("template path is UTF-8"),
                "--backend",
                "v8",
                "--output",
                output.to_str().expect("output path is UTF-8"),
            ],
        );
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("uses the quickjs backend, but --backend v8 was requested"),
            "{error}"
        );
        assert!(!output.exists(), "wrote a pack from a mismatched template");
    }

    #[test]
    fn cache_does_not_select_a_template_from_another_version() {
        let directory = TempDir::new("stale-template-version");
        // Avoid the CLI's sibling-runtime fallback so only the cache is tested.
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let stale = cache
            .join("templates")
            .join("0.0.0")
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&stale).expect("create stale template cache entry");
        fs::write(&entry, "module.exports = 1;\n").expect("write app entry");
        // A full capability list ensures the candidate reaches version matching.
        fake_template(&stale.join("thinpack-rt"), "0.0.0");
        let capabilities = thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(stale.join("capabilities"), format!("{capabilities}\n"))
            .expect("write cached capabilities");

        let mut command = Command::new(cli);
        command
            .current_dir(directory.path())
            .env("THINPACK_CACHE", &cache)
            .args(["pack"])
            .arg(&entry)
            .arg("--output")
            .arg(&output);
        let result = run_with_timeout(&mut command);
        assert!(!result.status.success(), "stale template was selected");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("no quickjs template covers these capabilities"),
            "unexpected error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from a stale cache entry"
        );
    }

    #[test]
    fn cache_skips_same_version_runtime_without_template_api_marker() {
        let directory = TempDir::new("legacy-template-api");
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let stale = cache
            .join("templates")
            .join(env!("CARGO_PKG_VERSION"))
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&stale).expect("create same-version legacy cache entry");
        fs::write(&entry, "module.exports = 1;\n").expect("write app entry");
        let capabilities = all_capability_names();
        fake_template_with(
            &stale.join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
            "quickjs",
            &capabilities,
            false,
        );
        write_cached_capabilities(&stale, &capabilities);

        let result = run_pack(
            &cli,
            directory.path(),
            &entry,
            &["--output", output.to_str().expect("output path is UTF-8")],
        );
        assert!(!result.status.success(), "legacy runtime was selected");
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("no quickjs template covers these capabilities"),
            "unexpected error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from a legacy runtime"
        );
    }

    #[test]
    fn cache_metadata_cannot_hide_missing_runtime_capabilities() {
        let directory = TempDir::new("cache-actual-capabilities");
        let cli = copy_cli(&directory);
        let entry = directory.path().join("main.js");
        let output = directory.path().join("packed-app");
        let cache = directory.path().join("cache");
        let cached = cache
            .join("templates")
            .join(env!("CARGO_PKG_VERSION"))
            .join("quickjs")
            .join("0000000000000000");
        fs::create_dir_all(&cached).expect("create cache entry");
        fs::write(&entry, "require('node:crypto');\n").expect("write crypto entry");

        let all = all_capability_names();
        let actual: Vec<_> = all
            .iter()
            .copied()
            .filter(|name| *name != "crypto")
            .collect();
        fake_template_with(
            &cached.join("thinpack-rt"),
            env!("CARGO_PKG_VERSION"),
            "quickjs",
            &actual,
            true,
        );
        write_cached_capabilities(&cached, &all);

        let result = run_pack(
            &cli,
            directory.path(),
            &entry,
            &["--output", output.to_str().expect("output path is UTF-8")],
        );
        assert!(
            !result.status.success(),
            "insufficient cached runtime was selected"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("no quickjs template covers these capabilities"),
            "{error}"
        );
        assert!(
            !output.exists(),
            "pack wrote an executable from an insufficient runtime"
        );
    }

    #[test]
    fn sibling_template_requires_current_version_and_capabilities() {
        for (name, version, missing_crypto) in [
            ("sibling-stale-version", "0.0.0", false),
            (
                "sibling-missing-capability",
                env!("CARGO_PKG_VERSION"),
                true,
            ),
        ] {
            let directory = TempDir::new(name);
            let cli = copy_cli(&directory);
            let entry = directory.path().join("main.js");
            let output = directory.path().join("packed-app");
            fs::write(&entry, "require('node:crypto');\n").expect("write crypto entry");

            let all = all_capability_names();
            let capabilities: Vec<_> = all
                .iter()
                .copied()
                .filter(|capability| !missing_crypto || *capability != "crypto")
                .collect();
            fake_template_with(
                &directory.path().join("thinpack-rt"),
                version,
                "quickjs",
                &capabilities,
                true,
            );

            let result = run_pack(
                &cli,
                directory.path(),
                &entry,
                &["--output", output.to_str().expect("output path is UTF-8")],
            );
            assert!(
                !result.status.success(),
                "invalid sibling template was selected"
            );
            let error = String::from_utf8_lossy(&result.stderr);
            assert!(
                error.contains("no quickjs template covers these capabilities"),
                "{error}"
            );
            assert!(
                !output.exists(),
                "pack wrote an executable from an invalid sibling"
            );
        }
    }

    fn fake_template(path: &Path, version: &str) -> PathBuf {
        let capabilities = all_capability_names();
        fake_template_with(path, version, "quickjs", &capabilities, true)
    }

    fn fake_template_with(
        path: &Path,
        version: &str,
        backend: &str,
        capabilities: &[&str],
        template_api: bool,
    ) -> PathBuf {
        let mut module_output = String::new();
        for capability in capabilities {
            writeln!(module_output, "    printf 'module {capability}\\n'")
                .expect("writing to a String cannot fail");
        }
        let template_api = if template_api {
            "template-api 1\\n"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n  --template-info)\n    printf 'version {version}\\nbackend {backend}\\n{template_api}capabilities-hash 0x0000000000000000\\n'\n    ;;\n  --capabilities)\n{module_output}    ;;\n  *) exit 2 ;;\nesac\n",
        );
        let _guard = EXECUTABLE_FIXTURES
            .lock()
            .expect("lock executable fixtures");
        fs::write(path, script).expect("write fake runtime template");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make fake runtime template executable");
        path.to_path_buf()
    }

    fn all_capability_names() -> Vec<&'static str> {
        thinpack_caps::MODULES
            .iter()
            .map(|module| module.name)
            .collect()
    }

    fn write_cached_capabilities(directory: &Path, capabilities: &[&str]) {
        fs::write(
            directory.join("capabilities"),
            capabilities.join("\n") + "\n",
        )
        .expect("write cached capabilities");
    }

    fn run_pack(cli: &Path, directory: &Path, entry: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(cli);
        command
            .current_dir(directory)
            .env("THINPACK_CACHE", directory.join("cache"))
            .args(["pack"])
            .arg(entry)
            .args(args);
        run_with_timeout(&mut command)
    }

    fn run_cli(cli: &Path, directory: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(cli);
        command.current_dir(directory).args(args);
        run_with_timeout(&mut command)
    }

    fn copy_cli(directory: &TempDir) -> PathBuf {
        let source = Path::new(env!("CARGO_BIN_EXE_thinpack"));
        let destination = directory.path().join("thinpack");
        let _guard = EXECUTABLE_FIXTURES
            .lock()
            .expect("lock executable fixtures");
        fs::copy(source, &destination).expect("copy thinpack CLI into isolated directory");
        fs::set_permissions(
            &destination,
            fs::metadata(source).expect("CLI metadata").permissions(),
        )
        .expect("preserve CLI executable permissions");
        destination
    }

    fn run_with_timeout(command: &mut Command) -> Output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = {
            let _guard = EXECUTABLE_FIXTURES
                .lock()
                .expect("lock executable fixtures");
            command.spawn().expect("spawn thinpack CLI")
        };
        let stdout = child.stdout.take().expect("capture CLI stdout");
        let stderr = child.stderr.take().expect("capture CLI stderr");
        let stdout = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stdout = stdout;
            stdout.read_to_end(&mut bytes).expect("read CLI stdout");
            bytes
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stderr = stderr;
            stderr.read_to_end(&mut bytes).expect("read CLI stderr");
            bytes
        });

        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for thinpack CLI") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = join_output(stdout, "stdout");
                let stderr = join_output(stderr, "stderr");
                panic!(
                    "thinpack CLI timed out after {} ms\nstdout: {}\nstderr: {}",
                    COMMAND_TIMEOUT.as_millis(),
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr),
                );
            }
            thread::sleep(Duration::from_millis(10));
        };

        Output {
            status,
            stdout: join_output(stdout, "stdout"),
            stderr: join_output(stderr, "stderr"),
        }
    }

    fn join_output(reader: thread::JoinHandle<Vec<u8>>, stream: &str) -> Vec<u8> {
        reader
            .join()
            .unwrap_or_else(|_| panic!("{stream} reader panicked"))
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "thinpack-cli-{name}-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create temporary test directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
