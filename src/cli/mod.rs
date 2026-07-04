//! Command line parsing: a hand-written tokenizer feeding a typed [`Config`].
//!
//! Pipeline: `tokenize(argv) -> Raw flags -> determine mode -> validate`.

pub mod config;
pub mod help;

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use crate::error::{AppError, AppResult};
use config::{
    default_end_marker_stdout, default_error_marker_stderr, Bind, ClientConfig, Config, CtlRoute,
    FileInput, GeneralAction, PidfileSpec, ServerConfig, DEFAULT_ADDR, DEFAULT_PORT,
};

/// Accumulator filled by the tokenizer; converted to [`Config`] afterwards.
///
/// Optional-value options use `Option<Option<T>>`: outer `None` = not given,
/// `Some(None)` = given without a value, `Some(Some(v))` = given with a value.
#[derive(Default)]
struct Raw {
    server: bool,
    client: bool,
    restart: bool,
    strip_marker_stdout: bool,
    strip_marker_stderr: bool,
    raw: bool,
    help: bool,
    version: bool,
    addr: Option<Option<String>>,
    port: Option<u16>,
    unixsocket: Option<Option<OsString>>,
    pidfile: Option<Option<OsString>>,
    kill: Option<Option<OsString>>,
    arginput: Option<Vec<u8>>,
    fileinput: Option<OsString>,
    suffix: Option<Vec<u8>>,
    end_marker_stdout: Option<Vec<u8>>,
    end_marker_stderr: Option<Vec<u8>>,
    error_marker_stdout: Option<Vec<u8>>,
    error_marker_stderr: Option<Vec<u8>>,
    ready_marker_stdout: Option<Vec<u8>>,
    ready_marker_stderr: Option<Vec<u8>>,
    ready_wait: Option<Duration>,
    ready_marker_timeout: Option<Duration>,
    warmup: Option<Vec<u8>>,
    warmup_marker_stdout: Option<Vec<u8>>,
    warmup_marker_stderr: Option<Vec<u8>>,
    warmup_wait: Option<Duration>,
    warmup_marker_timeout: Option<Duration>,
    ctl: Option<String>,
    timeout: Option<Duration>,
    response_timeout: Option<Duration>,
    exec: Option<Vec<OsString>>,
}

/// Parse the program arguments (without argv[0]) into a [`Config`].
pub fn parse(args: &[OsString]) -> AppResult<Config> {
    let raw = tokenize(args)?;
    build_config(raw)
}

// ---- byte conversion helpers -------------------------------------------------

#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

#[cfg(unix)]
fn os_from_bytes(b: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(b)
}

#[cfg(not(unix))]
fn os_from_bytes(b: Vec<u8>) -> OsString {
    OsString::from(String::from_utf8_lossy(&b).into_owned())
}

// ---- tokenizer ---------------------------------------------------------------

fn tokenize(args: &[OsString]) -> AppResult<Raw> {
    let mut raw = Raw::default();
    let mut i = 0;
    while i < args.len() {
        let bytes = os_bytes(&args[i]);
        if bytes == b"--" {
            raw.exec = Some(args[i + 1..].to_vec());
            break;
        }
        if bytes.starts_with(b"--") {
            let body = &bytes[2..];
            let (name, value) = match body.iter().position(|&c| c == b'=') {
                Some(eq) => (&body[..eq], Some(body[eq + 1..].to_vec())),
                None => (body, None),
            };
            let name = String::from_utf8_lossy(name).into_owned();
            i += 1;
            if handle_long(&mut raw, &name, value, args, &mut i)? {
                break;
            }
        } else if bytes.len() > 1 && bytes[0] == b'-' {
            i += 1;
            if handle_short(&mut raw, &bytes[1..], args, &mut i)? {
                break;
            }
        } else {
            let arg = args[i].to_string_lossy().into_owned();
            let prev = if i > 0 {
                Some(os_bytes(&args[i - 1]))
            } else {
                None
            };
            return Err(unexpected_argument(&arg, prev.as_deref()));
        }
    }
    Ok(raw)
}

