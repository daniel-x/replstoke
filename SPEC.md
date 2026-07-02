# replstoke — wrap a REPL as a one-shot tool

## Goal

The goal of this session is to create a command line tool that makes it possible to use a REPL (Read-Eval-Print Loop) console tool through a so called batch tool one-shot tool. We will use the name one-shot tool henceforth. The one-shot tool shall be executable, take input, forward the input to the REPL process, and forward the output of the REPL process back to the caller of the one-shot tool. In this sense, the tool that is to be developed is an adapter between a REPL and a one-shot tool. We'll call it replstoke, because it keeps a REPL's fire stoked — warm and ready for instant one-shot calls.

## Aspects
- programming language: rust
- target systems: Linux, Windows, Mac
- no dependencies on external non-standard libraries at runtime. only depend on things that are provided by a standard OS installation.
- it shall run fast, thus create little additional overhead
- it shall be lightweight, i.e. just do its job in a minimalistic way
- support unix sockets and network sockets

## Introduction

### Batch Tools or One Shot Command Line Tools

There are tools called batch tools or one-shot tools. They have this lifecycle:
- the tool gets an input via command line argument or stdin, or both
- does processing
- writes its output to stdout
- when the tool is done with the output, it terminates

Examples are cat, echo, ls, printf, grep, sed.

### REPL tools

REPL tools (Read-Eval-Print Loop) are long-lived processes that start up and accept input via a prompt or simply accept input. The Python REPL is a classic example for this.

## Problems And Solution

Pure REPL tools are hard to use in all applications that are aimed at using one-shot tools, like other one-shot scripts or most other REPLs. Therefore it helps if a REPL tool can be made accessible as a one-shot tool. Most REPL tools also provide a way to use it as a one-shot tool, but in some contexts this causes problems, e.g. when the startup of the REPL tool takes a long time and that repeatedly causes delays, or when the one-shot tool should maintain internal state between inputs, but a pure one-shot tool cannot do this. Then bridging access to a long-running REPL tool through a one-shot tool makes things easier. When a one-shot tool exists that can redirect input to a long-running REPL process and forward the REPL's response back to the caller, then the caller can benefit of the advantages of both approaches.

In total, 4 processes will be involved when using replstoke practically:
- repl: the REPL tool itself
- replstoke server: the replstoke process that wraps the REPL tool
- user process: a process (or a user) who accesses the REPL tool in a one-shot fashion
- replstoke client: another replstoke process that is called in a one-shot fashion and which acts as a client and connects to a replstoke server

## Synopsis

replstoke 0.2.0

Usage: replstoke [OPTIONS] [-- COMMAND [ARG]...]
Wrap a long-running REPL and make it accessible as a one-shot (batch) tool.

The operating mode is chosen by --server, --client, or neither (general mode).
Each mode has a set of options which are allowed in this mode.

