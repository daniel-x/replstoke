//! Server-mode runtime.
//!
//! Concurrency model (std-only, blocking I/O, one thread per data direction):
//!
//! | thread        | source -> sink                                   |
//! |---------------|-------------------------------------------------|
//! | main          | accept loop + REPL supervision + lifecycle      |
//! | `repl_out`    | REPL stdout -> active client, else server stdout|
//! | `repl_err`    | REPL stderr -> server stderr, and active client |
//! | `client_in`   | client socket -> REPL stdin (per active client) |
//!
//! By default the server-to-client direction is framed (see `protocol`): the
//! REPL's stdout and stderr become separate `out`/`err` packet streams and the
//! server pushes `ctl` status/error packets. `--raw` disables framing and the
//! server behaves like a plain forwarder (stderr merged into the client stream).
//!
//! The server's control state is one explicit state machine (see
//! `STATE_MACHINE_DESIGN.md`): a [`Core`] holding a [`Phase`] plus the client
//! slot, guarded by a `Mutex` + `Condvar`. The data path is untouched; only the
//! control plane is consolidated. The hard rule is that the `Core` lock is held
//! only to read/transition/notify - never across socket or pipe I/O. The two
//! blocking sinks (the REPL's stdin, the client socket) therefore live in their
//! own mutexes and are written outside the `Core` lock.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::PathBuf;
use std::process::{Child, ChildStdin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::cli::config::{Bind, PidfileSpec, ServerConfig};
use crate::error::{AppError, AppResult};
use crate::marker::{Feed, MarkerScanner, Markers, Outcome, ResponseScanner};
use crate::protocol::{self, STREAM_ERR, STREAM_OUT};
use crate::repl::{self, Repl};
use crate::transport::{Listener, Stream};
use crate::{names, platform};

const BUF: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(50);
/// A REPL that exits within this window of being spawned, before serving any
/// client, counts as a startup failure.
const STARTUP_GRACE: Duration = Duration::from_millis(500);
/// Consecutive startup failures before the crash-loop breaker gives up.
const MAX_STARTUP_FAILURES: u32 = 5;

struct ClientConn {
    sink: Stream,
    gen: u64,
}

/// The REPL/request lifecycle. Every variant holds only `Copy` data, so a
/// snapshot is cheap.
#[derive(Clone, Copy, Debug)]
enum Phase {
    /// REPL spawned, not yet ready. A client may be attached but its input is
    /// not forwarded until the REPL becomes ready.
    Booting { since: Instant },
    /// Booted, running the one-shot `--warmup-input` before serving clients. A
    /// client may be attached but its input is not forwarded; the warmup's output goes to
    /// the server, not the client. `since` bounds the optional warmup timeout.
    Warmup { since: Instant },
    /// Ready, no request in flight (a client may or may not be attached).
    Idle,
    /// A request's input was forwarded; awaiting an end-of-response marker.
    Busy,
    /// The client left mid-request; the REPL is still finishing. A deadline, if
    /// present, bounds how long to wait before treating it as wedged.
    Draining { deadline: Option<Instant> },
    /// Terminal.
    Stopped,
}

/// The consolidated control state, guarded by `Mutex` + `Condvar`. Holds the
/// lifecycle phase and the counters; the client and REPL-stdin *handles* live in
/// their own mutexes because they are written with blocking I/O.
struct Core {
    phase: Phase,
    /// This REPL instance has accepted at least one client (startup-failure
    /// accounting).
    served: bool,
    requests: u64,
    next_gen: u64,
    /// Incremented each time a request begins (Idle -> Busy). A pump uses it to
    /// reset its response scanner cleanly at every request boundary, so one
    /// response's held-back partial-marker bytes never bleed into the next.
    req_seq: u64,
    /// True from the connected client's first request until it disconnects. REPL
    /// output is forwarded to that client for the whole span, so a response is not
    /// truncated when the server's own end marker fires (Busy -> Idle) while more
    /// output is still in flight on the other stream. The end marker only decides
    /// when the *next* client may be served, never when to stop feeding this one.
    serving: bool,
}

impl Core {
    fn new(ready: bool, has_warmup: bool, now: Instant) -> Core {
        Core {
            phase: initial_phase(ready, has_warmup, now),
            served: false,
            requests: 0,
            next_gen: 1,
            req_seq: 0,
            serving: false,
        }
    }

    fn is_booting(&self) -> bool {
        matches!(self.phase, Phase::Booting { .. })
    }

    /// The REPL is not yet servable: still booting or running its warmup. Client
    /// input is not forwarded and REPL output is withheld from the client in both.
    fn is_pre_serving(&self) -> bool {
        matches!(self.phase, Phase::Booting { .. } | Phase::Warmup { .. })
    }

    /// Accept gate. `has_sink` is whether a client sink is already stored. Accepts
    /// only when no client is attached and the REPL is not mid-request. On success
    /// bumps the counters and returns the new client generation.
    fn try_accept(&mut self, has_sink: bool) -> Option<u64> {
        let acceptable = matches!(
            self.phase,
            Phase::Booting { .. } | Phase::Warmup { .. } | Phase::Idle
        );
        if has_sink || !acceptable {
            return None;
        }
        let gen = self.next_gen;
        self.next_gen += 1;
        self.requests += 1;
        self.served = true;
        self.serving = false;
        Some(gen)
    }

    /// A request's first input was forwarded: Idle -> Busy. Framed mode only; raw
    /// mode has no end-of-response marker, so it never becomes Busy (which would
    /// never clear). Marks the client as being served for the rest of its
    /// connection, so trailing output is not cut when the end marker fires.
    fn request_start(&mut self, protocol: bool) {
        if !protocol {
            return;
        }
        self.serving = true;
        if matches!(self.phase, Phase::Idle) {
            self.phase = Phase::Busy;
            self.req_seq += 1;
        }
    }

    /// End-of-response marker seen: Busy/Draining -> Idle.
    fn response_end(&mut self) {
        if matches!(self.phase, Phase::Busy | Phase::Draining { .. }) {
            self.phase = Phase::Idle;
        }
    }

    /// The active client departed. Busy -> Draining (arming `deadline`); in any
    /// other phase the phase is unchanged (the sink is cleared by the caller).
    fn client_gone(&mut self, deadline: Option<Instant>) {
        self.serving = false;
        if matches!(self.phase, Phase::Busy) {
            self.phase = Phase::Draining { deadline };
        }
    }

    /// The REPL finished booting. With a warmup configured it enters Warmup;
    /// otherwise it is Idle and servable. Returns whether it transitioned (so the
    /// caller notifies waiters).
    fn ready(&mut self, has_warmup: bool, now: Instant) -> bool {
        if self.is_booting() {
            self.phase = if has_warmup {
                Phase::Warmup { since: now }
            } else {
                Phase::Idle
            };
            true
        } else {
            false
        }
    }

    /// The warmup finished: Warmup -> Idle. Returns whether it transitioned (so the
    /// caller notifies waiters).
    fn warmup_done(&mut self) -> bool {
        if matches!(self.phase, Phase::Warmup { .. }) {
            self.phase = Phase::Idle;
            true
        } else {
            false
        }
    }

    /// A freshly (re)spawned REPL instance begins its lifecycle.
    fn begin_booting(&mut self, ready: bool, has_warmup: bool, now: Instant) {
        self.phase = initial_phase(ready, has_warmup, now);
        self.served = false;
        self.serving = false;
    }

    fn stop(&mut self) {
        self.phase = Phase::Stopped;
    }
}

/// The phase a freshly (re)spawned REPL starts in: Booting while a readiness
/// mechanism is pending; else Warmup if a warmup is configured; else immediately
/// servable (Idle).
fn initial_phase(ready: bool, has_warmup: bool, now: Instant) -> Phase {
    if !ready {
        Phase::Booting { since: now }
    } else if has_warmup {
        Phase::Warmup { since: now }
    } else {
        Phase::Idle
    }
}

struct Shared {
    core: Mutex<Core>,
    cv: Condvar,
    /// The active client's write handle (separate lock: written with blocking
    /// I/O, must not be held under the `Core` lock).
    sink: Mutex<Option<ClientConn>>,
    /// The current REPL's stdin (separate lock, likewise blocking).
    repl_stdin: Mutex<Option<ChildStdin>>,
    shutdown: AtomicBool,
    /// Whether a (re)spawned REPL is ready immediately (no readiness mechanism).
    initial_ready: bool,
    /// Input run once after the REPL boots, before serving clients; empty disables
    /// the warmup phase.
    warmup: Vec<u8>,
    /// End-of-warmup markers watched during Warmup (per stream); empty disables.
    warmup_marker_stdout: Vec<u8>,
    warmup_marker_stderr: Vec<u8>,
    /// Whether this REPL instance's warmup input has been written. Warmup cannot
    /// complete before it is sent, so boot output seen before it cannot end the
    /// warmup prematurely.
    warmup_sent: AtomicBool,
    protocol: bool,
    end_marker_stdout: Vec<u8>,
    end_marker_stderr: Vec<u8>,
    error_marker_stdout: Vec<u8>,
    error_marker_stderr: Vec<u8>,
    ready_marker_stdout: Vec<u8>,
    ready_marker_stderr: Vec<u8>,
    strip_out: bool,
    strip_err: bool,
    response_timeout: Option<Duration>,
    start: Instant,
    started_at: String,
    server_pid: u32,
    listen_desc: String,
}

impl Shared {
    fn should_stop(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst) || platform::shutdown_requested()
    }

    fn phase(&self) -> Phase {
        self.core.lock().unwrap().phase
    }

    /// Phase plus whether a connected client is currently being served, snapshotted
    /// together so a pump routes output consistently.
    fn phase_and_serving(&self) -> (Phase, bool) {
        let core = self.core.lock().unwrap();
        (core.phase, core.serving)
    }

    fn is_booting(&self) -> bool {
        self.core.lock().unwrap().is_booting()
    }

    fn served(&self) -> bool {
        self.core.lock().unwrap().served
    }

    fn req_seq(&self) -> u64 {
        self.core.lock().unwrap().req_seq
    }

    fn has_warmup(&self) -> bool {
        !self.warmup.is_empty()
    }

    // ---- transitions (lock Core briefly, then notify; never do I/O here) ------

    fn request_start(&self) {
        self.core.lock().unwrap().request_start(self.protocol);
    }

    fn response_end(&self) {
        self.core.lock().unwrap().response_end();
    }

    /// The REPL became ready; move to Warmup or Idle and wake anything parked
    /// waiting to forward input.
    fn signal_ready(&self) {
        if self
            .core
            .lock()
            .unwrap()
            .ready(self.has_warmup(), Instant::now())
        {
            self.cv.notify_all();
        }
    }

    /// The warmup finished; the REPL is now servable. Ignored until the warmup has
    /// actually been sent, so a pre-warmup (startup) prompt cannot end it early.
    fn warmup_done(&self) {
        if !self.warmup_sent.load(Ordering::SeqCst) {
            return;
        }
        if self.core.lock().unwrap().warmup_done() {
            self.cv.notify_all();
        }
    }

    fn warmup_is_sent(&self) -> bool {
        self.warmup_sent.load(Ordering::SeqCst)
    }

    /// Write the warmup input to the REPL's stdin (outside the Core lock), then
    /// mark it sent. The flag is set only *after* the bytes are in the pipe, so a
    /// deferred client's input (forwarded once warmup completes) can never overtake
    /// the warmup in the REPL's stdin. Best-effort: a dead REPL is caught by the
    /// supervisor's exit handling.
    fn send_warmup(&self) {
        {
            let mut stdin = self.repl_stdin.lock().unwrap();
            if let Some(w) = stdin.as_mut() {
                let _ = w.write_all(&self.warmup);
                let _ = w.flush();
            }
        }
        self.warmup_sent.store(true, Ordering::SeqCst);
    }

    /// Begin a fresh boot phase after a (re)spawn.
    fn begin_booting(&self) {
        self.warmup_sent.store(false, Ordering::SeqCst);
        self.core.lock().unwrap().begin_booting(
            self.initial_ready,
            self.has_warmup(),
            Instant::now(),
        );
        self.cv.notify_all();
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.core.lock().unwrap().stop();
        self.cv.notify_all();
    }

    /// The client identified by `gen` has departed: drop its sink and, if it left
    /// mid-request, transition to Draining. Locks Core before sink to keep the one
    /// consistent lock order (Core -> sink).
    fn client_gone(&self, gen: u64) {
        let deadline = self.response_timeout.map(|t| Instant::now() + t);
        let mut core = self.core.lock().unwrap();
        {
            let mut sink = self.sink.lock().unwrap();
            if sink.as_ref().map(|c| c.gen) == Some(gen) {
                *sink = None;
            }
        }
        core.client_gone(deadline);
    }

    /// Drop the active client (on respawn) and shut its socket down so its
    /// `client_in` thread unwinds promptly instead of limping.
    fn clear_sink(&self) {
        if let Some(conn) = self.sink.lock().unwrap().take() {
            let _ = conn.sink.shutdown(Shutdown::Both);
        }
    }

    // ---- output delivery (operates on the sink lock, never on Core) -----------

    /// Write a chunk to the active client as either a framed packet or raw bytes.
    fn deliver(&self, conn: &mut ClientConn, stream_name: &str, chunk: &[u8]) -> io::Result<()> {
        if self.protocol {
            let mut pkt = Vec::with_capacity(chunk.len() + 32);
            protocol::encode(&mut pkt, stream_name, chunk);
            conn.sink.write_all(&pkt)?;
        } else {
            conn.sink.write_all(chunk)?;
        }
        conn.sink.flush()
    }

    /// REPL stdout: to the active client (framed `out`) when `to_client`, else the
    /// server's own stdout. Boot-phase output is withheld from the client
    /// (`to_client == false`) so a booting REPL's banner is not mistaken for a
    /// response.
    fn route_out(&self, chunk: &[u8], to_client: bool) {
        let mut guard = self.sink.lock().unwrap();
        match guard.as_mut() {
            Some(conn) if to_client => {
                if self.deliver(conn, STREAM_OUT, chunk).is_err() {
                    *guard = None;
                }
            }
            _ => {
                let stdout = io::stdout();
                let mut lock = stdout.lock();
                let _ = lock.write_all(chunk);
                let _ = lock.flush();
            }
        }
    }

    /// REPL stderr: always to the server's stderr, and to the active client too
    /// when `to_client` (withheld during the boot phase).
    fn route_err(&self, chunk: &[u8], to_client: bool) {
        {
            let stderr = io::stderr();
            let mut lock = stderr.lock();
            let _ = lock.write_all(chunk);
            let _ = lock.flush();
        }
        if !to_client {
            return;
        }
        let mut guard = self.sink.lock().unwrap();
        if let Some(conn) = guard.as_mut() {
            if self.deliver(conn, STREAM_ERR, chunk).is_err() {
                *guard = None;
            }
        }
    }

    /// Send a `ctl` packet to the active client (framed mode only).
    fn send_ctl(&self, payload: &[u8]) {
        if !self.protocol {
            return;
        }
        let mut guard = self.sink.lock().unwrap();
        if let Some(conn) = guard.as_mut() {
            if self.deliver(conn, protocol::STREAM_CTL, payload).is_err() {
                *guard = None;
            }
        }
    }

    fn send_ctl_status(&self, repl_pid: u32) {
        let requests = self.core.lock().unwrap().requests;
        let fields = format!(
            "server_pid={} repl_pid={} requests={} uptime_s={} started={} listening={}",
            self.server_pid,
            repl_pid,
            requests,
            self.start.elapsed().as_secs(),
            self.started_at,
            self.listen_desc,
        );
        self.send_ctl(&protocol::ctl_status(&fields));
    }

    fn send_ctl_error(&self, message: &str) {
        self.send_ctl(&protocol::ctl_error(message));
    }

    // ---- input forwarding (waits under Core, writes outside it) ---------------

    /// Block until the REPL is out of its boot phase (or the server is stopping).
    /// Returns false if stopping. This is where a client's *read* is gated so no
    /// data is pulled into userspace until the REPL can process it.
    fn wait_until_ready(&self) -> bool {
        let mut core = self.core.lock().unwrap();
        while core.is_pre_serving() && !self.should_stop() {
            let (g, _) = self.cv.wait_timeout(core, POLL).unwrap();
            core = g;
        }
        !self.should_stop()
    }

    /// Write `data` to the REPL's stdin, waiting while the REPL is booting (via
    /// the condvar) or momentarily absent between exit and respawn. Returns false
    /// only if the server is shutting down. The blocking write happens under the
    /// `repl_stdin` lock, never the `Core` lock.
    fn forward_to_repl(&self, data: &[u8]) -> bool {
        loop {
            if !self.wait_until_ready() {
                return false;
            }
            {
                let mut stdin = self.repl_stdin.lock().unwrap();
                if let Some(w) = stdin.as_mut() {
                    if w.write_all(data).is_ok() && w.flush().is_ok() {
                        return true;
                    }
                }
            }
            // REPL stdin momentarily absent/broken (respawn gap): brief poll.
            if self.should_stop() {
                return false;
            }
            thread::sleep(POLL);
        }
    }
}