fn handle_long(
    raw: &mut Raw,
    name: &str,
    value: Option<Vec<u8>>,
    args: &[OsString],
    i: &mut usize,
) -> AppResult<bool> {
    let no_value = |value: &Option<Vec<u8>>| -> AppResult<()> {
        if value.is_some() {
            Err(AppError::usage(format!(
                "option '--{name}' does not take a value"
            )))
        } else {
            Ok(())
        }
    };
    let required = |value: Option<Vec<u8>>, i: &mut usize| -> AppResult<Vec<u8>> {
        if let Some(v) = value {
            return Ok(v);
        }
        if *i < args.len() {
            let v = os_bytes(&args[*i]);
            *i += 1;
            Ok(v)
        } else {
            Err(AppError::usage(format!(
                "option '--{name}' requires an argument"
            )))
        }
    };

    match name {
        "server" => {
            no_value(&value)?;
            raw.server = true;
        }
        "client" => {
            no_value(&value)?;
            raw.client = true;
        }
        "restart" => {
            no_value(&value)?;
            raw.restart = true;
        }
        "strip-marker-stdout" => {
            no_value(&value)?;
            raw.strip_marker_stdout = true;
        }
        "strip-marker-stderr" => {
            no_value(&value)?;
            raw.strip_marker_stderr = true;
        }
        "raw" => {
            no_value(&value)?;
            raw.raw = true;
        }
        "help" => {
            no_value(&value)?;
            raw.help = true;
        }
        "version" => {
            no_value(&value)?;
            raw.version = true;
        }
        "addr" => raw.addr = Some(value.map(bytes_to_string)),
        "unixsocket" => raw.unixsocket = Some(value.map(os_from_bytes)),
        "pidfile" => raw.pidfile = Some(value.map(os_from_bytes)),
        "kill" => raw.kill = Some(value.map(os_from_bytes)),
        "port" => raw.port = Some(parse_port(&required(value, i)?)?),
        "arginput" => raw.arginput = Some(required(value, i)?),
        "suffix" => raw.suffix = Some(required(value, i)?),
        "end-marker-stdout" => raw.end_marker_stdout = Some(required(value, i)?),
        "end-marker-stderr" => raw.end_marker_stderr = Some(required(value, i)?),
        "error-marker-stdout" => raw.error_marker_stdout = Some(required(value, i)?),
        "error-marker-stderr" => raw.error_marker_stderr = Some(required(value, i)?),
        "ready-marker-stdout" => raw.ready_marker_stdout = Some(required(value, i)?),
        "ready-marker-stderr" => raw.ready_marker_stderr = Some(required(value, i)?),
        "ready-wait" => raw.ready_wait = Some(parse_timeout(&required(value, i)?)?),
        "ready-marker-timeout" => {
            raw.ready_marker_timeout = Some(parse_timeout(&required(value, i)?)?)
        }
        "warmup-input" => raw.warmup = Some(required(value, i)?),
        "warmup-marker-stdout" => raw.warmup_marker_stdout = Some(required(value, i)?),
        "warmup-marker-stderr" => raw.warmup_marker_stderr = Some(required(value, i)?),
        "warmup-wait" => raw.warmup_wait = Some(parse_timeout(&required(value, i)?)?),
        "warmup-marker-timeout" => {
            raw.warmup_marker_timeout = Some(parse_timeout(&required(value, i)?)?)
        }
        "ctl" => raw.ctl = Some(bytes_to_string(required(value, i)?)),
        "timeout" => raw.timeout = Some(parse_timeout(&required(value, i)?)?),
        "response-timeout" => raw.response_timeout = Some(parse_timeout(&required(value, i)?)?),
        "fileinput" => raw.fileinput = Some(os_from_bytes(required(value, i)?)),
        "exec" => {
            let mut argv = Vec::new();
            if let Some(v) = value {
                argv.push(os_from_bytes(v));
            }
            argv.extend_from_slice(&args[*i..]);
            *i = args.len();
            raw.exec = Some(argv);
            return Ok(true);
        }
        _ => return Err(AppError::usage(format!("unknown option '--{name}'"))),
    }
    Ok(false)
}

