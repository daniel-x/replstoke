# replstoke 0.1.0 — Implementation Plan

This document describes how we will implement `replstoke` as specified in `SPEC.md`.
It is the blueprint for the actual coding work that follows. It does not change the
specification; where the spec leaves something open, the choice made here is marked
as a **Decision**.

## 1. Guiding constraints (from the spec)

- **Language:** Rust.
- **No third-party crates.** Standard library only. OS-provided system libraries
  (libc on Unix/Mac, kernel32 on Windows) are reached through hand-written `extern`
  FFI blocks — this does not count as a third-party dependency.
- **Targets:** Linux, macOS, Windows.
- **Fast and lightweight.** Minimal overhead, no async runtime, no allocation-heavy
  hot paths. Forwarding is byte-exact.
- **Transports:** Unix domain sockets and TCP sockets.

### Consequences of "std only"

| Need | std provides | Gap → how we fill it |
|------|--------------|----------------------|
| TCP listener/stream | `std::net` | — |
| Unix socket listener/stream | `std::os::unix::net` | — (Unix/Mac only) |
| Spawn child process, piped stdio | `std::process` | — |
| Threads, channels, sync | `std::thread`, `std::sync` | — |
| Duplicate a socket handle for 2-way use | `TcpStream::try_clone` / `UnixStream::try_clone` | — |
| `poll`/`select` over many fds | **not available** | thread-per-direction blocking I/O |
| Signals (install handler, send signal) | **not available** | FFI to libc |
| Process groups, kill, liveness | **not available** | FFI to libc |
| Windows job objects, console ctrl, TerminateProcess | partial via `CommandExt::creation_flags` | FFI to kernel32 |

**Decision — concurrency model:** blocking I/O with one thread per data direction.
This is the only std-only option that avoids busy polling and keeps the forwarding
code simple. No `select`/`poll`, no async.

## 2. Crate layout

A single crate split into a **library** plus a **thin binary**, so unit and
integration tests can drive the logic directly.

```
Cargo.toml            # edition 2021, no dependencies, [lib] + [[bin]]
src/
  main.rs             # parse argv -> dispatch by mode; map errors to exit codes
  lib.rs              # re-exports; pub fn run(args) for tests
  cli/
    mod.rs            # arg tokenizer + Config builder + validation
    config.rs         # typed Config structs/enums
    help.rs           # the --help text and --version string
  marker.rs           # streaming end-of-response marker scanner
  names.rs            # default socket/pidfile names, <cmdname>, glob discovery
  transport.rs        # Listener/Stream abstraction over TCP and Unix
  repl.rs             # spawn + supervise the REPL child process
  server.rs           # server-mode runtime
  client.rs           # client-mode runtime
  general.rs          # kill / help / version
  platform/
    mod.rs            # trait-like facade, cfg-selected
    unix.rs           # signals, kill, process group, liveness (libc FFI)
    windows.rs        # job object, ctrl handler, kill, liveness (kernel32 FFI)
tests/
  cli.rs              # arg parsing & validation (unit-style, via lib)
  marker.rs           # marker scanner edge cases
  e2e_unix.rs         # spawn binary as server over dummyrepl.py, drive client
dummyrepl.py          # already present; used by tests, not shipped
```

**Decision — `dummyrepl.py`** stays in the repo for integration tests only; it is
excluded from any release artifact and never referenced by `--help`.

## 3. Data model

Mode is determined first, then options are validated against the mode.

```rust
enum Config {
    Server(ServerConfig),
    Client(ClientConfig),
    General(GeneralAction),
}

enum Bind {                 // shared shape for server (listen) and client (connect)
    Tcp { addr: String, port: u16 },   // default 127.0.0.1:44556
    Unix { path: Option<PathBuf> },    // None => default (server) / discover (client)
}

struct ServerConfig {
    repl_argv: Vec<OsString>,   // from -e / --  (argv[0] = executable)
    restart: bool,
    bind: Bind,
    pidfile: Option<PidfileSpec>,   // None=off, Some(Default|Path)
    greeting: bool,
}

struct ClientConfig {
    bind: Bind,
    arginput: Option<Vec<u8>>,
    fileinput: Option<FileInput>,   // Path | Stdin
    suffix: Vec<u8>,                // default empty
    marker: Vec<u8>,                // platform default
    strip_marker: bool,
}

enum GeneralAction { Help, Version, Kill(Option<PathBuf>) }
```

**Decision — option/value bytes:** `-i`, `-x`, `-m` values are taken as raw bytes
(`OsString`/`Vec<u8>`), not validated as UTF-8, so binary markers and inputs work.
On Unix this is exact; on Windows argv is UTF-16 → lossy for non-UTF-8, accepted.

