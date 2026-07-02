# Server state machine

The server's control state used to live in several independent primitives
(`busy`, `ready`, `dc_deadline`, `served` plus the main-loop locals
`force_restart`, `ready_timed_out`). They encoded one coupled lifecycle whose
invariants were enforced only by careful ordering across three threads, which is
where the races and the per-feature weight came from.

This document defines that lifecycle as one explicit `Phase` enum. The data path
(thread-per-direction, blocking writes, kernel-buffer backpressure) is unchanged;
only the control plane is consolidated.

## Core structure

```rust
enum Phase {
    Booting  { since: Instant },            // spawned, not ready; a client may be attached but its input is not forwarded
    Warmup,                                 // ready, running the one-shot --warmup-input before serving; input still not forwarded
    Idle,                                   // ready, no request in flight (client may or may not be attached)
    Busy,                                   // input forwarded, awaiting end-of-response
    Draining { deadline: Option<Instant> }, // client left mid-request, REPL still finishing
    Stopped,                                // terminal
}

struct Core {                 // guarded by Mutex<Core> + Condvar
    phase: Phase,
    served: bool,                 // this REPL instance accepted >=1 client (startup-failure accounting)
    requests: u64,
    next_gen: u64,
    req_seq: u64,                 // ++ on each Idle -> Busy; pumps reset their scanner per request
    serving: bool,                // client's request has started and it is still attached; gates output delivery
}

// each written with BLOCKING I/O, so each lives in its OWN Mutex, never under the Core lock:
sink:       Mutex<Option<ClientConn>>   // the single active client's write handle (+ gen)
repl_stdin: Mutex<Option<ChildStdin>>
```

Both blocking sinks - the client socket and the REPL's stdin - live *outside*
`Core`, because holding the Core lock across a blocking `write_all` would stall
every transition. `Core` holds only lifecycle facts. The accept-gate takes both
locks briefly in one order (Core -> sink); output routing reads `phase` and then
writes the sink, never nested.

**Hard rule: the Core lock is held only to read/transition/notify - never across
socket or pipe I/O.**

## Events

| event | raised by |
|---|---|
| `Accept(sink)` | acceptor - a connection passed the gate |
| `RequestStart` | client_in - first chunk forwarded after ready (`req_seq++`) |
| `ResponseEnd` | pump - end-of-response marker seen |
| `WarmupDone` | pump (warmup marker) or supervisor (`--warmup-wait` elapsed); ignored until the warmup input is sent |
| `ClientGone` | client_in - client socket closed/errored |
| `ReplReady` | pump (ready marker) or supervisor (`--ready-wait` elapsed) |
| `ReplStuck` | supervisor - `--ready-marker-timeout` elapsed while Booting |
| `DrainTimeout` | supervisor - Draining deadline elapsed |
| `ReplExited{stuck, forced}` | supervisor - child reaped |
| `Shutdown` | signal flag / `-k` / fatal error |

`ReplReady` targets `Warmup` when a `--warmup-input` is configured, else `Idle`. With no
readiness mechanism the REPL starts directly in `Warmup` (if a warmup is set) or
`Idle`, skipping `Booting`.

## Transitions

Only meaningful pairs; anything omitted is a no-op in that phase.

| Phase | Event | -> Phase | Actions |
|---|---|---|---|
| **Booting** | `Accept` | Booting | if `sink==None`: store sink, `served=true`, spawn client_in; else reject |
| | `ReplReady` | **Warmup** / **Idle** | Warmup if `--warmup-input` set, else Idle; notify condvar (wakes a client_in parked before its read) |
| | `ReplStuck` | Booting¹ | `terminate()`; stamp exit `stuck` -> resolved by `ReplExited` |
| | `ClientGone` | Booting | clear `sink` (no deadline - no request was in flight) |
| | `ReplExited` | Booting / **Stopped** | see *ReplExited* |
| **Warmup** | `Accept` | Warmup | accept, defer forwarding, exactly like Booting |
| | `WarmupDone` | **Idle** | supervisor already sent the warmup once on entry; notify condvar |
| | `ClientGone` | Warmup | clear `sink` |
| | `ReplExited` | Booting / Stopped | drop `client`; see *ReplExited* |
| **Idle** | `Accept` | Idle | if `sink==None`: store sink, `served=true`, spawn client_in; else reject |
| | `RequestStart` | **Busy** | (framed mode only) |
| | `ClientGone` | Idle | clear `client` |
| | `ReplExited` | Booting / Stopped | drop `client`; see *ReplExited* |
| **Busy** | `ResponseEnd` | **Idle** | request done |
| | `ClientGone` | **Draining**{d} | `d = response_timeout.map(now + _)`; clear sink, keep REPL reserved |
| | `ReplExited` | Booting / Stopped | drop `client`; see *ReplExited* |
| **Draining** | `ResponseEnd` | **Idle** | REPL finished on its own |
| | `DrainTimeout` | Draining¹ | `terminate()`; stamp exit `forced` -> resolved by `ReplExited` |
| | `ReplExited` | Booting / Stopped | see *ReplExited* |
| **any** | `Shutdown` | **Stopped** | terminate REPL, notify condvar, cleanup |