fn handle_short(raw: &mut Raw, chars: &[u8], args: &[OsString], i: &mut usize) -> AppResult<bool> {
    let mut j = 0;
    while j < chars.len() {
        let c = chars[j];
        // value for required/optional options = rest of this token after `c`.
        let attached = &chars[j + 1..];
        let take_required = |i: &mut usize| -> AppResult<Vec<u8>> {
            if !attached.is_empty() {
                return Ok(attached.to_vec());
            }
            if *i < args.len() {
                let v = os_bytes(&args[*i]);
                *i += 1;
                Ok(v)
            } else {
                Err(AppError::usage(format!(
                    "option '-{}' requires an argument",
                    c as char
                )))
            }
        };
        let optional = || -> Option<Vec<u8>> {
            if attached.is_empty() {
                None
            } else {
                Some(attached.to_vec())
            }
        };

        match c {
            b's' => raw.server = true,
            b'c' => raw.client = true,
            b'r' => raw.restart = true,
            b'h' => raw.help = true,
            b'a' => {
                raw.addr = Some(optional().map(bytes_to_string));
                return Ok(false);
            }
            b'u' => {
                raw.unixsocket = Some(optional().map(os_from_bytes));
                return Ok(false);
            }
            b'd' => {
                raw.pidfile = Some(optional().map(os_from_bytes));
                return Ok(false);
            }
            b'k' => {
                raw.kill = Some(optional().map(os_from_bytes));
                return Ok(false);
            }
            b'p' => {
                raw.port = Some(parse_port(&take_required(i)?)?);
                return Ok(false);
            }
            b'i' => {
                raw.arginput = Some(take_required(i)?);
                return Ok(false);
            }
            b'f' => {
                raw.fileinput = Some(os_from_bytes(take_required(i)?));
                return Ok(false);
            }
            b'm' => {
                raw.end_marker_stdout = Some(take_required(i)?);
                return Ok(false);
            }
            b'x' => {
                raw.suffix = Some(take_required(i)?);
                return Ok(false);
            }
            b'e' => {
                let mut argv = Vec::new();
                if !attached.is_empty() {
                    argv.push(os_from_bytes(attached.to_vec()));
                }
                argv.extend_from_slice(&args[*i..]);
                *i = args.len();
                raw.exec = Some(argv);
                return Ok(true);
            }
            _ => return Err(AppError::usage(format!("unknown option '-{}'", c as char))),
        }
        j += 1;
    }
    Ok(false)
}

fn bytes_to_string(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).into_owned()
}

/// Build the error for a stray positional. If it immediately follows a bare
/// optional-value option, hint that such options need an attached value.
fn unexpected_argument(arg: &str, prev: Option<&[u8]>) -> AppError {
    // (previous token, short letter, long name) of the optional-value options
    let opts: &[(&[u8], char, &str)] = &[
        (b"-a", 'a', "addr"),
        (b"--addr", 'a', "addr"),
        (b"-u", 'u', "unixsocket"),
        (b"--unixsocket", 'u', "unixsocket"),
        (b"-d", 'd', "pidfile"),
        (b"--pidfile", 'd', "pidfile"),
        (b"-k", 'k', "kill"),
        (b"--kill", 'k', "kill"),
    ];
    if let Some(prev) = prev {
        if let Some((_, short, long)) = opts.iter().find(|(tok, _, _)| *tok == prev) {
            return AppError::usage(format!(
                "unexpected argument '{arg}'; did you mean -{short}{arg} or --{long}={arg}? \
                 optional-value options take their value attached, not as a separate argument"
            ));
        }
    }
    AppError::usage(format!("unexpected argument '{arg}'"))
}

