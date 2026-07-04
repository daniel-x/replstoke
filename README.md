# replstoke - keep a REPL warm, call it like a one-shot tool

## tl;dr

Many ai agents can't handle long-running REPL tools well (REPL = Read-Eval-Print-Loop). replstoke
solves this, e.g. to make a long-running Lean proof assistant accessible for an ai agent without
rebooting Lean.

Use it like this:

```sh
# 1. start server that boots a REPL subprocess, here python3 is the REPL:
replstoke --server -d -u -e /usr/bin/python3 -i -u
```

```sh
# 2. connect with a client to the repl:
replstoke --client --strip-marker-stdout -i $'print(3*6)\nprint()\n'
```

## Details

Wrap a long-running REPL (or any interactive `stdin`/`stdout` program) in a small
server, and talk to it from short-lived client invocations as if it were a
one-shot batch tool. The REPL stays warm between requests, so its slow startup
(loading Mathlib, importing a big Python environment, JIT warmup, …) is paid
once.

`replstoke` is a single, dependency-free binary. It is glue, not a public
network service: the same person normally runs both the server and the clients.
It does no access control and no encryption.

The `--help` output is a cheat sheet. **This file is the usage reference.** For
the wire format see [PROTOCOL.md](PROTOCOL.md); for the design specification see
[SPEC.md](SPEC.md).

## Install