¹ `ReplStuck`/`DrainTimeout` do not move the phase directly - they kill the REPL
and stamp the *reason*; the ensuing `ReplExited` performs the phase move, so
"process died" handling stays in one place.

### `ReplExited{stuck, forced}` resolution

```
do_restart = cfg.restart || forced
if !do_restart                       -> Stopped        [ctl error "REPL exited"]
startup_failure = stuck || (!served && since < GRACE)
if ++consecutive >= MAX              -> Stopped        [ctl error "repeatedly failed to start"]
else respawn; served=false           -> initial phase   [notify condvar]   (backoff first)
                                       (Booting, else Warmup, else Idle - see initial_phase)
```

`forced` replaces the old `force_restart`; `stuck` replaces `ready_timed_out`.
Both now have a lifetime of one exit (message payloads), not standing flags.

## Derived gates

The scattered `if busy || !ready || ...` checks become predicates on `phase`.
`pre_serving = matches!(phase, Booting | Warmup)`:

| decision | predicate |
|---|---|
| accept a new client | `sink.is_none() && matches!(phase, Booting \| Warmup \| Idle)` |
| forward client input | wait until `!pre_serving` (and not stopping); write when `repl_stdin.is_some()` |
| deliver REPL output to the client | whenever `serving` (the client's first request has started and it has not disconnected), across `Busy`, `Idle`, and `Draining`; boot/warmup output and any remnant seen while `!serving` goes to the server's std streams |
| scan a response | fed only in `Busy \| Draining`, reset fresh when `req_seq` advances, so no response's held bytes bleed into the next |
| a marker means "done" | in `Busy \| Draining` -> `ResponseEnd`; in `Warmup` -> `WarmupDone` |

`serving` (a `Core` flag: set on the first `RequestStart`, cleared on `ClientGone`,
`Accept`, and respawn) decouples output delivery from the `Busy`/`Idle` phase. The
server's own end marker fires `ResponseEnd` (`Busy` -> `Idle`) only to decide when
the *next* client may be served; it must not stop feeding the currently-connected
one. Because stdout and stderr are independent pipes, a marker on one stream can be
observed before the tail of the response is drained on the other; delivering across
`Idle` while `serving` keeps that tail flowing to the client instead of dropping it.
Withholding whenever `!serving` still keeps boot banners and warmup output
race-proof (the warmup's output may be processed after the phase flips, but no
client is being served then) and prevents one client's tail from bleeding into the
next (its `serving` clears the instant it disconnects).

## Flag mapping

| old | new |
|---|---|
| `busy: AtomicBool` | `phase in {Busy, Draining}` |
| `ready: AtomicBool` | `phase not in {Booting, Warmup}` (i.e. `!pre_serving`) |
| `dc_deadline: Mutex<Option<Instant>>` | `Draining{deadline}` payload |
| `served: AtomicBool` | `Core.served` |
| `force_restart` (local) | `ReplExited{forced}` payload |
| `ready_timed_out` (local) | `ReplExited{stuck}` payload |
| `client: Mutex<..>` | `sink: Mutex<..>` (separate lock; blocking writes) |
| `repl_stdin`, `next_gen`, `requests` | unchanged in role |

## Concurrency

`Mutex<Core> + Condvar`. Waiters (a client_in parked before its read; a forward
parked on a respawn gap) are woken by `ReplReady`, respawn (new `repl_stdin`), and
`Shutdown`, which all `notify_all()`. Because the shutdown signal handler cannot
notify a condvar (not async-signal-safe), waits use `wait_timeout` with a coarse
backstop poll of the shutdown flag.

## Residuals (accepted)

1. The condvar wait keeps a coarse timeout for the signal-driven shutdown flag.
2. "No blocking I/O under the Core lock" is a discipline, enforced by keeping the
   only blocking sinks (`repl_stdin`, the client socket) written outside the lock.
3. On `ReplExited` the dropped client's `client_in` thread is unblocked by
   shutting down its socket, so it unwinds promptly instead of limping.
