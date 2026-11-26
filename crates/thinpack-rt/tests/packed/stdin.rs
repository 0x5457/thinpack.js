use std::io::{self, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::thread;
use std::time::Duration;

use super::super::{Cli, Outcome};
use super::{TempDir, build_packed_payload, run_command};

const SOURCE: &str = r"
process.stdin.read();
const fallback = process.argv[2] ?? '';
let text = '';
process.stdin.setEncoding('utf8');
process.stdin.on('readable', () => {
  const chunk = process.stdin.read();
  if (chunk !== null) text += chunk;
});
process.stdin.on('end', () => {
  console.log(text.length ? text : fallback);
});
";

const SIZED_SOURCE: &str = r"
let total = 0;
process.stdin.setEncoding('utf8');
process.stdin.on('readable', () => {
  process.stdin.read(0);
  const chunk = process.stdin.read(100000);
  if (chunk !== null) {
    if (chunk !== 'a'.repeat(chunk.length)) throw new Error('unexpected stdin text');
    total += chunk.length;
  }
});
process.stdin.on('end', () => {
  console.log(total);
});
";

/// One write above Node's 65,536-byte pipe high-water mark.
static LARGE_INPUT: [u8; 100_000] = [b'a'; 100_000];

/// The positional fallback must not be confused with piped text.
const FALLBACK: &str = "from-argv";

/// How long the harness holds an empty pipe open. A pull read that does not
/// keep the loop alive lets the process exit long before this.
const IDLE_HOLD: Duration = Duration::from_millis(300);

/// How long each write is given to arrive before the next one.
const FRAGMENT_GAP: Duration = Duration::from_millis(30);

pub(super) struct PullTest {
    name: &'static str,
    directory: &'static str,
    source: &'static str,
    /// Written one at a time, so the program reads them separately.
    fragments: &'static [&'static [u8]],
    expected: &'static str,
}

impl PullTest {
    pub(super) const fn name(&self) -> &'static str {
        self.name
    }
}

const TESTS: &[PullTest] = &[
    PullTest {
        name: "packed::stdin-pull-no-data",
        directory: "stdin-pull-no-data",
        source: SOURCE,
        fragments: &[],
        expected: FALLBACK,
    },
    PullTest {
        name: "packed::stdin-pull-utf8-fragments",
        directory: "stdin-pull-utf8-fragments",
        source: SOURCE,
        // `A`, the first byte of `糖`, both bytes of `糖` plus the first two of
        // `🍪`, then the rest and a final unterminated `糖` lead byte. Every
        // read ends inside a character, and EOF has to flush the last one.
        fragments: &[b"A\xe7", b"\xb3\x96\xf0\x9f", b"\x8d\xaaZ\xe7"],
        expected: "A糖🍪Z\u{FFFD}",
    },
    PullTest {
        name: "packed::stdin-pull-above-high-water-mark",
        directory: "stdin-pull-above-high-water-mark",
        source: SIZED_SOURCE,
        fragments: &[&LARGE_INPUT],
        expected: "100000",
    },
];

pub(super) fn selected(cli: &Cli) -> Vec<&'static PullTest> {
    TESTS.iter().filter(|test| cli.matches(test.name)).collect()
}

pub(super) fn print_list(cli: &Cli) {
    for test in selected(cli) {
        println!("{}", test.name);
    }
}

pub(super) fn run_test(test: &'static PullTest) -> Outcome {
    let directory = match TempDir::new(test.directory) {
        Ok(directory) => directory,
        Err(error) => return Outcome::Failed(format!("cannot create test directory: {error}")),
    };
    let files = [("/app/main.js", test.source)];
    let binary = match build_packed_payload("/app/main.js", &files, &[], directory.path()) {
        Ok(binary) => binary,
        Err(error) => {
            return Outcome::Failed(format!("cannot build packed test executable: {error}"));
        }
    };

    let mut command = Command::new(binary);
    command
        .arg(FALLBACK)
        .current_dir(directory.path())
        .stdin(Stdio::piped());
    run_command(
        command,
        None,
        Some(test.expected),
        Some(Box::new(move |input| feed(input, test.fragments))),
    )
}

fn feed(mut stdin: ChildStdin, fragments: &[&[u8]]) -> io::Result<()> {
    for fragment in fragments {
        stdin.write_all(fragment)?;
        thread::sleep(FRAGMENT_GAP);
    }
    if fragments.is_empty() {
        thread::sleep(IDLE_HOLD);
    }
    Ok(())
}
