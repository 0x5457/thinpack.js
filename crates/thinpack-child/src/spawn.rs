use std::fmt;
use std::io::Write as _;
use std::process::Stdio as ProcessStdio;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use crate::ChildError;
use crate::table;

/// How one standard stream of a spawned child is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Piped,
    Inherit,
    Ignore,
}

impl Stream {
    fn parse(word: Option<&str>) -> Result<Self, ChildError> {
        match word {
            Some("pipe") => Ok(Self::Piped),
            Some("inherit") => Ok(Self::Inherit),
            Some("ignore") => Ok(Self::Ignore),
            Some(other) => Err(ChildError::new(
                "ERR_INVALID_ARG_VALUE",
                format!("unknown stdio {other:?}"),
            )),
            None => Err(ChildError::new(
                "ERR_INVALID_ARG_VALUE",
                "stdio must name three streams",
            )),
        }
    }

    /// The value `std::process::Command` accepts; tokio's command reuses it.
    fn process(self) -> ProcessStdio {
        match self {
            Self::Piped => ProcessStdio::piped(),
            Self::Inherit => ProcessStdio::inherit(),
            Self::Ignore => ProcessStdio::null(),
        }
    }
}

/// How a child's three standard streams are connected.
///
/// A cluster worker picks one policy for all three (`silent: true` pipes, the default
/// inherits); `node:child_process` picks one per stream from its `stdio` option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stdio {
    stdin: Stream,
    stdout: Stream,
    stderr: Stream,
}

impl Stdio {
    /// Every stream piped, the default of both modules.
    pub const PIPED: Self = Self::all(Stream::Piped);
    /// Every stream inherited from the parent.
    pub const INHERITED: Self = Self::all(Stream::Inherit);

    const fn all(stream: Stream) -> Self {
        Self {
            stdin: stream,
            stdout: stream,
            stderr: stream,
        }
    }

    /// Parses the three `\0`-joined `pipe`/`inherit`/`ignore` words the modules send.
    ///
    /// # Errors
    ///
    /// Returns `ERR_INVALID_ARG_VALUE` unless exactly three known words are present.
    pub fn parse(spec: &str) -> Result<Self, ChildError> {
        let mut words = spec.split('\0');
        let stdio = Self {
            stdin: Stream::parse(words.next())?,
            stdout: Stream::parse(words.next())?,
            stderr: Stream::parse(words.next())?,
        };
        if words.next().is_some() {
            return Err(ChildError::new(
                "ERR_INVALID_ARG_VALUE",
                "stdio names more than three streams",
            ));
        }
        Ok(stdio)
    }
}

/// One spawn request: the program, its arguments, environment and working directory.
///
/// The values arrive from JavaScript `\0`-joined, which cannot be ambiguous: neither a
/// path nor an `KEY=value` environment entry can contain a `\0`.
#[derive(Debug)]
pub struct Spawn {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
}

impl Spawn {
    /// Splits the `\0`-joined fields the modules send over the wire.
    ///
    /// # Errors
    ///
    /// Returns `ERR_INVALID_ARG_VALUE` when the program name is empty.
    pub fn parse(argv: &str, env: &str, cwd: &str) -> Result<Self, ChildError> {
        let mut parts = argv.split('\0');
        let program = parts.next().unwrap_or_default();
        if program.is_empty() {
            return Err(ChildError::new("ERR_INVALID_ARG_VALUE", "empty command"));
        }
        Ok(Self {
            program: program.to_owned(),
            args: parts.map(str::to_owned).collect(),
            env: env
                .split('\0')
                .filter_map(|entry| entry.split_once('='))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
            cwd: (!cwd.is_empty()).then(|| cwd.to_owned()),
        })
    }

    /// Adds an entry the module supplies on top of the inherited environment, late enough to
    /// win over a same-named entry inherited from the parent.
    #[must_use]
    pub fn with_env(mut self, key: &str, value: impl fmt::Display) -> Self {
        self.env.push((key.to_owned(), value.to_string()));
        self
    }

    /// The command both spawn paths start from.
    ///
    /// An empty environment inherits the parent's; a supplied one replaces it, matching the
    /// `env` option both modules implement.
    fn command(&self, stdio: Stdio) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(stdio.stdin.process())
            .stdout(stdio.stdout.process())
            .stderr(stdio.stderr.process());
        if !self.env.is_empty() {
            command
                .env_clear()
                .envs(self.env.iter().map(|(key, value)| (key, value)));
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }
}