Server mode options:
  -s, --server                  run in server mode, i.e. keep running and wait
                                for clients to connect. only one of -s or -c may
                                be given at the same time, but not both.
  -e, --exec                    marks the start of the REPL command. all
                                arguments that follow -e form the argv of the
                                REPL process: the first is the executable, the
                                rest are its arguments. -e must therefore be the
                                last replstoke option on the command line. the
                                argv is passed directly to the operating system,
                                without any shell interpretation (no word
                                splitting beyond what the calling shell already
                                did, no globbing, no variable expansion, no
                                pipes or redirection). the executable is resolved
                                via PATH; if it cannot be found, replstoke exits
                                with an error before starting the server. the
                                bare separator -- is accepted as an alias for
                                -e, so "replstoke --server -- python3 -i" and
                                "replstoke --server -e python3 -i" are equivalent.
                                <cmdname>, used in default file names, is the
                                filename of the executable without its path.
                                the REPL must run in an interactive and
                                unbuffered (or at least line-buffered) mode:
                                replstoke keeps the REPL's stdin open the whole
                                time, so a REPL that waits for stdin EOF before
                                producing output will deadlock, and one that
                                block-buffers its stdout will not deliver the
                                end of response marker in time. most REPLs have
                                flags or env vars for this; e.g. python3 needs
                                -i (force interactive) and -u (unbuffered):
                                "-e /usr/bin/python3 -i -u".
  -r, --restart-on-repl-exit    restart the REPL if the REPL process ended for
                                any reason
      --restart-on-midrequ-dc  if a client disconnects after its input was
                                forwarded to the REPL, but before the REPL responded,
                                and the REPL then fails to finish within --response-timeout,
                                restart the REPL. requires --response-timeout to take
                                effect.
      --reponse-timeout=SECONDS after a client disconnects mid-request, how long
                                to wait for the REPL to finish before considering
                                it wedged (fractional ok). by default the server
                                waits indefinitely.
  -a, --addr[=ADDR]             bind to (listen on) the specified local ADDR
                                using a tcp listening socket;
                                default: 127.0.0.1
                                only one of -u and -a are allowed at the same
                                time. if neither of the options -a or -u are
                                given, then -a is assumed.
  -p, --port=PORT               bind to (listen on) the specified local network
                                port number; only allowed when -a is used;
                                default: 44556
  -u, --unixsocket[=SOCKPATH]   bind to (listen on) the unix socket at the
                                specified path and listen for incoming
                                connections;
                                default: ./.replstoke_socket_<cmdname>_pid<pid>
                                (<pid> = process id, the id of the own process,
                                i.e. the replstoke server process). <cmdname> is
                                the filename (without the path) of the REPL
                                executable.
                                only one of -u and -a are allowed at the same
                                time.
  -d, --pidfile[=PIDFILE]       write the process id (pid) of the own process,
                                i.e. the process id of the server process, to
                                the specified file and delete the file when the
                                process exits; by default, no pidfile is
                                written. if this option is specified, but no
                                value is given, then the default pidfile is
                                used.
                                default:
                                ./.replstoke_process_id_<cmdname>_pid<pid>
                                <cmdname> is the filename (without the path) of
                                the REPL executable.
      --raw                     disable the framed protocol and behave like a
                                plain byte forwarder: the REPL's stdout and
                                stderr are merged into one unframed stream sent
                                to the client. must be set identically on the
                                server and the client (there is no negotiation);
                                a mismatch produces errors. see PROTOCOL.md.
                                disables server-side detection below.
  -m, --end-marker-stdout=M     success end-of-response marker the server watches
                                on the REPL's stdout ("out" stream) to decide the
                                REPL became idle again. empty disables. default as
                                on the client.
      --end-marker-stderr=M     success end-of-response marker the server watches
                                on the REPL's stderr ("err" stream). empty
                                disables. default as on the client.
      --error-marker-stdout=M   error end-of-response marker watched on stdout;
                                also ends a response (the server treats every
                                marker as "idle again"). empty disables. default
                                as on the client.
      --error-marker-stderr=M   error end-of-response marker watched on stderr.
                                empty disables. default as on the client.
      --strip-marker-stdout     drop the matched stdout marker from what is
                                forwarded to the client.
      --strip-marker-stderr     drop the matched stderr marker from what is
                                forwarded to the client.
      --ready-marker-stdout=M   marker the server watches on the REPL's stdout
                                during the boot (start/restart) phase to decide
                                the REPL has finished booting and may be served
                                to clients. empty disables. default: disabled.
      --ready-marker-stderr=M   likewise on the REPL's stderr. if both ready
                                markers are set, the earlier match makes it
                                ready. empty disables. default: disabled.
      --ready-wait=SECONDS      instead of a marker, give the REPL this long
                                (fractional ok) after (re)spawn to boot; once it
                                elapses the REPL is considered ready. mutually
                                exclusive with the ready markers.
      --ready-marker-timeout=S  if no ready marker is seen within S seconds of
                                (re)spawn, assume the REPL is stuck and terminate
                                it (using the same SIGTERM->SIGKILL teardown as
                                -k), then restart it if --restart was given.
                                allowed only when a ready marker is given.
      --warmup-input=DATA       after the REPL boots, send DATA to it a single
                                time before serving any client (e.g. to preload
                                libraries). its output is routed to the server,
                                not to clients, and client input is not forwarded
                                until it finishes. needs a warmup-end detector
                                (below). default: empty (none).
      --warmup-marker-stdout=M  the warmup finishes when M is seen on the
      --warmup-marker-stderr=M  REPL's stdout / stderr. empty disables.
      --warmup-wait=SECONDS     the warmup finishes at the earliest of a marker
                                match or this wait. at least one warmup marker or
                                --warmup-wait is required with --warmup-input.
      --warmup-marker-timeout=S if a warmup marker is set but not seen within S
                                seconds, assume the REPL is stuck and terminate it
                                (restarting it if --restart was given), analogous
                                to --ready-marker-timeout.