pub fn run(cfg: ServerConfig) -> AppResult<()> {
    platform::install_shutdown_handler();

    let pid = std::process::id();
    let cmdname = names::cmdname(&cfg.repl_argv[0]);

    // Spawn the REPL first so a bad command fails before we take the port/socket.
    let repl = repl::spawn(&cfg.repl_argv)?;

    let (bind, created_socket) = resolve_bind(cfg.bind, &cmdname, pid)?;
    let listener = match Listener::bind(&bind) {
        Ok(l) => l,
        Err(e) => {
            repl.guard.terminate();
            return Err(AppError::failure(format!(
                "cannot bind {}: {e}",
                describe(&bind)
            )));
        }
    };
    listener
        .set_nonblocking(true)
        .map_err(|e| AppError::failure(format!("cannot set non-blocking accept: {e}")))?;

    let pidfile = write_pidfile(cfg.pidfile.as_ref(), &cmdname, pid)?;

    // A REPL is ready immediately unless a readiness mechanism is configured.
    let has_readiness = cfg.ready_wait.is_some()
        || !cfg.ready_marker_stdout.is_empty()
        || !cfg.ready_marker_stderr.is_empty();
    let initial_ready = !has_readiness;

    let start = Instant::now();
    let shared = Arc::new(Shared {
        core: Mutex::new(Core::new(initial_ready, !cfg.warmup.is_empty(), start)),
        cv: Condvar::new(),
        sink: Mutex::new(None),
        repl_stdin: Mutex::new(None),
        shutdown: AtomicBool::new(false),
        initial_ready,
        warmup: cfg.warmup.clone(),
        warmup_marker_stdout: cfg.warmup_marker_stdout.clone(),
        warmup_marker_stderr: cfg.warmup_marker_stderr.clone(),
        warmup_sent: AtomicBool::new(false),
        protocol: !cfg.raw,
        end_marker_stdout: cfg.end_marker_stdout.clone(),
        end_marker_stderr: cfg.end_marker_stderr.clone(),
        error_marker_stdout: cfg.error_marker_stdout.clone(),
        error_marker_stderr: cfg.error_marker_stderr.clone(),
        ready_marker_stdout: cfg.ready_marker_stdout.clone(),
        ready_marker_stderr: cfg.ready_marker_stderr.clone(),
        strip_out: cfg.strip_marker_stdout,
        strip_err: cfg.strip_marker_stderr,
        response_timeout: cfg.response_timeout,
        start,
        started_at: format_utc_now(),
        server_pid: pid,
        listen_desc: describe(&bind),
    });

    let mut repl_threads: Vec<JoinHandle<()>> = Vec::new();
    let (mut child, mut guard) = wire_repl(repl, &shared, &mut repl_threads);

    let mut consecutive_startup_failures: u32 = 0;
    let mut last_spawn = Instant::now();
    // Per-exit reasons, consumed by the ReplExited handling:
    let mut forced = false; // a Draining deadline fired: restart even without -r
    let mut stuck = false; // a ready-marker timeout fired: count as startup failure
    let mut outcome: AppResult<()> = Ok(());

    loop {
        if shared.should_stop() {
            break;
        }

        // Time-based supervision, driven off the current phase.
        match shared.phase() {
            Phase::Booting { since } => {
                if let Some(w) = cfg.ready_wait {
                    if since.elapsed() >= w {
                        shared.signal_ready();
                    }
                } else if let Some(t) = cfg.ready_marker_timeout {
                    if !stuck && since.elapsed() >= t {
                        stuck = true;
                        guard.terminate();
                    }
                }
            }
            // The REPL just booted: run the warmup once, then end it when the
            // warmup wait elapses (if no marker arrives first), or kill the REPL if
            // its warmup marker fails to appear within the marker timeout.
            Phase::Warmup { since } => {
                if !shared.warmup_is_sent() {
                    shared.send_warmup();
                } else if cfg.warmup_wait.map_or(false, |t| since.elapsed() >= t) {
                    shared.warmup_done();
                } else if let Some(t) = cfg.warmup_marker_timeout {
                    if !stuck && since.elapsed() >= t {
                        stuck = true;
                        guard.terminate();
                    }
                }
            }
            Phase::Draining {
                deadline: Some(deadline),
            } if !forced && Instant::now() >= deadline => {
                forced = true;
                guard.terminate();
            }
            _ => {}
        }

        match child.try_wait() {
            Ok(Some(_status)) => {
                let do_restart = cfg.restart || forced;
                forced = false;
                if !do_restart {
                    shared.send_ctl_error("the REPL process exited");
                    shared.stop();
                    break;
                }
                join_threads(&mut repl_threads);

                // A REPL torn down for never signalling readiness counts as a
                // startup failure regardless of how long it took, so a REPL that
                // never boots does not restart forever.
                let startup_failure =
                    stuck || (!shared.served() && last_spawn.elapsed() < STARTUP_GRACE);
                stuck = false;
                consecutive_startup_failures = if startup_failure {
                    consecutive_startup_failures + 1
                } else {
                    0
                };

                if consecutive_startup_failures >= MAX_STARTUP_FAILURES {
                    let msg = "the REPL repeatedly failed to start; giving up";
                    shared.send_ctl_error(msg);
                    shared.stop();
                    outcome = Err(AppError::failure(msg));
                    break;
                }

                shared.clear_sink();
                let backoff = backoff_for(consecutive_startup_failures);
                if !backoff.is_zero() {
                    thread::sleep(backoff);
                }

                match repl::spawn(&cfg.repl_argv) {
                    Ok(new_repl) => {
                        last_spawn = Instant::now();
                        shared.begin_booting();
                        let (c, g) = wire_repl(new_repl, &shared, &mut repl_threads);
                        child = c;
                        guard = g;
                    }
                    Err(e) => {
                        shared.send_ctl_error(&format!("failed to restart the REPL: {e}"));
                        shared.stop();
                        outcome = Err(e);
                        break;
                    }
                }
            }
            Ok(None) => {}
            Err(_) => {
                shared.stop();
                break;
            }
        }

        match listener.accept() {
            Ok(conn) => handle_connection(conn, &shared, child.id()),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => thread::sleep(POLL),
        }
    }

    cleanup(&guard, created_socket.as_deref(), pidfile.as_deref());
    outcome
}