/// A spawned process and the table entry that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spawned {
    pub id: u32,
    pub pid: u32,
}

/// A descriptor the spawned child keeps open across `exec`.
///
/// Unix moves `source` to `target` in the child, which is how a cluster worker is handed the
/// parent end of its IPC channel. Windows has no such inheritance — a worker's channel is a
/// loopback socket it connects to itself — so the layer never receives one there.
#[derive(Clone, Copy, Debug)]
pub struct InheritedFd {
    /// The descriptor in this process.
    pub source: i32,
    /// The descriptor number it has to have in the child.
    pub target: i32,
}

/// Makes the child keep `fd` open, as `fd.target`, across `exec`.
#[cfg(unix)]
fn inherit_fd(command: &mut std::process::Command, fd: Option<InheritedFd>) {
    let Some(InheritedFd { source, target }) = fd else {
        return;
    };
    // SAFETY: the hook runs in the child between fork and exec, where `dup2` and `fcntl` are
    // async-signal-safe and only touch descriptors, which is what `pre_exec` requires.
    unsafe {
        command.pre_exec(move || {
            if source == target {
                // Already in place: only the close-on-exec flag stands in the way.
                if libc::fcntl(target, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(source, target) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Windows has no descriptor inheritance to set up: a cluster worker's channel is a loopback
/// socket it connects to itself.
#[cfg(not(unix))]
const fn inherit_fd(_command: &mut std::process::Command, _fd: Option<InheritedFd>) {}

/// Spawns the child and registers it in the table.
///
/// # Errors
///
/// Returns the spawn failure as the OS reported it.
pub fn spawn(
    request: &Spawn,
    stdio: Stdio,
    inherit: Option<InheritedFd>,
) -> Result<Spawned, ChildError> {
    let mut command = request.command(stdio);
    inherit_fd(&mut command, inherit);
    let mut command = tokio::process::Command::from(command);
    // The table owns the child, not the op call that spawned it: dropping the future must not
    // kill a process the program is still waiting on.
    command.kill_on_drop(false);
    let child = command.spawn().map_err(ChildError::spawn)?;
    table::register(child)
}

/// A child that already exited, with everything it wrote.
#[derive(Debug)]
pub struct Completed {
    pub exited: Exited,
    pub pid: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs a child to completion with the requested stream connections, writing `input` to
/// its stdin when stdin is piped.
///
/// The synchronous half of the layer: it drives `wait_with_output` itself instead of going
/// through the table, and shares only the request parsing with [`spawn`]. The captured
/// outputs stay empty for streams that are inherited or ignored.
///
/// # Errors
///
/// Returns the spawn failure, a failed stdin write, or a failed wait.
///
/// # Panics
///
/// Panics if the stdin writer thread panics, which it does not: it only writes to the pipe.
pub fn run_to_completion(
    request: &Spawn,
    stdio: Stdio,
    input: Vec<u8>,
) -> Result<Completed, ChildError> {
    let mut child = request.command(stdio).spawn().map_err(ChildError::spawn)?;
    let pid = child.id();
    let input_pipe = child.stdin.take();
    // Writing stdin in-line would deadlock with a child that fills an output pipe while the
    // parent is still writing, so the input is written from a side thread while the parent
    // drains the pipes. Dropping the handle inside the thread closes the pipe and gives the
    // child EOF even when the write fails.
    let writer =
        std::thread::spawn(move || input_pipe.map_or(Ok(()), |mut pipe| pipe.write_all(&input)));
    let output = child
        .wait_with_output()
        .map_err(|error| ChildError::io("ECHILD", error))?;
    let written = writer.join().expect("the stdin writer thread cannot panic");
    if let Err(error) = written {
        return Err(ChildError::io("EPIPE", error));
    }
    Ok(Completed {
        exited: Exited::new(output.status),
        pid,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// The exit status of a reaped child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exited {
    pub code: Option<i32>,
    pub signal: i32,
}

impl Exited {
    #[must_use]
    pub fn new(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
            signal: exit_signal(status),
        }
    }

    /// The payload `op_child_wait` returns to JavaScript.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{}\0{}", self.code.unwrap_or(-1), self.signal)
    }
}

/// The signal number that terminated the child, or 0 for a plain exit.
#[cfg(unix)]
fn exit_signal(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().unwrap_or(0)
}

#[cfg(windows)]
const fn exit_signal(_status: std::process::ExitStatus) -> i32 {
    // Windows processes always terminate through their exit code; `TerminateProcess`
    // surfaces as a code, never as a signal number.
    0
}
