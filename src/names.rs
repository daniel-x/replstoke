//! Default socket / pidfile names and current-directory discovery (manual
//! prefix-match "globbing", no glob crate).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cli::config::{PIDFILE_PREFIX, SOCKET_PREFIX};
use crate::error::{AppError, AppResult};

/// File name of the REPL executable without its directory, used in default
/// file names. Falls back to `"repl"` if it cannot be determined.
pub fn cmdname(executable: &OsStr) -> String {
    Path::new(executable)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "repl".to_string())
}

/// `./.replstoke_socket_<cmdname>_pid<pid>`
pub fn default_socket_path(cmdname: &str, pid: u32) -> PathBuf {
    PathBuf::from(format!("./{SOCKET_PREFIX}{cmdname}_pid{pid}"))
}

/// `./.replstoke_process_id_<cmdname>_pid<pid>`
pub fn default_pidfile_path(cmdname: &str, pid: u32) -> PathBuf {
    PathBuf::from(format!("./{PIDFILE_PREFIX}{cmdname}_pid{pid}"))
}

/// Discover exactly one file in `dir` whose name starts with `prefix`.
/// `kind` names the thing for error messages.
fn discover_one_in(dir: &Path, prefix: &str, kind: &str) -> AppResult<PathBuf> {
    let mut matches: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| {
        AppError::failure(format!("cannot read directory '{}': {e}", dir.display()))
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(prefix) {
            matches.push(entry.path());
        }
    }
    match matches.len() {
        1 => Ok(matches.pop().unwrap()),
        0 => Err(AppError::failure(format!(
            "no {kind} found matching ./{prefix}*"
        ))),
        n => Err(AppError::failure(format!(
            "found {n} {kind} matching ./{prefix}*; specify one explicitly"
        ))),
    }
}

/// Find a single `./.replstoke_socket_*` for client unix discovery.
pub fn discover_socket() -> AppResult<PathBuf> {
    discover_one_in(Path::new("."), SOCKET_PREFIX, "socket")
}

/// Find a single `./.replstoke_process_id_*` for `--kill` discovery.
pub fn discover_pidfile() -> AppResult<PathBuf> {
    discover_one_in(Path::new("."), PIDFILE_PREFIX, "pidfile")
}

#[cfg(test)]
mod tests;
