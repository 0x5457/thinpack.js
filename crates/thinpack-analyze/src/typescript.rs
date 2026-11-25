//! Type stripping for packed TypeScript.
//!
//! Node runs `.ts`/`.mts`/`.cts` through SWC's strip-only pass (the engine behind
//! `amaro`), which erases type syntax in place instead of regenerating the source.
//! Packing uses the same pass, so the payload carries JavaScript that keeps its
//! line breaks and therefore its `Function#toString` output. The pass and its
//! diagnostics are Apache-2.0; the runtime never links a TypeScript parser.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use swc_common::Span;
use swc_common::errors::{DiagnosticBuilder, Emitter, HANDLER, Handler};
use swc_common::source_map::SourceMap;
use swc_common::sync::Lrc;
use swc_ecma_parser::TsSyntax;
use swc_ts_fast_strip::{ErrorCode, Mode, Options, operate};

use crate::Error;

/// Strips the types from a TypeScript module and records the JavaScript packing
/// stores in `stripped`; a file that is already JavaScript comes back unchanged.
///
/// Node's rules apply: only erasable syntax is accepted, so the transform-only
/// constructs are rejected here rather than becoming broken JavaScript in the
/// payload. `.tsx` is not supported at all, and TypeScript under `node_modules` is
/// refused the way Node refuses it.
pub(super) fn strip(
    path: &Path,
    source: String,
    stripped: &mut BTreeMap<PathBuf, String>,
) -> Result<String, Error> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    if matches!(extension, Some("ts" | "mts" | "cts" | "tsx"))
        && thinpack_resolve::is_under_node_modules(path)
    {
        return Err(Error::Unsupported {
            path: path.to_path_buf(),
            message: "TypeScript under node_modules is not type-stripped (ERR_UNSUPPORTED_NODE_MODULES_TYPE_STRIPPING); ship the dependency as JavaScript".to_owned(),
        });
    }
    match extension {
        Some("ts" | "mts" | "cts") => {}
        Some("tsx") => {
            return Err(Error::Unsupported {
                path: path.to_path_buf(),
                message: "TypeScript JSX (.tsx) is not supported; compile it to JavaScript before packing"
                    .to_owned(),
            });
        }
        _ => return Ok(source),
    }

    let source_map = Lrc::new(SourceMap::default());
    let diagnostics = Diagnostics::default();
    let seen = diagnostics.clone();
    let handler = Handler::with_emitter(true, false, Box::new(diagnostics));
    let options = Options {
        filename: Some(path.display().to_string()),
        // Node's `amaro` passes the same parser defaults: legacy decorators are
        // valid TypeScript, and `.tsx` stays off.
        parser: TsSyntax {
            decorators: true,
            ..TsSyntax::default()
        },
        mode: Mode::StripOnly,
        ..Options::default()
    };
    let result = HANDLER.set(&handler, || operate(&source_map, &handler, source, options));
    match result {
        Ok(output) => {
            stripped.insert(path.to_path_buf(), output.code.clone());
            Ok(output.code)
        }
        Err(error) => {
            let diagnostics = seen
                .0
                .lock()
                .expect("the diagnostic buffer is never poisoned");
            let message = if diagnostics.is_empty() {
                error.to_string()
            } else {
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.render(&source_map))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            Err(if matches!(error.code, ErrorCode::UnsupportedSyntax) {
                Error::Unsupported {
                    path: path.to_path_buf(),
                    message,
                }
            } else {
                Error::Parse {
                    path: path.to_path_buf(),
                    message,
                }
            })
        }
    }
}

#[derive(Clone, Debug)]
struct Diagnostic {
    span: Option<Span>,
    message: String,
}

impl Diagnostic {
    fn render(&self, source_map: &SourceMap) -> String {
        let Some(span) = self.span else {
            return self.message.clone();
        };
        let location = source_map.lookup_char_pos(span.lo());
        format!(
            "{}:{}: {}",
            location.line,
            location.col_display + 1,
            self.message
        )
    }
}

/// SWC emits through a sink; keep the messages so packing can report the
/// position of the construct it refuses.
#[derive(Clone, Default)]
struct Diagnostics(Arc<Mutex<Vec<Diagnostic>>>);

impl Emitter for Diagnostics {
    fn emit(&mut self, builder: &mut DiagnosticBuilder<'_>) {
        let message = builder
            .message
            .iter()
            .map(|message| message.0.as_str())
            .collect::<String>();
        self.0
            .lock()
            .expect("the diagnostic buffer is never poisoned")
            .push(Diagnostic {
                span: builder.span.primary_span(),
                message,
            });
    }
}
