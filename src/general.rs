//! General mode: `--help`, `--version`, and `--kill`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::config::GeneralAction;
use crate::cli::help;
use crate::error::{AppError, AppResult};
use crate::names;
use crate::platform;

/// Grace period before a server is force-killed.
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(action: GeneralAction) -> AppResult<()> {
    match action {
        GeneralAction::Help => {
            print!("{}", help::HELP);
            Ok(())
        }
        GeneralAction::Version => {
            println!("{}", help::VERSION);
            Ok(())
        }
        GeneralAction::Kill(pidfile) => kill(pidfile),
    }
}

fn kill(pidfile: Option<PathBuf>) -> AppResult<()> {
    let path = match pidfile {
        Some(p) => p,
        None => names::discover_pidfile()?,
    };

    let pid = read_pid(&path)?;

    let gone = platform::terminate_pid(pid, KILL_TIMEOUT);

    // If the process is gone and the pidfile is still around, remove it.
    if gone && path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    if gone {
        Ok(())
    } else {
        Err(AppError::failure(format!(
            "could not terminate server with pid {pid}"
        )))
    }
}

fn read_pid(path: &Path) -> AppResult<u32> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::failure(format!("cannot read pidfile '{}': {e}", path.display())))?;
    content.trim().parse::<u32>().map_err(|_| {
        AppError::failure(format!(
            "pidfile '{}' does not contain a valid pid",
            path.display()
        ))
    })
}
