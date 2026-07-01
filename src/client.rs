//! Client-mode runtime: connect to a server, send input (byte-exact, no added
//! newlines), then read the response.
//!
//! In the default (framed) mode the client demuxes the server's packets,
//! writing the REPL's `out` stream to its stdout and `err` to its stderr, and
//! applying separate end-of-response markers to each. In `--raw` mode it reads a
//! single unframed byte stream like the original forwarder.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::cli::config::{Bind, ClientConfig, CtlRoute, FileInput};
use crate::error::{AppError, AppResult};
use crate::marker::{Feed, MarkerScanner, Markers, Outcome};
use crate::names;
use crate::protocol::{self, Ctl, PacketReader, ProtocolError};
use crate::transport::Stream;

const BUF: usize = 64 * 1024;

pub fn run(cfg: ClientConfig) -> AppResult<()> {
    let bind = resolve_bind(cfg.bind.clone())?;

    let stream = Stream::connect(&bind)
        .map_err(|e| AppError::failure(format!("cannot connect to server: {e}")))?;

    // Full-duplex: a sender thread pushes input while the main thread reads the
    // response, so a REPL that replies mid-input cannot deadlock us.
    let writer = stream
        .try_clone()
        .map_err(|e| AppError::failure(format!("cannot clone connection: {e}")))?;
    let arginput = cfg.arginput.clone();
    let fileinput = cfg.fileinput.clone();
    let suffix = cfg.suffix.clone();
    let sender = thread::spawn(move || send_input(writer, arginput, fileinput, suffix));

    // Optional timeout: a watchdog shuts the socket down at the deadline, which
    // unblocks the reader; the read then fails and we report a timeout.
    let timed_out = Arc::new(AtomicBool::new(false));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let watchdog = match cfg.timeout {
        Some(dur) => {
            let sh = stream
                .try_clone()
                .map_err(|e| AppError::failure(format!("cannot clone connection: {e}")))?;
            let timed_out = Arc::clone(&timed_out);
            let done = Arc::clone(&done);
            Some(thread::spawn(move || {
                let (lock, cv) = &*done;
                let guard = lock.lock().unwrap();
                let (_g, res) = cv.wait_timeout_while(guard, dur, |d| !*d).unwrap();
                if res.timed_out() {
                    timed_out.store(true, Ordering::SeqCst);
                    let _ = sh.shutdown(Shutdown::Both);
                }
            }))
        }
        None => None,
    };

    let result = if cfg.raw {
        read_raw(stream, &cfg)
    } else {
        read_protocol(stream, &cfg)
    };

    // Release the watchdog (a completed read wins over a racing deadline).
    {
        let (lock, cv) = &*done;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }
    if let Some(w) = watchdog {
        let _ = w.join();
    }
    let _ = sender.join();

    match result {
        Ok(()) => Ok(()),
        Err(_) if timed_out.load(Ordering::SeqCst) => Err(AppError::timeout(format!(
            "timed out after {} seconds waiting for a complete response",
            cfg.timeout.unwrap_or_default().as_secs_f64()
        ))),
        Err(e) => Err(e),
    }
}

/// For unix transport without an explicit path, discover a single socket.
fn resolve_bind(bind: Bind) -> AppResult<Bind> {
    match bind {
        Bind::Unix { path: None } => Ok(Bind::Unix {
            path: Some(names::discover_socket()?),
        }),
        other => Ok(other),
    }
}

// ---- sending input -----------------------------------------------------------

fn send_input(
    mut writer: Stream,
    arginput: Option<Vec<u8>>,
    fileinput: Option<FileInput>,
    suffix: Vec<u8>,
) -> io::Result<()> {
    if let Some(arg) = arginput {
        writer.write_all(&arg)?;
    }
    match fileinput {
        Some(FileInput::Stdin) => {
            let stdin = io::stdin();
            copy(&mut stdin.lock(), &mut writer)?;
        }
        Some(FileInput::Path(path)) => {
            let mut file = std::fs::File::open(&path)?;
            copy(&mut file, &mut writer)?;
        }
        None => {}
    }
    if !suffix.is_empty() {
        writer.write_all(&suffix)?;
    }
    writer.flush()?;
    Ok(())
}

fn copy(src: &mut impl Read, dst: &mut impl Write) -> io::Result<()> {
    let mut buf = vec![0u8; BUF];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
    }
    Ok(())
}

// ---- framed (protocol) reading ----------------------------------------------

