//! Platform facade. A uniform API (`configure_command`, `after_spawn`,
//! `ReplGuard`, shutdown signals, `terminate_pid`, `is_alive`) with a
//! `#[cfg]`-selected implementation backed by hand-written FFI.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