fn parse_port(bytes: &[u8]) -> AppResult<u16> {
    let s = String::from_utf8_lossy(bytes);
    s.parse::<u16>()
        .map_err(|_| AppError::usage(format!("invalid port number '{s}'")))
}

fn parse_timeout(bytes: &[u8]) -> AppResult<Duration> {
    let s = String::from_utf8_lossy(bytes);
    let secs = s
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| {
            AppError::usage(format!(
                "invalid timeout '{s}'; expected a positive number of seconds"
            ))
        })?;
    Ok(Duration::from_secs_f64(secs))
}

// ---- validation / building ---------------------------------------------------

fn build_config(raw: Raw) -> AppResult<Config> {
    if raw.help {
        return Ok(Config::General(GeneralAction::Help));
    }
    if raw.version {
        return Ok(Config::General(GeneralAction::Version));
    }
    if raw.server && raw.client {
        return Err(AppError::usage("-s and -c cannot be combined"));
    }
    if raw.server {
        build_server(raw)
    } else if raw.client {
        build_client(raw)
    } else {
        build_general(raw)
    }
}

fn not_allowed(opt: &str, mode: &str) -> AppError {
    AppError::usage(format!("{opt} is not allowed when running in {mode} mode"))
}

fn resolve_bind(raw: &Raw) -> AppResult<Bind> {
    let unix = raw.unixsocket.is_some();
    let addr_given = raw.addr.is_some();
    if unix && addr_given {
        return Err(AppError::usage("-a and -u cannot be combined"));
    }
    if unix {
        if raw.port.is_some() {
            return Err(AppError::usage("-p is only allowed together with -a"));
        }
        let path = raw.unixsocket.clone().flatten().map(PathBuf::from);
        Ok(Bind::Unix { path })
    } else {
        let addr = raw
            .addr
            .clone()
            .flatten()
            .unwrap_or_else(|| DEFAULT_ADDR.to_string());
        let port = raw.port.unwrap_or(DEFAULT_PORT);
        Ok(Bind::Tcp { addr, port })
    }
}