fn read_protocol(stream: Stream, cfg: &ClientConfig) -> AppResult<()> {
    let mut reader = PacketReader::new(stream);
    let stdout = io::stdout();
    let stderr = io::stderr();

    let mut out_scanner = scanner_for(out_markers(cfg), cfg.strip_out_marker);
    let mut err_scanner = scanner_for(err_markers(cfg), cfg.strip_err_marker);

    loop {
        match reader.read_packet() {
            Ok(Some(pkt)) => match pkt.stream.as_str() {
                protocol::STREAM_OUT => {
                    if let Some(outcome) = feed(&mut out_scanner, &pkt.payload, &mut stdout.lock())?
                    {
                        return finish(outcome);
                    }
                }
                protocol::STREAM_ERR => {
                    if let Some(outcome) = feed(&mut err_scanner, &pkt.payload, &mut stderr.lock())?
                    {
                        return finish(outcome);
                    }
                }
                protocol::STREAM_CTL => {
                    let text = String::from_utf8_lossy(&pkt.payload).into_owned();
                    match protocol::parse_ctl(&pkt.payload) {
                        Ctl::Error(msg) => return Err(AppError::failure(format!("server: {msg}"))),
                        _ => route_ctl(cfg.ctl, &text),
                    }
                }
                _ => {} // ignore unknown streams for forward-compatibility
            },
            Ok(None) => {
                flush_scanner(out_scanner, &mut stdout.lock())?;
                flush_scanner(err_scanner, &mut stderr.lock())?;
                return Err(AppError::failure(
                    "connection closed by server before end-of-response marker",
                ));
            }
            Err(ProtocolError::UnsupportedVersion { major, minor }) => {
                return Err(AppError::failure(format!(
                    "server protocol version {major}.{minor} is newer than this client supports; \
                     read the stream with text tools, or run both server and client with --raw"
                )));
            }
            Err(e) => return Err(AppError::failure(e.to_string())),
        }
    }
}

/// Markers watched on the REPL's stdout: an end marker (success) and an error
/// marker, either of which may be disabled (empty).
fn out_markers(cfg: &ClientConfig) -> Markers {
    Markers::new(vec![
        (cfg.end_marker_stdout.clone(), Outcome::End),
        (cfg.error_marker_stdout.clone(), Outcome::Error),
    ])
}

/// Markers watched on the REPL's stderr.
fn err_markers(cfg: &ClientConfig) -> Markers {
    Markers::new(vec![
        (cfg.end_marker_stderr.clone(), Outcome::End),
        (cfg.error_marker_stderr.clone(), Outcome::Error),
    ])
}

fn scanner_for(markers: Markers, strip: bool) -> Option<MarkerScanner> {
    if markers.is_empty() {
        None
    } else {
        Some(MarkerScanner::new(markers, strip))
    }
}

/// Turn a completed response into a process result: an error marker exits
/// non-zero, an end marker exits zero.
fn finish(outcome: Outcome) -> AppResult<()> {
    match outcome {
        Outcome::End => Ok(()),
        Outcome::Error => Err(AppError::failure("end-of-response error marker seen")),
    }
}

/// Feed payload to the (optional) scanner and write emitted bytes. Returns the
/// outcome when a marker is found.
fn feed(
    scanner: &mut Option<MarkerScanner>,
    payload: &[u8],
    out: &mut impl Write,
) -> AppResult<Option<Outcome>> {
    match scanner {
        None => {
            write_out(out, payload)?;
            Ok(None)
        }
        Some(s) => match s.feed(payload) {
            Feed::More(bytes) => {
                write_out(out, &bytes)?;
                Ok(None)
            }
            Feed::Done { outcome, bytes } => {
                write_out(out, &bytes)?;
                out.flush().ok();
                Ok(Some(outcome))
            }
        },
    }
}

fn flush_scanner(scanner: Option<MarkerScanner>, out: &mut impl Write) -> AppResult<()> {
    if let Some(s) = scanner {
        let rest = s.flush();
        write_out(out, &rest)?;
        out.flush().ok();
    }
    Ok(())
}

fn route_ctl(route: CtlRoute, line: &str) {
    match route {
        CtlRoute::Ignore => {}
        CtlRoute::Stdout => {
            let _ = writeln!(io::stdout(), "{line}");
        }
        CtlRoute::Stderr => {
            let _ = writeln!(io::stderr(), "{line}");
        }
    }
}

// ---- raw (unframed) reading --------------------------------------------------

fn read_raw(mut stream: Stream, cfg: &ClientConfig) -> AppResult<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // Raw collapses the two streams into one, so every marker applies to it. The
    // stdout strip flag governs stripping on the merged stream.
    let markers = Markers::new(vec![
        (cfg.end_marker_stdout.clone(), Outcome::End),
        (cfg.end_marker_stderr.clone(), Outcome::End),
        (cfg.error_marker_stdout.clone(), Outcome::Error),
        (cfg.error_marker_stderr.clone(), Outcome::Error),
    ]);

    if markers.is_empty() {
        copy(&mut stream, &mut out)
            .map_err(|e| AppError::failure(format!("error reading from server: {e}")))?;
        out.flush().ok();
        return Ok(());
    }

    let mut scanner = MarkerScanner::new(markers, cfg.strip_out_marker);
    let mut buf = vec![0u8; BUF];
    loop {
        let n = stream
            .read(&mut buf)
            .map_err(|e| AppError::failure(format!("error reading from server: {e}")))?;
        if n == 0 {
            let rest = scanner.flush();
            write_out(&mut out, &rest)?;
            return Err(AppError::failure(
                "connection closed by server before end-of-response marker",
            ));
        }
        match scanner.feed(&buf[..n]) {
            Feed::More(bytes) => write_out(&mut out, &bytes)?,
            Feed::Done { outcome, bytes } => {
                write_out(&mut out, &bytes)?;
                out.flush().ok();
                return finish(outcome);
            }
        }
    }
}

fn write_out(out: &mut impl Write, bytes: &[u8]) -> AppResult<()> {
    out.write_all(bytes)
        .map_err(|e| AppError::failure(format!("error writing output: {e}")))
}
