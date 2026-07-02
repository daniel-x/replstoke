//! Typed configuration produced by the CLI parser.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// Default TCP address used for both binding (server) and connecting (client).
pub const DEFAULT_ADDR: &str = "127.0.0.1";
/// Default TCP port.
pub const DEFAULT_PORT: u16 = 44556;
/// Prefix of default / discoverable unix socket files.
pub const SOCKET_PREFIX: &str = ".replstoke_socket_";
/// Prefix of default / discoverable pid files.
pub const PIDFILE_PREFIX: &str = ".replstoke_process_id_";

/// Top-level mode of operation.
#[derive(Debug)]
pub enum Config {
    Server(ServerConfig),
    Client(ClientConfig),
    General(GeneralAction),
}

/// Transport selection, shared by server (listen) and client (connect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    Tcp {
        addr: String,
        port: u16,
    },
    /// `None` => server uses its default path, client discovers a single socket.
    Unix {
        path: Option<PathBuf>,
    },
}

/// How a pidfile path is determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PidfileSpec {
    /// Use the default `./.replstoke_process_id_<cmdname>_pid<pid>` name.
    Default,
    /// Use an explicit path.
    Path(PathBuf),
}

#[derive(Debug)]
pub struct ServerConfig {
    /// argv of the REPL process; `repl_argv[0]` is the executable.
    pub repl_argv: Vec<OsString>,
    pub restart: bool,
    pub bind: Bind,
    pub pidfile: Option<PidfileSpec>,
    /// Disable the framed protocol (plain byte forwarder).
    pub raw: bool,
    /// Success end-of-response marker watched on stdout; empty disables it.
    /// Drives the server's own busy/idle opinion of the REPL.
    pub end_marker_stdout: Vec<u8>,
    /// Success end-of-response marker watched on stderr; empty disables it.
    pub end_marker_stderr: Vec<u8>,
    /// Error end-of-response marker watched on stdout; empty disables it.
    pub error_marker_stdout: Vec<u8>,
    /// Error end-of-response marker watched on stderr; empty disables it.
    pub error_marker_stderr: Vec<u8>,
    /// Strip the matched marker from the stdout stream forwarded to the client.
    pub strip_marker_stdout: bool,
    /// Strip the matched marker from the stderr stream forwarded to the client.
    pub strip_marker_stderr: bool,
    /// Ready marker watched on stdout during the REPL's boot (start/restart)
    /// phase; empty disables it. The REPL is served to clients only once ready.
    pub ready_marker_stdout: Vec<u8>,
    /// Ready marker watched on stderr during the REPL's boot phase; empty
    /// disables it. If both ready markers are set, the earlier match wins.
    pub ready_marker_stderr: Vec<u8>,
    /// Fixed time to wait after (re)spawn before treating the REPL as ready,
    /// instead of a marker. Mutually exclusive with the ready markers.
    pub ready_wait: Option<Duration>,
    /// If the REPL emits no ready marker within this long after (re)spawn, it is
    /// assumed stuck and terminated (restarted when `restart` is set). Requires a
    /// ready marker.
    pub ready_marker_timeout: Option<Duration>,
    /// Input written to the REPL once it has booted, before any client is served
    /// (e.g. to preload libraries). Its output is not forwarded to clients, and no
    /// client is served until it completes. Empty disables it.
    pub warmup: Vec<u8>,
    /// Marker on stdout that signals the warmup finished; empty disables it.
    pub warmup_marker_stdout: Vec<u8>,
    /// Marker on stderr that signals the warmup finished; empty disables it.
    pub warmup_marker_stderr: Vec<u8>,
    /// Fixed time to wait during warmup before treating it as finished, regardless
    /// of markers. The warmup ends at the earliest of a marker match or this wait.
    pub warmup_wait: Option<Duration>,
    /// If no warmup marker is seen within this long, the REPL is assumed stuck and
    /// terminated (restarted when `restart` is set). Requires a warmup marker.
    pub warmup_marker_timeout: Option<Duration>,
    /// After a client disconnects mid-request, how long to wait for the REPL to
    /// finish before terminating (and restarting) it. `None` waits indefinitely.
    pub response_timeout: Option<Duration>,
}

/// Where the client routes the server's `ctl` status messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtlRoute {
    Ignore,
    Stdout,
    Stderr,
}

/// Source of the streamed client input (`-f`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileInput {
    Stdin,
    Path(PathBuf),
}

#[derive(Debug)]
pub struct ClientConfig {
    pub bind: Bind,
    pub arginput: Option<Vec<u8>>,
    pub fileinput: Option<FileInput>,
    pub suffix: Vec<u8>,
    /// Success end-of-response marker watched on stdout; empty disables it.
    pub end_marker_stdout: Vec<u8>,
    /// Success end-of-response marker watched on stderr; empty disables it.
    pub end_marker_stderr: Vec<u8>,
    /// Error end-of-response marker watched on stdout; empty disables it.
    pub error_marker_stdout: Vec<u8>,
    /// Error end-of-response marker watched on stderr; empty disables it.
    pub error_marker_stderr: Vec<u8>,
    /// Strip the matched marker from the stdout stream output.
    pub strip_marker_stdout: bool,
    /// Strip the matched marker from the stderr stream output.
    pub strip_marker_stderr: bool,
    /// Where to route the server's ctl status messages.
    pub ctl: CtlRoute,
    /// Disable the framed protocol (plain byte reader, stderr merged into stdout).
    pub raw: bool,
    /// Give up waiting for a complete response after this long.
    pub timeout: Option<Duration>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GeneralAction {
    Help,
    Version,
    Kill(Option<PathBuf>),
}

/// Platform default success end-of-response marker for stdout.
pub fn default_end_marker_stdout() -> Vec<u8> {
    if cfg!(windows) {
        b"\r\n\r\n".to_vec()
    } else {
        b"\n\n".to_vec()
    }
}

/// Default error end-of-response marker for stderr.
pub fn default_error_marker_stderr() -> Vec<u8> {
    b"error".to_vec()
}
