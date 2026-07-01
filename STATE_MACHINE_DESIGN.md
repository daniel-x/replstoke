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
    Booting  { since: Instant },            // spawned, not ready; a client may be attached but its input is held
    Idle,                                   // ready, no request in flight (client may or may not be attached)
    Busy,                                   // input forwarded, awaiting end-of-response
    Draining { deadline: Option<Instant> }, // client left mid-request, REPL still finishing
    Stopped,                                // terminal
}

struct Core {                 // guarded by Mutex<Core> + Condvar
    phase: Phase,
    client: Option<ClientConn>,   // the single active client sink (+ gen)
    served: bool,                 // this REPL instance handled >=1 request (startup-failure accounting)
    requests: u64,
    next_gen: u64,
}

// stays in its OWN Mutex, NOT in Core - blocking writes must never hold the Core lock:
repl_stdin: Mutex<Option<ChildStdin>>
```

`client` lives *in* `Core` because the accept-gate and output-routing decisions
must read/modify it atomically with `phase`. `repl_stdin` lives *outside* because
it is only ever written through (a blocking `write_all` that backpressures), so
holding the Core lock across that write would stall every transition.

**Hard rule: the Core lock is held only to read/transition/notify - never across
socket or pipe I/O.**

## Events

| event | raised by |
|---|---|
| `Accept(sink)` | acceptor - a connection passed the gate |
| `RequestStart` | client_in - first chunk forwarded after ready |
| `ResponseEnd` | pump - end-of-response marker seen |
| `ClientGone` | client_in - client socket closed/errored |
| `ReplReady` | pump (ready marker) or supervisor (`--ready-wait` elapsed) |
| `ReplStuck` | supervisor - `--ready-marker-timeout` elapsed while Booting |
| `DrainTimeout` | supervisor - Draining deadline elapsed |
| `ReplExited{stuck, forced}` | supervisor - child reaped |
| `Shutdown` | signal flag / `-k` / fatal error |

## Transitions

Only meaningful pairs; anything omitted is a no-op in that phase.

| Phase | Event | -> Phase | Actions |
|---|---|---|---|
| **Booting** | `Accept` | Booting | if `client==None`: store sink, `served=true`, spawn client_in; else reject |
| | `ReplReady` | **Idle** | notify condvar (wakes a client_in parked before its read) |
| | `ReplStuck` | Booting¹ | `terminate()`; stamp exit `stuck` -> resolved by `ReplExited` |
| | `ClientGone` | Booting | clear `client` (no deadline - no request was in flight) |
| | `ReplExited` | Booting / **Stopped** | see *ReplExited* |
| **Idle** | `Accept` | Idle | if `client==None`: store sink, `served=true`, spawn client_in; else reject |
| | `RequestStart` | **Busy** | (framed mode only) |
| | `ClientGone` | Idle | clear `client` |
| | `ReplExited` | Booting / Stopped | drop `client`; see *ReplExited* |
| **Busy** | `ResponseEnd` | **Idle** | request done |
| | `ClientGone` | **Draining**{d} | `d = (restart_on_client_dc && timeout).then(now+timeout)`; clear sink, keep REPL reserved |
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
else respawn; served=false           -> Booting{now}   [notify condvar]   (backoff first)
```

`forced` replaces the old `force_restart`; `stuck` replaces `ready_timed_out`.
Both now have a lifetime of one exit (message payloads), not standing flags.

## Derived gates

The scattered `if busy || !ready || ...` checks become predicates on `phase`:

| decision | predicate |
|---|---|
| accept a new client | `client.is_none() && matches!(phase, Booting \| Idle)` |
| forward client input | wait until `!matches!(phase, Booting \| Stopped)` and `repl_stdin.is_some()`, then write |
| route REPL output -> client | `client.is_some() && !matches!(phase, Booting)` |
| a marker means "response done" | only acts in `Busy \| Draining` |

## Flag mapping

| old | new |
|---|---|
| `busy: AtomicBool` | `phase in {Busy, Draining}` |
| `ready: AtomicBool` | `phase not in {Booting}` |
| `dc_deadline: Mutex<Option<Instant>>` | `Draining{deadline}` payload |
| `served: AtomicBool` | `Core.served` |
| `force_restart` (local) | `ReplExited{forced}` payload |
| `ready_timed_out` (local) | `ReplExited{stuck}` payload |
| `client`, `repl_stdin`, `next_gen`, `requests` | unchanged in role |

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
