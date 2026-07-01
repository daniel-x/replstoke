//! Uniform error type. Every user-facing failure is a single line prefixed
//! with `error:` on stderr, together with a non-zero exit code.

use std::fmt;

/// Exit code used for invalid command line usage.
pub const EXIT_USAGE: i32 = 2;
/// Exit code used for runtime failures (connect, spawn, kill, marker missing).
pub const EXIT_FAILURE: i32 = 1;
/// Exit code used when the client times out waiting for a response.
pub const EXIT_TIMEOUT: i32 = 124;

#[derive(Debug)]
pub struct AppError {
    pub message: String,
    pub code: i32,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn usage(message: impl Into<String>) -> Self {
        AppError {
            message: message.into(),
            code: EXIT_USAGE,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        AppError {
            message: message.into(),
            code: EXIT_FAILURE,
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        AppError {
            message: message.into(),
            code: EXIT_TIMEOUT,
        }
    }

    /// Print the error to stderr and return the exit code.
    pub fn report(&self) -> i32 {
        eprintln!("error: {}", self.message);
        self.code
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}
