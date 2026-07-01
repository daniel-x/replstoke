//! Spawn and own the REPL child process with piped stdio.

use std::ffi::OsString;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

use crate::error::{AppError, AppResult};
use crate::platform::{self, ReplGuard};

pub struct Repl {
    pub child: Child,
    pub guard: ReplGuard,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// Spawn the REPL. `argv[0]` is the executable (resolved via PATH); the rest are
/// its arguments. Fails fast if the process cannot be started.
pub fn spawn(argv: &[OsString]) -> AppResult<Repl> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    platform::configure_command(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| {
        AppError::failure(format!(
            "cannot start REPL '{}': {e}",
            argv[0].to_string_lossy()
        ))
    })?;

    let guard = platform::after_spawn(&child);
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    Ok(Repl {
        child,
        guard,
        stdin,
        stdout,
        stderr,
    })
}