/// Wire a freshly spawned REPL into shared state and start its I/O threads.
fn wire_repl(
    repl: Repl,
    shared: &Arc<Shared>,
    threads: &mut Vec<JoinHandle<()>>,
) -> (Child, platform::ReplGuard) {
    *shared.repl_stdin.lock().unwrap() = Some(repl.stdin);
    *threads = spawn_repl_io(shared, repl.stdout, repl.stderr);
    (repl.child, repl.guard)
}

fn handle_connection(conn: Stream, shared: &Arc<Shared>, repl_pid: u32) {
    // Accept-gate and register the sink atomically (Core -> sink lock order) so
    // two racing connections cannot both pass the single-client gate. A client
    // that connects while the REPL is still booting IS accepted; forwarding of
    // its input is deferred (see wait_until_ready) until the REPL is ready.
    let gen = {
        let mut core = shared.core.lock().unwrap();
        let mut sink = shared.sink.lock().unwrap();
        if sink.is_some() {
            return;
        }
        let cloned = match conn.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some(gen) = core.try_accept(false) else {
            return;
        };
        *sink = Some(ClientConn { sink: cloned, gen });
        gen
    };

    shared.send_ctl_status(repl_pid);

    let shared = Arc::clone(shared);
    thread::spawn(move || client_in(conn, shared, gen));
}