## 4. CLI parsing (`cli/`)

Hand-written tokenizer (no getopt). Rules, fixed and documented:

- Long: `--name`, `--name=VALUE`. For options whose value is **optional**
  (`-a`, `-u`, `-d`, `-k`, `-g`), a value is only taken in the `=VALUE` form; a
  following separate token is **not** consumed.
- Short: `-x`, `-xVALUE`, and `-x VALUE` for **required**-value options
  (`-e` is special, `-p`, `-i`, `-f`, `-m`). Optional-value short options take a
  value only when attached (`-a127.0.0.1`).
- `-e` / `--exec` and the bare `--` **stop option parsing**: every remaining token
  is appended verbatim to `repl_argv`. They are mutually the same; `-e` must be last.
- Unknown option → fail fast with `error: unknown option '--foo'`.

Pipeline: `tokenize(argv) -> raw flags -> determine mode -> validate -> Config`.

Validation produces precise messages, e.g.
`error: --pidfile is not allowed when running in client mode`,
`error: -s and -c cannot be combined`,
`error: -p is only allowed together with -a`,
`error: -a and -u cannot be combined`.
`--help`/`--version` short-circuit before mode validation.

## 5. Marker scanner (`marker.rs`)

Streaming search so a marker that straddles two reads is still found, and so that
under `--strip-marker` the marker bytes are never emitted.

```rust
struct MarkerScanner { marker: Vec<u8>, hold: Vec<u8> /* <= marker.len()-1 */ }
enum Feed { More(Vec<u8> emit), Done(Vec<u8> emit_incl_or_excl_marker) }
```

Algorithm: append the held tail + new chunk; scan for the marker (naive search is
fine for short markers). Everything safely before any possible marker prefix is
emitted immediately; the trailing bytes that could begin a marker are held back.
On a full match: emit up to the match, include or drop the marker per
`strip_marker`, return `Done`. Unit-tested with split points inside the marker,
multiple partial-then-fail prefixes, marker at EOF, empty marker disallowed.

## 6. Names & discovery (`names.rs`)

- `<cmdname>` = file name of `repl_argv[0]` without directory.
- Default socket: `./.replstoke_socket_<cmdname>_pid<pid>` (`pid` = own pid via
  `std::process::id()`).
- Default pidfile: `./.replstoke_process_id_<cmdname>_pid<pid>`.
- Client unix discovery: glob `./.replstoke_socket_*` ourselves (read the current
  dir, match prefix); exactly one → use it, zero/many → error.
- Kill discovery: glob `./.replstoke_process_id_*`; exactly one → use it, many → error.

(Globbing is a manual `read_dir` + prefix match — no glob crate.)

## 7. Transport (`transport.rs`)

Thin enum wrapper so server/client code is transport-agnostic.

```rust
enum Listener { Tcp(TcpListener), Unix(UnixListener) }
enum Stream   { Tcp(TcpStream),   Unix(UnixStream) }
impl Stream { fn try_clone(&self) -> io::Result<Stream>; }  // for 2-way threads
```

`Listener::bind(&Bind)`, `Listener::accept() -> Stream`, `Stream::connect(&Bind)`.
On Windows the `Unix` arms compile only if `AF_UNIX` is usable; otherwise selecting
a unix transport fails fast (per spec, no silent TCP fallback).

## 8. REPL process management (`repl.rs` + `platform/`)

Spawn with `std::process::Command`, `stdin/stdout/stderr = piped()`.

Platform setup at spawn:
- **Unix:** `pre_exec` calls `setpgid(0,0)` (libc FFI) so the REPL is in its own
  process group → clean group teardown on server exit.
- **Windows:** `creation_flags(CREATE_NEW_PROCESS_GROUP)`; create a job object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and `AssignProcessToJobObject`. The server
  holds the job handle, so REPL dies with the server even on a hard kill.

A dedicated **`repl_wait` thread** blocks on `child.wait()`; its return is the
single source of truth for "REPL ended" → triggers restart or shutdown.

## 9. Server runtime (`server.rs`)

### Threads

| Thread | Source → Sink |
|--------|---------------|
| main | accept loop + lifecycle |
| `repl_out` | REPL stdout → active client, else server stdout |
| `repl_err` | REPL stderr → server stderr, **and** active client if present |
| `client_in` (per active client) | client socket → REPL stdin |
| `repl_wait` | `child.wait()` → restart/shutdown trigger |
| `signal` (Unix) | `sigwait` → shutdown trigger |