fn build_server(raw: Raw) -> AppResult<Config> {
    if raw.arginput.is_some() {
        return Err(not_allowed("--arginput", "server"));
    }
    if raw.fileinput.is_some() {
        return Err(not_allowed("--fileinput", "server"));
    }
    if raw.suffix.is_some() {
        return Err(not_allowed("--suffix", "server"));
    }
    if raw.ctl.is_some() {
        return Err(not_allowed("--ctl", "server"));
    }
    if raw.kill.is_some() {
        return Err(not_allowed("--kill", "server"));
    }
    if raw.timeout.is_some() {
        return Err(not_allowed("--timeout", "server"));
    }

    let bind = resolve_bind(&raw)?;

    let repl_argv = raw.exec.filter(|v| !v.is_empty()).ok_or_else(|| {
        AppError::usage("no REPL command given; use -e or -- followed by the command")
    })?;

    let pidfile = match raw.pidfile {
        None => None,
        Some(None) => Some(PidfileSpec::Default),
        Some(Some(p)) => Some(PidfileSpec::Path(PathBuf::from(p))),
    };

    let ready_marker_stdout = raw.ready_marker_stdout.unwrap_or_default();
    let ready_marker_stderr = raw.ready_marker_stderr.unwrap_or_default();
    let has_ready_marker = !ready_marker_stdout.is_empty() || !ready_marker_stderr.is_empty();
    if has_ready_marker && raw.ready_wait.is_some() {
        return Err(AppError::usage(
            "--ready-wait cannot be combined with --ready-marker-stdout or --ready-marker-stderr",
        ));
    }
    if raw.ready_marker_timeout.is_some() && !has_ready_marker {
        return Err(AppError::usage(
            "--ready-marker-timeout requires --ready-marker-stdout or --ready-marker-stderr",
        ));
    }
    let warmup = raw.warmup.unwrap_or_default();
    let warmup_marker_stdout = raw.warmup_marker_stdout.unwrap_or_default();
    let warmup_marker_stderr = raw.warmup_marker_stderr.unwrap_or_default();
    let has_warmup_marker = !warmup_marker_stdout.is_empty() || !warmup_marker_stderr.is_empty();
    let has_warmup_end = has_warmup_marker || raw.warmup_wait.is_some();
    if !warmup.is_empty() && !has_warmup_end {
        return Err(AppError::usage(
            "--warmup-input requires a way to detect its end \
             (--warmup-marker-stdout/--warmup-marker-stderr or --warmup-wait)",
        ));
    }
    if warmup.is_empty() && has_warmup_end {
        return Err(AppError::usage(
            "--warmup-marker-* / --warmup-wait require --warmup-input",
        ));
    }
    if raw.warmup_marker_timeout.is_some() && !has_warmup_marker {
        return Err(AppError::usage(
            "--warmup-marker-timeout requires --warmup-marker-stdout or --warmup-marker-stderr",
        ));
    }

    Ok(Config::Server(ServerConfig {
        repl_argv,
        restart: raw.restart,
        bind,
        pidfile,
        raw: raw.raw,
        end_marker_stdout: raw
            .end_marker_stdout
            .unwrap_or_else(default_end_marker_stdout),
        end_marker_stderr: raw.end_marker_stderr.unwrap_or_default(),
        error_marker_stdout: raw.error_marker_stdout.unwrap_or_default(),
        error_marker_stderr: raw
            .error_marker_stderr
            .unwrap_or_else(default_error_marker_stderr),
        strip_marker_stdout: raw.strip_marker_stdout,
        strip_marker_stderr: raw.strip_marker_stderr,
        ready_marker_stdout,
        ready_marker_stderr,
        ready_wait: raw.ready_wait,
        ready_marker_timeout: raw.ready_marker_timeout,
        warmup,
        warmup_marker_stdout,
        warmup_marker_stderr,
        warmup_wait: raw.warmup_wait,
        warmup_marker_timeout: raw.warmup_marker_timeout,
        response_timeout: raw.response_timeout,
    }))
}

fn build_client(raw: Raw) -> AppResult<Config> {
    if raw.exec.is_some() {
        return Err(not_allowed("--exec", "client"));
    }
    if raw.restart {
        return Err(not_allowed("--restart", "client"));
    }
    if raw.response_timeout.is_some() {
        return Err(not_allowed("--response-timeout", "client"));
    }
    if raw.ready_marker_stdout.is_some() {
        return Err(not_allowed("--ready-marker-stdout", "client"));
    }
    if raw.ready_marker_stderr.is_some() {
        return Err(not_allowed("--ready-marker-stderr", "client"));
    }
    if raw.ready_wait.is_some() {
        return Err(not_allowed("--ready-wait", "client"));
    }
    if raw.ready_marker_timeout.is_some() {
        return Err(not_allowed("--ready-marker-timeout", "client"));
    }
    if raw.warmup.is_some() {
        return Err(not_allowed("--warmup-input", "client"));
    }
    if raw.warmup_marker_stdout.is_some() {
        return Err(not_allowed("--warmup-marker-stdout", "client"));
    }
    if raw.warmup_marker_stderr.is_some() {
        return Err(not_allowed("--warmup-marker-stderr", "client"));
    }
    if raw.warmup_wait.is_some() {
        return Err(not_allowed("--warmup-wait", "client"));
    }
    if raw.warmup_marker_timeout.is_some() {
        return Err(not_allowed("--warmup-marker-timeout", "client"));
    }
    if raw.pidfile.is_some() {
        return Err(not_allowed("--pidfile", "client"));
    }
    if raw.kill.is_some() {
        return Err(not_allowed("--kill", "client"));
    }

    let bind = resolve_bind(&raw)?;

    let fileinput = raw.fileinput.map(|p| {
        if p.as_os_str() == OsStr::new("-") {
            FileInput::Stdin
        } else {
            FileInput::Path(PathBuf::from(p))
        }
    });

    let ctl = match raw.ctl.as_deref() {
        None | Some("ignore") => CtlRoute::Ignore,
        Some("stdout") => CtlRoute::Stdout,
        Some("stderr") => CtlRoute::Stderr,
        Some(other) => {
            return Err(AppError::usage(format!(
                "invalid --ctl mode '{other}'; expected ignore, stdout, or stderr"
            )))
        }
    };

    Ok(Config::Client(ClientConfig {
        bind,
        arginput: raw.arginput,
        fileinput,
        suffix: raw.suffix.unwrap_or_default(),
        end_marker_stdout: raw
            .end_marker_stdout
            .unwrap_or_else(default_end_marker_stdout),
        end_marker_stderr: raw.end_marker_stderr.unwrap_or_default(),
        error_marker_stdout: raw.error_marker_stdout.unwrap_or_default(),
        error_marker_stderr: raw
            .error_marker_stderr
            .unwrap_or_else(default_error_marker_stderr),
        strip_marker_stdout: raw.strip_marker_stdout,
        strip_marker_stderr: raw.strip_marker_stderr,
        ctl,
        raw: raw.raw,
        timeout: raw.timeout,
    }))
}