/// Forward bytes from the client socket to the REPL's stdin. The read is gated on
/// readiness so nothing is pulled into userspace until the REPL can process it.
fn client_in(mut conn: Stream, shared: Arc<Shared>, gen: u64) {
    let mut buf = vec![0u8; BUF];
    loop {
        if !shared.wait_until_ready() {
            break;
        }
        let n = match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        shared.request_start();
        if !shared.forward_to_repl(&buf[..n]) {
            break;
        }
    }
    shared.client_gone(gen);
}

fn spawn_repl_io(
    shared: &Arc<Shared>,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
) -> Vec<JoinHandle<()>> {
    let out_shared = Arc::clone(shared);
    let out = thread::spawn(move || pump(stdout, &out_shared, false));
    let err_shared = Arc::clone(shared);
    let err = thread::spawn(move || pump(stderr, &err_shared, true));
    vec![out, err]
}

fn pump(mut src: impl Read, shared: &Shared, is_err: bool) {
    // Single-shot per-stream detectors that only observe bytes: `ready_scanner`
    // for the boot phase, `warmup_scanner` for the warmup phase.
    let mut ready_scanner = ready_scanner_for(shared, is_err);
    let mut warmup_scanner = warmup_scanner_for(shared, is_err);

    // Raw mode: plain forwarder. Boot and warmup output go to the server's own std
    // streams; once servable, output goes to the connected client.
    if !shared.protocol {
        let mut buf = vec![0u8; BUF];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            match shared.phase() {
                Phase::Booting { .. } => {
                    detect_ready(shared, &mut ready_scanner, &buf[..n]);
                    deliver_repl_output(shared, is_err, &buf[..n], false);
                }
                Phase::Warmup { .. } => {
                    detect_warmup(shared, &mut warmup_scanner, &buf[..n]);
                    deliver_repl_output(shared, is_err, &buf[..n], false);
                }
                _ => deliver_repl_output(shared, is_err, &buf[..n], true),
            }
        }
        return;
    }

    // Framed mode. The response scanner is fed only while a request is in flight
    // (Busy/Draining) and is reset fresh at each request start. Everything before
    // the request - boot, warmup, and idle output - is routed to the server's own
    // std streams, so nothing (including warmup output arriving on a separate
    // thread that may race the phase flip) can bleed into a client's response.
    let mut scanner = response_scanner_for(shared, is_err);
    // The request this pump's scanner is currently scanning; 0 = none. When the
    // live request advances, the scanner is reset so the previous response's
    // held-back bytes are flushed to the server rather than leaking to the next.
    let mut scanned_seq = 0u64;
    let mut buf = vec![0u8; BUF];
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let (phase, serving) = shared.phase_and_serving();
        match phase {
            Phase::Booting { .. } => {
                detect_ready(shared, &mut ready_scanner, &buf[..n]);
                deliver_repl_output(shared, is_err, &buf[..n], false);
            }
            Phase::Warmup { .. } => {
                detect_warmup(shared, &mut warmup_scanner, &buf[..n]);
                deliver_repl_output(shared, is_err, &buf[..n], false);
            }
            // Between the served client's requests (its end marker has fired) the
            // REPL may still be flushing the tail of the response on the other
            // stream: keep forwarding it to that client. Only remnants seen while no
            // client is being served (a boot/warmup leftover, or after the client
            // left) go to the server, so nothing bleeds into the next client.
            Phase::Idle => {
                deliver_repl_output(shared, is_err, &buf[..n], serving);
            }
            Phase::Stopped => {
                deliver_repl_output(shared, is_err, &buf[..n], false);
            }
            // A request is in flight: scan the response and deliver it to the
            // client, resetting the scanner cleanly at each new request boundary.
            Phase::Busy | Phase::Draining { .. } => {
                let seq = shared.req_seq();
                if seq != scanned_seq {
                    if let Some(s) = scanner.take() {
                        let rest = s.flush();
                        deliver_repl_output(shared, is_err, &rest, false);
                    }
                    scanner = response_scanner_for(shared, is_err);
                    scanned_seq = seq;
                }
                if let Some(s) = scanner.as_mut() {
                    let (emit, boundaries) = s.feed(&buf[..n]);
                    deliver_repl_output(shared, is_err, &emit, true);
                    if boundaries > 0 {
                        shared.response_end();
                    }
                } else {
                    deliver_repl_output(shared, is_err, &buf[..n], true);
                }
            }
        }
    }
    if let Some(s) = scanner {
        let rest = s.flush();
        deliver_repl_output(shared, is_err, &rest, false);
    }
}