### Shared state

```rust
struct ClientLink { sink: Stream /* try_clone'd write handle */, gen: u64 }
type ActiveClient = Arc<Mutex<Option<ClientLink>>>;
let shutdown = Arc<AtomicBool>;
```

`repl_out`/`repl_err` route each chunk by reading `ActiveClient` under the lock,
cloning the sink handle (cached by `gen` to avoid a dup per chunk), then writing
**outside** the lock so a slow client can't block the lock. When `None`, `repl_out`
writes to the server's own stdout (this covers the pre-first-client banner and the
"discard for clients / show on server stdout" rule).

### Accept / single-client policy

```
loop {
  if shutdown { break }
  conn = accept (non-blocking listener + short poll, see Shutdown)
  if greeting { best-effort write greeting line to conn }
  if active.is_some() {
      drop(conn)            // reject: closed immediately (after greeting if any)
  } else {
      active = Some(link from conn.try_clone()); gen += 1
      spawn client_in(conn -> repl stdin); on EOF/err it sets active=None
  }
}
```

**Decision — greeting line** is sent to *every* connection (including the one that
becomes active), matching "send a greeting line to every client upon connection".
The format uses `bindaddr:port` for TCP and the socket path for Unix.

### REPL exit

`repl_wait` returns →
- `--restart`: drop current REPL pipes (its `repl_out`/`repl_err` see EOF and end),
  disconnect any active client, respawn REPL, restart the three REPL threads.
- no `--restart`: set `shutdown`, clean up, exit.

### Shutdown & cleanup

`shutdown` is set by: `repl_wait` (no restart), the signal/ctrl handler, or a fatal
error. **Decision — waking a blocked `accept()`:** the listener is set non-blocking
and the accept loop polls with a ~50 ms sleep while checking `shutdown`. This is
std-only, needs no self-pipe/self-connect trick, and costs negligible CPU.

Cleanup (idempotent, run once): terminate REPL (close its stdin, then group/job
teardown), remove the Unix socket file if we created one, remove the pidfile if we
wrote one.

## 10. Client runtime (`client.rs`)

1. Resolve `Bind` (TCP addr:port, explicit unix path, or unix discovery).
2. `connect`; failure → exit non-zero with a clear message.
3. **Send input, in order, byte-exact, no added newlines:**
   `arginput (-i)` → `fileinput (-f, streamed in chunks; '-' = stdin)` → `suffix (-x)`.
4. **Do not** half-close the write side (a half-close would make the server's
   `client_in` see EOF and drop us). Keep the socket fully open and start reading.
5. Read loop → feed bytes to `MarkerScanner` → write emitted bytes to stdout
   (honoring `--strip-marker`). On `Done`: flush stdout, close socket, exit 0.
6. If the server closes before the marker is seen: flush whatever was received,
   exit non-zero.

A separate sender thread is used so sending a large `-f` stream and reading the
response proceed concurrently (full-duplex, avoids deadlock if the REPL responds
mid-input).

## 11. General mode (`general.rs`)

- `--help` → print the help text (same content as the spec Synopsis), exit 0.
- `--version` → print `replstoke 0.1.0`, exit 0.
- `--kill` → resolve pidfile (explicit or discover single `./.replstoke_process_id_*`;
  many → error), read pid, then `platform::terminate(pid, timeout)`:
  - **Unix:** `kill(pid, SIGTERM)`; poll liveness with `kill(pid, 0)` up to the
    timeout; if still alive `kill(pid, SIGKILL)`.
  - **Windows:** `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)`; wait up to the
    timeout; if still alive `OpenProcess` + `TerminateProcess`.
  - If the process is already gone and the pidfile remains, delete the pidfile.

## 12. Platform FFI surface

Hand-written `extern` declarations, no `libc`/`windows` crate.

- **Unix (libc):** `setpgid`, `kill`, `sigemptyset`/`sigaddset`/`pthread_sigmask`,
  `sigwait`. **Decision — signals:** mask `SIGTERM`/`SIGINT` in all threads at
  startup and dedicate one `signal` thread to `sigwait`; it then runs the same
  cleanup path as a normal shutdown. This avoids async-signal-safety pitfalls of
  in-handler logic entirely.
- **Windows (kernel32):** `CreateJobObjectW`, `SetInformationJobObject`,
  `AssignProcessToJobObject`, `SetConsoleCtrlHandler`, `GenerateConsoleCtrlEvent`,
  `OpenProcess`, `TerminateProcess`, `GetExitCodeProcess`, `WaitForSingleObject`.

