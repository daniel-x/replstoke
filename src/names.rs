//! Default socket / pidfile names and current-directory discovery (manual
//! prefix-match "globbing", no glob crate).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cli::config::{NAME_PREFIX, PIDFILE_SUFFIX, SOCKET_SUFFIX};
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

/// `./.replstoke_pid<pid>_<cmdname>_socket`
pub fn default_socket_path(cmdname: &str, pid: u32) -> PathBuf {
    PathBuf::from(format!("./{NAME_PREFIX}{pid}_{cmdname}{SOCKET_SUFFIX}"))
}

/// `./.replstoke_pid<pid>_<cmdname>_pidfile`
pub fn default_pidfile_path(cmdname: &str, pid: u32) -> PathBuf {
    PathBuf::from(format!("./{NAME_PREFIX}{pid}_{cmdname}{PIDFILE_SUFFIX}"))
}

/// Discover exactly one file in `dir` whose name starts with `prefix` and ends
/// with `suffix` (socket and pidfile names share a prefix and differ only by
/// suffix). `kind` names the thing for error messages.
fn discover_one_in(dir: &Path, prefix: &str, suffix: &str, kind: &str) -> AppResult<PathBuf> {
    let mut matches: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| {
        AppError::failure(format!("cannot read directory '{}': {e}", dir.display()))
    })?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) && name.ends_with(suffix) {
            matches.push(entry.path());
        }
    }
    match matches.len() {
        1 => Ok(matches.pop().unwrap()),
        0 => Err(AppError::failure(format!(
            "no {kind} found matching ./{prefix}*{suffix}"
        ))),
        n => Err(AppError::failure(format!(
            "found {n} {kind} matching ./{prefix}*{suffix}; specify one explicitly"
        ))),
    }
}

/// Find a single `./.replstoke_pid*_socket` for client unix discovery.
pub fn discover_socket() -> AppResult<PathBuf> {
    discover_one_in(Path::new("."), NAME_PREFIX, SOCKET_SUFFIX, "socket")
}

/// Find a single `./.replstoke_pid*_pidfile` for `--kill` discovery.
pub fn discover_pidfile() -> AppResult<PathBuf> {
    discover_one_in(Path::new("."), NAME_PREFIX, PIDFILE_SUFFIX, "pidfile")
}

#[cfg(test)]
mod tests;