/// Build a single-shot marker scanner for one stream, or `None` if `marker` is
/// empty (disabled).
fn single_marker_scanner(marker: Vec<u8>) -> Option<MarkerScanner> {
    (!marker.is_empty())
        .then(|| MarkerScanner::new(Markers::new(vec![(marker, Outcome::End)]), false))
}

fn ready_scanner_for(shared: &Shared, is_err: bool) -> Option<MarkerScanner> {
    let marker = if is_err {
        shared.ready_marker_stderr.clone()
    } else {
        shared.ready_marker_stdout.clone()
    };
    single_marker_scanner(marker)
}

fn warmup_scanner_for(shared: &Shared, is_err: bool) -> Option<MarkerScanner> {
    let marker = if is_err {
        shared.warmup_marker_stderr.clone()
    } else {
        shared.warmup_marker_stdout.clone()
    };
    single_marker_scanner(marker)
}

/// Build the continuous end-of-response scanner for one stream (framed mode
/// only; raw mode is a plain forwarder).
fn response_scanner_for(shared: &Shared, is_err: bool) -> Option<ResponseScanner> {
    if !shared.protocol {
        return None;
    }
    let (markers, strip) = if is_err {
        (
            Markers::new(vec![
                (shared.end_marker_stderr.clone(), Outcome::End),
                (shared.error_marker_stderr.clone(), Outcome::Error),
            ]),
            shared.strip_err,
        )
    } else {
        (
            Markers::new(vec![
                (shared.end_marker_stdout.clone(), Outcome::End),
                (shared.error_marker_stdout.clone(), Outcome::Error),
            ]),
            shared.strip_out,
        )
    };
    Some(ResponseScanner::new(markers, strip))
}