Client mode options:
  -c, --client                  run in client mode, i.e. connect to a replstoke
                                server. only -c or -s may be given, but not
                                both.
  -a, --addr[=ADDR]             connect to the specified network address;
                                default: 127.0.0.1
                                only one of -u and -a are allowed at the same
                                time. if neither of the options -a or -u are
                                given, then -a is assumed.
  -p, --port=PORT               connect to the specified network port number;
                                only allowed when -a is used;
                                default: 44556
  -u, --unixsocket[=SOCKPATH]   connect to the unix socket at the specified
                                path; behavior when SOCKPATH is omitted: look
                                for sockets named ./.replstoke_socket_* . if
                                exactly one exists, connect to it. if multiple
                                exist, print an error and exit.
                                only one of -u and -a are allowed at the same
                                time.
  -i, --arginput=ARGINPUT       ARGINPUT is sent to the server, which forwards
                                it to the REPL. if -f is also provided, then
                                first ARGINPUT is sent to the REPL and
                                afterwards the streamed data from -f.
  -f, --fileinput=FILEINPUT     read data from file FILEINPUT and send it to
                                the server. if --arginput is also used, then
                                the input given via --arginput is sent first
                                and then the streamed input data. if FILEINPUT
                                is - , then input is read from stdin.
  -x, --suffix=SUFFIX           when the client is done forwarding the input to
                                the server, then it sends the SUFFIX to the
                                server, which forwards it to the REPL.
                                default: empty string, i.e. by default, no
                                suffix is sent.
  Each stream (stdout, stderr) can be watched for two markers: an "end" marker
  that signals a successful end of response and an "error" marker that signals a
  failed one. whichever matches first ends the response; the earliest match wins.
  when a marker is found the client stops reading and terminates after processing
  the data up to and including the marker: exit 0 for an end marker, non-zero for
  an error marker. any marker set to the empty string is disabled.
  -m, --end-marker-stdout=M     success end-of-response marker on the REPL's
                                stdout (the reassembled "out" stream).
                                default: "\n\n" on linux/unix and "\r\n\r\n"
                                on windows.
      --end-marker-stderr=M     success end-of-response marker on the REPL's
                                stderr (the reassembled "err" stream). default:
                                disabled.
      --error-marker-stdout=M   error end-of-response marker on stdout; a match
                                exits non-zero. default: disabled.
      --error-marker-stderr=M   error end-of-response marker on stderr; a match
                                exits non-zero, so a caller is not left waiting
                                for an end marker that will never arrive.
                                default: "error".
      --strip-marker-stdout     when specified, the matched stdout marker is not
                                included in the client's output.
      --strip-marker-stderr     when specified, the matched stderr marker is not
                                included in the client's output.
      --timeout=SECONDS         give up waiting for a complete response after
                                SECONDS and exit with a distinct non-zero status
                                (124). by default the client waits indefinitely.
      --ctl=MODE                what to do with the server's ctl status
                                messages: "ignore" (default), "stdout", or
                                "stderr" to print them to the client's stdout or
                                stderr. terminal ctl error messages are always
                                printed to stderr regardless of this setting.
      --raw                     disable the framed protocol (see the server
                                option of the same name). must match the server.

General mode options:
  -k, --kill[=PIDFILE]          kill a previously started server process which
                                has its pid (process id) written to the
                                specified file;
                                if the PIDFILE is omitted, pid files matching
                                the format ./.replstoke_process_id_* are searched
                                for. if there is exactly one such file, the
                                process belonging to it is terminated.
                                termination is done in this manner:
                                linux/unix/mac:
                                first, SIGTERM is sent to the server process
                                and, if the process is not terminating itself
                                within a timeout, then the server process is
                                killed using SIGKILL.
                                if the server process was ended or wasn't
                                running in the first place and if the pidfile
                                still exists, then it is deleted by this
                                process.
                                windows:
                                windows has no posix signals, so the following
                                analogous mechanism is used. first, a
                                CTRL_BREAK_EVENT is sent to the server's process
                                group (the server starts itself in its own
                                process group so that it can receive this). the
                                server installs a console control handler that
                                performs the same clean shutdown it would do on
                                SIGTERM (terminate the REPL, remove the unix
                                socket file if any, remove the pidfile) and then
                                exits. if the server has not terminated within
                                the same timeout, it is killed forcefully with
                                TerminateProcess, which is the analog of
                                SIGKILL. as with SIGKILL, a forceful kill runs
                                no cleanup handler; to avoid leaking the REPL in
                                that case, the server assigns the REPL to a job
                                object created with
                                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, so the REPL
                                is reaped automatically when the server process
                                dies. the pidfile is then deleted following the
                                same rule as on linux/unix/mac.
                                these operations use functions from the
                                os-provided kernel32 library (GenerateConsole-
                                CtrlEvent, OpenProcess, TerminateProcess, the
                                job object api), declared via raw FFI; this does
                                not introduce a third-party dependency.
                                if many pidfiles are found with the above
                                pattern, no server process is killed and
                                instead an error message is printed.
  -h, --help                    display this help and exit
      --version                 output version information and exit