Prebuilt binaries are attached to each tagged
[GitHub Release](https://github.com/daniel-x/replstoke/releases), built by CI on
native runners for:

| OS      | x86-64                     | ARM64                                    |
|---------|----------------------------|------------------------------------------|
| Linux   | `x86_64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu`              |
| Windows | `x86_64-pc-windows-msvc`   | `aarch64-pc-windows-msvc`                |
| macOS   | `x86_64-apple-darwin` (Intel) | `aarch64-apple-darwin` (Apple silicon) |

Download the archive for your platform, verify it against the accompanying
`.sha256`, and put the `replstoke` binary on your `PATH`. Or build from source.

## Build

```sh
cargo build --release        # -> target/release/replstoke
```

## Quick start

```sh
# server: keep python3 warm on a unix socket, write a pidfile
replstoke --server -d -u -e /usr/bin/python3 -i

# client: send one expression, read the response up to a sentinel
replstoke --client -i $'print(3*6)\n' -x $'print("__END__")\n' -m '__END__'

# stop the server
replstoke -k
```

## Modes

`replstoke` runs in exactly one mode, selected by the flags present:

| mode    | selected by               |
|---------|---------------------------|
| server  | `-s`, `--server`          |
| client  | `-c`, `--client`          |
| general | neither (e.g. `-k`, `-h`) |

`-s` and `-c` cannot be combined.

A server serves **one client at a time**. While it is still handling one
request, another client that connects is refused; it should retry.

`replstoke` forwards input and output **byte-exactly** - it never inserts,
removes, or rewrites bytes (the only exception is an explicitly requested marker
strip). In particular it adds no trailing newline, so if your REPL needs one to
evaluate a line, include it in the input (e.g. `-i $'print(3*6)\n'`).

## Server

```
replstoke --server [transport] [-r] [-d[=PIDFILE]] [--raw] [detection] -e CMD [ARG]...
```

- `-e, --exec CMD [ARG]...` - the REPL command. Everything after `-e` is its
  argv (`argv[0]` is the executable, resolved via `PATH`). `-e` must be the last
  replstoke option; `--` is an alias. The argv is passed straight to the OS with
  no shell interpretation. The REPL should run interactively and unbuffered (or
  at least line-buffered) - e.g. `python3 -i -u`. If the executable can't be
  found, the server exits before binding the socket.
- `-r, --restart` - respawn the REPL whenever its process exits.
- Transport (see below): `-a`/`-p` for TCP, `-u` for a unix socket.
- `-d, --pidfile[=PATH]` - write the server pid to a file and remove it on exit.
  Without a value, a default name is used.
- `--raw` - disable framing; the server becomes a plain byte forwarder with the
  REPL's stderr merged into the client stream. The client must also use `--raw`.
- Detection options (`--end-marker-stdout`, `--end-marker-stderr`,
  `--error-marker-stdout`, `--error-marker-stderr`, `--strip-marker-stdout`,
  `--strip-marker-stderr`, `--response-timeout`) - see
  *End-of-response detection* and *Behaviour* below.
- Readiness options (`--ready-marker-stdout`, `--ready-marker-stderr`,
  `--ready-wait`, `--ready-marker-timeout`) and `--warmup-input` - see *Readiness*
  below.

## Client

```
replstoke --client [transport] [input] [boundary] [--ctl=MODE] [--raw]
```

Input (sent to the REPL in this order):

- `-i, --arginput=DATA` - literal bytes.
- `-f, --fileinput=PATH` - bytes streamed from a file; `-` means stdin. Sent
  after `--arginput`.
- `-x, --suffix=DATA` - bytes sent after all input (e.g. a newline, or a command
  that makes the REPL print a sentinel).

Reading the response - see *End-of-response detection*:

Each stream (stdout, stderr) can be watched for two markers: an **end** marker
that signals a normal (successful) end of response, and an **error** marker that
signals a failed one. Whichever appears first ends the response; an error marker
makes the client exit non-zero. Any marker not given (empty) is disabled.

- `-m, --end-marker-stdout=MARKER` - success end-of-response marker on stdout.
  Default: `\n\n` (`\r\n\r\n` on Windows).
- `--end-marker-stderr=MARKER` - success end-of-response marker on stderr.
  Default: disabled.
- `--error-marker-stdout=MARKER` - error end-of-response marker on stdout; a
  match makes the client exit non-zero. Default: disabled.
- `--error-marker-stderr=MARKER` - error end-of-response marker on stderr; a
  match makes the client exit non-zero. Default: `error`.
- `--strip-marker-stdout` / `--strip-marker-stderr` - drop the matched marker from
  the stdout / stderr output.
- `--timeout=SECONDS` - give up and exit `124` if a complete response has not
  arrived in time. Fractional seconds are allowed (e.g. `--timeout=0.12345`). By
  default the client waits indefinitely.
- `--ctl=MODE` - what to do with the server's `ctl` status messages: `ignore`
  (default), `stdout`, or `stderr`. Terminal `ctl` errors always go to stderr
  and make the client exit non-zero.
- `--raw` - read a single unframed byte stream; must match the server.

The client writes the REPL's `out` stream to its stdout and the `err` stream to
its stderr.

## Transport

Shared by both modes:

- `-a, --addr[=ADDR]` - TCP. Default `127.0.0.1`. Assumed if neither `-a` nor
  `-u` is given.
- `-p, --port=PORT` - TCP port (only with `-a`). Default `44556`.
- `-u, --unixsocket[=PATH]` - unix socket. As a server, a default path is used if
  `PATH` is omitted; as a client, a bare `-u` discovers a single
  `./.replstoke_pid*_socket` and connects to it (error if zero or many exist).

`-a` and `-u` are mutually exclusive. Unix sockets are first-class on Linux/macOS;
on Windows the portable default is TCP.

## General

- `-k, --kill[=PIDFILE]` - terminate a previously started server via its pidfile
  (SIGTERM then SIGKILL after a timeout; `CTRL_BREAK_EVENT` then
  `TerminateProcess` on Windows). With no value, a single
  `./.replstoke_pid*_pidfile` is discovered. Stale pidfiles are removed.
- `-h, --help`, `--version`.

## End-of-response detection

A REPL just produces a stream of bytes; it does not announce where one response
ends. So "the response is complete" is something you tell `replstoke` how to
recognise. The mechanisms, which compose:

- **Markers** - a known byte sequence the REPL emits at the end of a response,
  matched on stdout and/or stderr. Each stream can carry both a success (`end`)
  and a failure (`error`) marker. Works when the REPL has a stable prompt/trailer.
- **Sentinel in the input** - put a unique, unlikely token in the input (`-x`
  running a command that prints it) and match that token with `-m`. This is the
  most reliable approach: the token rides along in the input and echoes back in
  the output.
- **Timeout** - a backstop (`--timeout`), not a completion signal.

None of these is foolproof; choose the one that fits your REPL.

> **Keep the end marker on the stream that carries the response content.** A
> REPL's stdout and stderr are separate OS pipes with no recoverable cross-stream
> byte ordering, so whichever side watches for the marker can see it on one stream
> before the content lands on the other. With `python3`, results print to stdout
> but the `>>> ` prompt is written to stderr, so using `>>> ` as the end marker can
> truncate a stdout result. This cross-stream case is an accepted limitation - it
> cannot be solved for every case - so make the REPL emit its terminator on the
> **same** stream as the result: for `python3`, a trailing blank line on stdout
> (matching the default `\n\n` marker) or a sentinel via `-x`/`-m` on stdout. That
> is why the quick-start prints a blank line with `print()`.

### Client-side or server-side?

The marker, strip, and timeout options exist on **both** the client and the
server. Put them where they belong by *what they describe*:

- **Properties of the REPL** - how it signals end-of-response, and when it should
  be considered wedged - are the same for every request. Declare them **once on
  the server** (the `--end-marker-*` / `--error-marker-*` markers, `--strip-marker-*`,
  and the wedged-REPL `--response-timeout`).
- **Properties of a single call** - this invocation's deadline - belong on the
  **client** (`--timeout`).

Giving the same option on both sides is **not** the same as giving it once: each
side acts on its own copy independently.

## Readiness

Some REPLs need a moment to boot (load a runtime, print a banner) before they can
handle input. The server can wait for the REPL to become **ready** before it
serves any client. Readiness applies only during the REPL's start phase and the
identical restart phase; once ready, it plays no further part. These options are
**server-only**:

- `--ready-marker-stdout=MARKER` / `--ready-marker-stderr=MARKER` - the REPL is
  ready as soon as the marker is seen on that stream. If both are given, the
  earlier match wins.
- `--ready-wait=SECONDS` - instead of a marker, give the REPL this long after it
  is (re)spawned to boot; once the time elapses it is considered ready. Mutually
  exclusive with the ready markers.
- `--ready-marker-timeout=SECONDS` - if no ready marker arrives within this long,
  the REPL is assumed stuck and terminated (then restarted if `--restart` is set,
  otherwise the server exits). Allowed only together with a ready marker.

By default no readiness mechanism is configured and the REPL is considered ready
immediately. While a REPL is still booting, a connecting client **is accepted**,
but its input is **not forwarded** to the REPL until the REPL is ready; the REPL's
boot output goes to the server's own stdout/stderr, not to the client.

### Warmup

Warmup is a phase between *booted* and *ready*: after the REPL boots, the server
runs a one-off command in it (e.g. to preload libraries or warm a JIT) before
serving any client. During warmup there is **no client interaction** - a
connected client's input is not forwarded and it receives no output - and the
warmup's output goes to the server's own stdout/stderr, never to a client.

- `--warmup-input=DATA` (server-only) - the input sent once after boot. Byte-exact
  like all input, so include a trailing newline if the REPL needs one.
- End of warmup is detected the same way readiness is - by a marker or a wait;
  one is **required** with `--warmup-input`:
  - `--warmup-marker-stdout=MARKER` / `--warmup-marker-stderr=MARKER` -
    warmup finishes when the marker appears on that stream.
  - `--warmup-wait=SECONDS` - ...or after this long, whichever comes first.
  - `--warmup-marker-timeout=SECONDS` - if a warmup marker is set but not seen
    within this long, the REPL is assumed stuck and terminated (restarted with
    `-r`), analogous to `--ready-marker-timeout`.

A warmup that crashes or never completes leaves the REPL unservable, so keep it a
quick, reliable command. As with response detection (below), a warmup marker
is most reliable on the same stream the warmup's last output lands on; a wait
avoids marker races entirely.

## Behaviour

- **One client at a time.** A new client is refused while a request is still in
  progress - including while the server is still waiting for the REPL to finish a
  request whose client already disconnected. This keeps one client from receiving
  another's output.
- **Client disconnects mid-request.** The server lets the REPL finish. With
  `--response-timeout` set, a REPL that does not finish within it is treated as
  wedged, terminated, and restarted; without it the server waits indefinitely for
  the REPL to finish.
- **REPL exits.** Without `--restart` the server exits too. With `--restart` it
  starts a fresh REPL. A REPL that keeps dying immediately on startup makes the
  server give up rather than spin.
- **REPL momentarily gone.** While the REPL is being (re)started, forwarding of
  client input is deferred until the new REPL is available rather than the input
  being dropped.
- **REPL still booting.** With a readiness option set, a connecting client is
  accepted but its input is not forwarded until the REPL is ready (boot output stays on the
  server's std streams); a REPL that never becomes ready is torn down per
  `--ready-marker-timeout`. See *Readiness*.
- **No client connected.** REPL output is written to the server's own stdout/
  stderr. The REPL's stderr is always also written to the server's stderr.
- **Cleanup.** On a clean exit (normal, SIGTERM, or Windows `CTRL_BREAK_EVENT`)
  the server stops the REPL and removes its socket and pidfile. A forced kill may
  leave a stale socket or pidfile; the next start and `-k` both tolerate that.

## Protocol

By default the server→client stream is framed so stdout and stderr stay
separated and the server can send control messages. `--raw` (on both ends)
disables framing entirely and merges the streams. See [PROTOCOL.md](PROTOCOL.md).

## Exit codes

| code | meaning                    |
|------|----------------------------|
| 0    | success                    |
| 1    | runtime failure            |
| 2    | usage error                |
| 124  | client `--timeout` expired |

## Tests

```sh
cargo test                                  # unit + e2e (Unix uses dummyrepl.py)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`dummyrepl.py` is a tiny stub REPL used only by the test suite.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