/// Watch a boot-phase chunk for the ready marker. Once the REPL is ready (via
/// this stream, the other stream, or a time-based mechanism) the scanner is
/// dropped so later output is not re-scanned.
fn detect_ready(shared: &Shared, ready_scanner: &mut Option<MarkerScanner>, chunk: &[u8]) {
    if ready_scanner.is_none() {
        return;
    }
    if !shared.is_booting() {
        *ready_scanner = None;
        return;
    }
    if let Some(rs) = ready_scanner.as_mut() {
        if let Feed::Done { .. } = rs.feed(chunk) {
            shared.signal_ready();
            *ready_scanner = None;
        }
    }
}

/// Watch a warmup-phase chunk for the warmup end marker. Only scans once the
/// warmup input has been sent, so boot output seen beforehand cannot match early.
fn detect_warmup(shared: &Shared, warmup_scanner: &mut Option<MarkerScanner>, chunk: &[u8]) {
    if warmup_scanner.is_none() || !shared.warmup_is_sent() {
        return;
    }
    if let Some(ws) = warmup_scanner.as_mut() {
        if let Feed::Done { .. } = ws.feed(chunk) {
            shared.warmup_done();
            *warmup_scanner = None;
        }
    }
}

fn deliver_repl_output(shared: &Shared, is_err: bool, bytes: &[u8], to_client: bool) {
    if bytes.is_empty() {
        return;
    }
    if is_err {
        shared.route_err(bytes, to_client);
    } else {
        shared.route_out(bytes, to_client);
    }
}