General options may not be combined with -s or -c.

Examples:
  replstoke --server -d -e /usr/bin/python3 -i -u
  replstoke --client -x $'print("__END__")\n' -m '__END__' -i $'print(3*6)\n'
  { echo 'hello'; cat - ; } | telnet 127.0.0.1 44556
  replstoke -k

## Concepts

- If any of the command line options don't make sense, the process shall exit with an appropriate error message that tells exactly why it is exiting, e.g. "error: --pidfile is not allowed when running in client mode".
- In general, the tool shall be fail fast at startup time and try to recover or prevent most errors during execution.
- replstoke forwards input and output as binary data, exactly as received, and never inserts, removes, or rewrites any payload bytes of its own. In particular it does not append a newline after the arginput (-i), the file input (-f), or the suffix (-x). If the wrapped REPL needs a trailing newline to evaluate a line, the client must include it in the input it sends (e.g. -i $'print(3*6)\n'). This keeps maximum flexibility on the client side. The framed protocol (see PROTOCOL.md) adds packet framing only on the wire; the client removes it again, so the reassembled out and err streams are byte-exact. The client-to-server direction is never framed.
- By default the server-to-client direction uses the framed protocol, which carries the REPL's stdout and stderr as separate streams ("out" and "err") plus a server control stream ("ctl"). The client demuxes them, writing "out" to its stdout and "err" to its stderr, so stderr can no longer corrupt the stdout end-of-response marker. The protocol is versioned and always on; it can be disabled on both sides with --raw, which restores the old plain forwarder (stderr merged into stdout, no framing). There is no protocol negotiation: if the client sees a server major version higher than it understands, or the stream is not parsable, it prints an error to its stderr and exits non-zero.
- The REPL's stderr is always also written to the server's own stderr.
- The server sends a ctl "status" packet to a client upon connection (this replaces the former greeting line); status packets are informational and the client ignores them unless --ctl routes them to its stdout or stderr. When the server determines a terminal problem while a client is connected (the REPL repeatedly crashed during startup, a restart failed, or the REPL exited without --restart), it sends a ctl "error" packet; the client then prints the message to its stderr and exits non-zero.
- The client never coordinates timing with the server. It connects, sends its input, and reads the response, bounded only by its own --timeout. Client-to-REPL timing relies on ordinary stream buffering: if the REPL is not yet consuming input (for example while it is still starting up), the bytes simply wait in the buffer until it does. There is no readiness flag and no "please wait" exchanged — the REPL is not modelled as a conversational peer.
- Startup and restart are the same procedure. At startup the server fails fast if the REPL executable cannot be started. With --restart, a REPL that exits is restarted with a short backoff; if the REPL repeatedly exits on its own before serving any client (a startup crash-loop), the server gives up, signals a ctl error to any connected client, and exits non-zero rather than spinning. While the REPL is momentarily absent (between exit and the next spawn), forwarding of client input is deferred until the new REPL's stdin is available rather than the input being dropped.
- Readiness: a REPL may need time to boot before it can handle input. When a readiness mechanism is configured, the server holds a freshly (re)spawned REPL as "not ready". A client that connects during this phase is still accepted (not refused); the server simply does not process its input yet — it defers forwarding the client's data to the REPL until the REPL is ready. While not ready, the REPL's own boot output is routed to the server's stdout/stderr and never to the connected client, so a booting REPL's banner cannot be mistaken for a response. Readiness is reached either when a ready marker (--ready-marker-stdout / --ready-marker-stderr) is seen on the corresponding stream — the earlier of the two wins — or, alternatively, once a fixed --ready-wait duration given to the REPL to boot has elapsed; the two mechanisms are mutually exclusive. Readiness detection is active only during the start phase and the identical restart phase; once ready it is not consulted again until the next respawn. Ready markers are only observed, never stripped from the REPL's output. If --ready-marker-timeout is set and no ready marker arrives within it, the REPL is assumed stuck and torn down with the same SIGTERM→SIGKILL teardown as --kill; the normal exit handling then restarts it if --restart was given, or otherwise the server exits. A REPL torn down for never becoming ready counts toward the startup crash-loop breaker, so a REPL that never boots does not restart forever. By default no readiness mechanism is set and a REPL is ready immediately.
- Warmup (--warmup-input): after the REPL boots, and before any client is served, the server sends the warmup input to the REPL a single time. This is a distinct phase between boot and servable ("start, warmup, ready"): clients that connect are accepted but their input is not forwarded (exactly as during boot), and the warmup's own output is routed to the server's stdout/stderr, never to a client. The end of the warmup is detected by its own detectors — a warmup marker on stdout or stderr (--warmup-marker-stdout / --warmup-marker-stderr) or a --warmup-wait, finishing at the earliest of a marker match or the wait; at least one is required. Additionally, --warmup-marker-timeout terminates (and, with --restart, restarts) a REPL whose warmup marker is not seen in time, analogous to --ready-marker-timeout. The warmup input is written to the REPL's stdin before the "warmup sent" flag is set, and the warmup cannot complete before that flag, so boot output seen beforehand (e.g. a startup prompt) cannot end it prematurely. A warmup is run once per REPL instance, and again after every restart. Because a warmup that crashes the REPL or never completes leaves the server unservable, it should be a quick, reliable command. By default no warmup is configured.
- The server forms its own end-of-response opinion of the REPL by watching the REPL's stdout/stderr streams for the configured markers (the --end-marker-* / --error-marker-* options): the REPL is "busy" from when client input is forwarded until any of those markers is seen, then "idle" again. The server does not distinguish end from error markers — either one means the response finished. Because the REPL stream has no inherent request/response boundaries, this is an approximation, not exact — it is the same never-perfect detection the client does, performed server-side. It is used only to serialise clients safely and to handle a client that disconnects mid-request; it never gates or delays the connected client's own reading. Detection applies only in framed mode; --raw is a plain byte forwarder with no server-side detection.
- If a client disconnects after its input was forwarded but before the REPL finished the response, the server keeps the REPL reserved (accepting no other client) until the REPL finishes. With --response-timeout set, a REPL that does not finish within it is considered wedged and is terminated and restarted using the same SIGTERM→SIGKILL teardown as --kill (the server is REPL-aware and no longer needs a separate client-disconnect hint); otherwise the server waits for the REPL to finish on its own.
- When no client is connected to the server, but there is input from the REPL, then the server prints this input to its stdout, but otherwise does not use it. This includes the case before the first client connects. This can cause a race condition when the first client connects while or before the REPL writes a welcome banner. This is an accepted risk.
- When the REPL process terminates and --restart is not specified, then the server also terminates.
- When the server terminates for any reason that allows cleanup (normal exit, SIGTERM, or the windows CTRL_BREAK_EVENT), it terminates its REPL child process, removes the unix socket file if it created one, and removes its pidfile if it wrote one. On a forceful kill (SIGKILL / TerminateProcess) no cleanup handler runs; on those platforms the REPL is reaped by other means (process-group teardown on unix, a kill-on-close job object on windows), and a stale unix socket file or pidfile may be left behind. A subsequent server start with the same default socket path, and the -k logic, both tolerate such leftovers.
- The server permits at most one client to be active. If a client is already connected and another client tries to connect, then the server does not accept the new client's connection. The same applies while the REPL is still busy with a previous request whose client has already disconnected. (A client that connects while the REPL is still booting is not rejected; it is accepted and its input is not forwarded until the REPL is ready.)
- Exit codes: replstoke exits with 0 on success. It exits with a non-zero code when command line options are invalid, when the REPL executable cannot be started, when a client cannot connect to a server, or when -k cannot find or terminate a server. In client mode it also exits non-zero when an error marker is seen (--error-marker-stdout / --error-marker-stderr), when a ctl error is received, when the protocol stream is unparsable or the server's major protocol version is unsupported, or when the connection is closed by the server before any end-of-response marker has been seen; in all of these cases the client still writes whatever it already received before exiting.
- There is no access control and no encryption done by replstoke. These are accepted missing features.
- Unix sockets are a first class transport on Linux and Mac. On Windows the portable default transport is tcp; unix socket support on Windows depends on the platform offering AF_UNIX and is best effort. When a unix socket is unavailable, replstoke fails fast with an error rather than silently falling back to tcp.

## Tests

Think of various tests and integrate them into the build of this tool before writing the code. Then you can do test driven development. There is a file called dummyrepl.py, which can be used as a stub REPL for testing. It uses the default "\n\n" to mark the end of a response. You can also use python3 as a REPL for testing. There you can make use of the --end-marker-stdout option and the --suffix option to make python3 print a more specific end of response marker.
 


