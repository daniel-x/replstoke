//! replstoke — keep a REPL warm and call it like a one-shot tool.
//!
//! Wrap a long-running REPL process and make it accessible as a one-shot
//! (batch) tool. See `SPEC.md` for the full specification.

use std::ffi::OsString;

pub mod cli;
pub mod client;
pub mod error;
pub mod general;
pub mod marker;
pub mod names;
pub mod platform;
pub mod protocol;
pub mod repl;
pub mod server;
pub mod transport;

pub use cli::config::{Bind, ClientConfig, Config, GeneralAction, ServerConfig};
pub use error::{AppError, AppResult};

/// Parse arguments, dispatch by mode, and return a process exit code.
///
/// All user-facing errors are printed to stderr as a single `error: ...` line.
pub fn run(args: &[OsString]) -> i32 {
    let config = match cli::parse(args) {
        Ok(c) => c,
        Err(e) => return e.report(),
    };

    let result = match config {
        Config::Server(cfg) => server::run(cfg),
        Config::Client(cfg) => client::run(cfg),
        Config::General(action) => general::run(action),
    };

    match result {
        Ok(()) => 0,
        Err(e) => e.report(),
    }
}