fn build_general(raw: Raw) -> AppResult<Config> {
    let forbidden = [
        (raw.addr.is_some(), "-a"),
        (raw.port.is_some(), "-p"),
        (raw.unixsocket.is_some(), "-u"),
        (raw.exec.is_some(), "-e"),
        (raw.restart, "--restart"),
        (raw.raw, "--raw"),
        (raw.pidfile.is_some(), "--pidfile"),
        (raw.arginput.is_some(), "--arginput"),
        (raw.fileinput.is_some(), "--fileinput"),
        (raw.suffix.is_some(), "--suffix"),
        (raw.end_marker_stdout.is_some(), "--end-marker-stdout"),
        (raw.end_marker_stderr.is_some(), "--end-marker-stderr"),
        (raw.error_marker_stdout.is_some(), "--error-marker-stdout"),
        (raw.error_marker_stderr.is_some(), "--error-marker-stderr"),
        (raw.ready_marker_stdout.is_some(), "--ready-marker-stdout"),
        (raw.ready_marker_stderr.is_some(), "--ready-marker-stderr"),
        (raw.ready_wait.is_some(), "--ready-wait"),
        (raw.ready_marker_timeout.is_some(), "--ready-marker-timeout"),
        (raw.warmup.is_some(), "--warmup-input"),
        (raw.warmup_marker_stdout.is_some(), "--warmup-marker-stdout"),
        (raw.warmup_marker_stderr.is_some(), "--warmup-marker-stderr"),
        (raw.warmup_wait.is_some(), "--warmup-wait"),
        (
            raw.warmup_marker_timeout.is_some(),
            "--warmup-marker-timeout",
        ),
        (raw.ctl.is_some(), "--ctl"),
        (raw.timeout.is_some(), "--timeout"),
        (raw.response_timeout.is_some(), "--response-timeout"),
        (raw.strip_marker_stdout, "--strip-marker-stdout"),
        (raw.strip_marker_stderr, "--strip-marker-stderr"),
    ];
    for (present, opt) in forbidden {
        if present {
            return Err(AppError::usage(format!(
                "{opt} is only valid in server or client mode"
            )));
        }
    }

    match raw.kill {
        Some(k) => Ok(Config::General(GeneralAction::Kill(k.map(PathBuf::from)))),
        None => Err(AppError::usage(
            "no mode selected; use --server, --client, --kill, or --help",
        )),
    }
}

#[cfg(test)]
mod tests;