`platform/mod.rs` exposes a uniform API (`spawn_repl`, `install_shutdown`,
`terminate`, `is_alive`) and `#[cfg]`-selects the implementation.

## 13. Error handling & exit codes

- `0` success.
- non-zero on: invalid options; REPL executable not found / not startable; client
  cannot connect; `--kill` cannot find/terminate a server; client connection closed
  before the marker was seen.
- All user-facing errors are single lines prefixed with `error:` on stderr, naming
  the exact cause. Internal `Result`-based flow; `main` maps the top-level error to
  a message + exit code. No panics on expected error paths.

## 14. Testing strategy (TDD — tests precede code per phase)

- **Unit (`cargo test`, via `lib`):**
  - CLI: mode detection; mutually-exclusive flags; per-mode option validity;
    `-e`/`--` capture; `=VALUE` vs separate-token rules; unknown option.
  - Marker scanner: split markers, partial prefixes, strip on/off, marker at EOF.
  - Names: `<cmdname>` extraction, default paths, single/zero/many discovery.
- **Integration (`tests/e2e_unix.rs`):** uses `env!("CARGO_BIN_EXE_replstoke")` to run
  the real binary as a server wrapping `dummyrepl.py`, then runs clients:
  - single request/response with default `"\n\n"` marker;
  - `-i` + `-f` ordering; `-x` suffix; `-t` strip-marker;
  - second concurrent client rejected; `-g` greeting line;
  - `-k` via pidfile; `--restart` after the REPL exits;
  - both TCP and Unix transports.
- **Windows:** the platform layer is structured so the same e2e tests run on a
  Windows CI runner (TCP transport; Unix socket best-effort). Job-object reaping and
  ctrl-break kill get dedicated Windows-only tests.
- **CI:** matrix on linux/macos/windows: `cargo build`, `cargo test`, `cargo clippy
  -D warnings`, `cargo fmt --check`.

## 15. Implementation phases

Each phase: write tests first, then code, keep `main` runnable throughout.

0. **Scaffold** — Cargo.toml (no deps), lib+bin split, CI skeleton.
1. **CLI** — tokenizer, Config, validation, `--help`/`--version`. *(unit tests)*
2. **Marker + names** — scanner and path/discovery utilities. *(unit tests)*
3. **Transport** — TCP + Unix Listener/Stream, `try_clone`. *(loopback test)*
4. **REPL spawn (Unix)** — Command + setpgid + `repl_wait`.
5. **Server core (Unix)** — forwarding threads, single-client policy, idle drain,
   greeting, non-blocking accept + shutdown, restart.
6. **Client core** — input ordering, streaming `-f`, marker, strip, exit codes.
7. **Signals + pidfile + kill (Unix)** — sigwait thread, cleanup, `-k`.
8. **Integration tests (Unix)** — full e2e against dummyrepl.py.
9. **Windows platform layer** — job object, ctrl handler, kill, `#[cfg]` wiring.
10. **Polish** — docs, help text parity with spec, clippy/fmt, cross-platform CI.

Phases 0–8 deliver a fully working Unix/Mac tool; 9–10 complete Windows and finish.

## 16. Accepted simplifications & open risks (from the spec)

- stdout/stderr from the REPL are forwarded to the client on the same channel;
  stderr may interleave with and corrupt marker detection — **accepted**.
- The server never inspects whether either side is "done"; it forwards until the
  client disconnects — **accepted**.
- Pre-first-client REPL output races with connection — **accepted**.
- No access control, no encryption — **accepted**.
- Hard kill (SIGKILL / TerminateProcess) may leave a stale socket file or pidfile;
  next start and `-k` tolerate leftovers — **accepted**.

## 17. Decisions made in this plan (not pinned by the spec)

1. Blocking threads, one per direction; no async, no `select`.
2. lib + thin bin split for testability.
3. Non-blocking listener + ~50 ms poll to make `accept()` interruptible by shutdown.
4. Unix signals handled via masked threads + `sigwait` (no in-handler logic).
5. Option values are raw bytes, not required to be UTF-8.
6. Optional-value options (`-a`, `-u`, `-d`, `-k`) take a value only in attached
   form (`-aVALUE` short, `--addr=VALUE` long); a separate following token is not
   consumed (matches GNU getopt `optional_argument`, and is required for "bare =
   default" to work). Required-value options (`-p`, `-i`, `-f`, `-m`, `-x`) accept
   all three forms, including the attached short form `-xVALUE`.
7. A per-client sender thread on the client side for full-duplex large-input safety.
8. `dummyrepl.py` is test-only and never shipped or referenced by `--help`.