fn join_threads(threads: &mut Vec<JoinHandle<()>>) {
    for t in threads.drain(..) {
        let _ = t.join();
    }
}

fn backoff_for(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let shift = (failures - 1).min(5);
    let ms = (100u64.saturating_mul(1u64 << shift)).min(2000);
    Duration::from_millis(ms)
}

// ---- bind resolution & names -------------------------------------------------

fn resolve_bind(bind: Bind, cmdname: &str, pid: u32) -> AppResult<(Bind, Option<PathBuf>)> {
    match bind {
        Bind::Tcp { addr, port } => Ok((Bind::Tcp { addr, port }, None)),
        Bind::Unix { path } => {
            let path = path.unwrap_or_else(|| names::default_socket_path(cmdname, pid));
            remove_stale_socket(&path);
            Ok((
                Bind::Unix {
                    path: Some(path.clone()),
                },
                Some(path),
            ))
        }
    }
}

#[cfg(unix)]
fn remove_stale_socket(path: &std::path::Path) {
    use std::os::unix::fs::FileTypeExt;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_socket() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(not(unix))]
fn remove_stale_socket(_path: &std::path::Path) {}

fn describe(bind: &Bind) -> String {
    match bind {
        Bind::Tcp { addr, port } => format!("{addr}:{port}"),
        Bind::Unix { path } => path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

fn write_pidfile(
    spec: Option<&PidfileSpec>,
    cmdname: &str,
    pid: u32,
) -> AppResult<Option<PathBuf>> {
    let path = match spec {
        None => return Ok(None),
        Some(PidfileSpec::Default) => names::default_pidfile_path(cmdname, pid),
        Some(PidfileSpec::Path(p)) => p.clone(),
    };
    std::fs::write(&path, format!("{pid}\n")).map_err(|e| {
        AppError::failure(format!("cannot write pidfile '{}': {e}", path.display()))
    })?;
    Ok(Some(path))
}

fn cleanup(
    guard: &platform::ReplGuard,
    socket: Option<&std::path::Path>,
    pidfile: Option<&std::path::Path>,
) {
    guard.terminate();
    if let Some(p) = socket {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = pidfile {
        let _ = std::fs::remove_file(p);
    }
}

// ---- UTC timestamp (std-only) ------------------------------------------------

fn format_utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}_{hour:02}-{min:02}-{sec:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn epoch_is_formatted() {
        assert_eq!(format_utc(0), "1970-01-01_00-00-00");
    }

    #[test]
    fn known_timestamp() {
        assert_eq!(format_utc(1_779_031_421), "2026-05-17_15-23-41");
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_for(0), Duration::ZERO);
        assert_eq!(backoff_for(1), Duration::from_millis(100));
        assert_eq!(backoff_for(2), Duration::from_millis(200));
        assert_eq!(backoff_for(5), Duration::from_millis(1600));
        assert_eq!(backoff_for(50), Duration::from_millis(2000));
    }

    // ---- state machine ------------------------------------------------------

    #[test]
    fn idle_accepts_then_rejects_second() {
        let mut c = Core::new(true, false, now());
        assert!(matches!(c.phase, Phase::Idle));
        assert_eq!(c.try_accept(false), Some(1));
        assert!(c.served);
        assert_eq!(c.requests, 1);
        // a sink is now present -> reject
        assert_eq!(c.try_accept(true), None);
    }

    #[test]
    fn booting_accepts_but_stays_booting() {
        let mut c = Core::new(false, false, now());
        assert!(c.is_booting());
        assert!(c.try_accept(false).is_some());
        assert!(
            c.is_booting(),
            "accepting a client must not end the boot phase"
        );
    }

    #[test]
    fn request_response_cycle() {
        let mut c = Core::new(true, false, now());
        c.request_start(true);
        assert!(matches!(c.phase, Phase::Busy));
        c.response_end();
        assert!(matches!(c.phase, Phase::Idle));
    }

    #[test]
    fn raw_mode_never_busy() {
        let mut c = Core::new(true, false, now());
        c.request_start(false);
        assert!(matches!(c.phase, Phase::Idle));
    }

    #[test]
    fn client_gone_while_busy_drains_and_blocks() {
        let mut c = Core::new(true, false, now());
        c.request_start(true);
        c.client_gone(Some(now()));
        assert!(matches!(c.phase, Phase::Draining { deadline: Some(_) }));
        // Draining rejects new clients even though no sink is attached.
        assert_eq!(c.try_accept(false), None);
        // the REPL finishing on its own returns to Idle.
        c.response_end();
        assert!(matches!(c.phase, Phase::Idle));
    }

    #[test]
    fn client_gone_while_idle_keeps_idle() {
        let mut c = Core::new(true, false, now());
        c.client_gone(Some(now()));
        assert!(matches!(c.phase, Phase::Idle));
    }

    #[test]
    fn ready_only_transitions_from_booting() {
        let mut c = Core::new(false, false, now());
        assert!(c.ready(false, now()));
        assert!(matches!(c.phase, Phase::Idle));
        assert!(
            !c.ready(false, now()),
            "ready is a no-op once already ready"
        );
    }

    #[test]
    fn ready_enters_warmup_when_configured() {
        let mut c = Core::new(false, false, now());
        assert!(c.ready(true, now()));
        assert!(matches!(c.phase, Phase::Warmup { .. }));
        assert!(c.is_pre_serving(), "warmup is not yet servable");
        // clients may connect during warmup but not be served
        assert!(c.try_accept(false).is_some());
        assert!(matches!(c.phase, Phase::Warmup { .. }));
        // warmup completing makes it servable
        assert!(c.warmup_done());
        assert!(matches!(c.phase, Phase::Idle));
        assert!(!c.warmup_done(), "warmup_done is a no-op once past warmup");
    }

    #[test]
    fn begin_booting_resets_served() {
        let mut c = Core::new(true, false, now());
        c.try_accept(false);
        assert!(c.served);
        c.begin_booting(false, false, now());
        assert!(!c.served);
        assert!(c.is_booting());
    }
}
