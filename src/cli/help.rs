//! `--help` and `--version` output.

pub const VERSION: &str = concat!("replstoke ", env!("CARGO_PKG_VERSION"));

pub const HELP: &str = "\
replstoke 0.2.0 - wrap a long-running REPL and use it as a one-shot tool.

Usage: replstoke [OPTIONS] [-- COMMAND [ARG]...]

Mode is chosen by --server, --client, or neither (general). This is a cheat
sheet; see README.md for the full behaviour and the protocol.

Server mode (-s, --server):
  -e, --exec CMD [ARG]...     REPL command; rest of the line is its argv. -- is
                              an alias. must be last. no shell interpretation.
  -r, --restart               respawn the REPL whenever it exits.
  -a, --addr[=ADDR]           listen on tcp ADDR (default 127.0.0.1).
  -p, --port=PORT             tcp port (with -a; default 44556).
  -u, --unixsocket[=PATH]     listen on a unix socket (default path if omitted).
  -d, --pidfile[=PATH]        write a pidfile, removed on exit.
      --raw                   plain byte forwarder, no framing; match the client.
  detection (the server's own busy/idle opinion of the REPL; see README):
      --end-marker-stdout / --end-marker-stderr    response finished (success).
      --error-marker-stdout / --error-marker-stderr  response finished (error).
      --strip-out-marker / --strip-err-marker      drop the matched marker.
      --timeout=SECONDS       bound the wait after a client disconnects mid-request.
      --restart-on-client-dc  restart the REPL if it does not finish in time then.
  readiness (when the REPL has finished booting; client input is held until then):
      --ready-marker-stdout=M / --ready-marker-stderr=M   ready when seen (first wins).
      --ready-wait=SECONDS    give the REPL this long to boot, then ready (no markers).
      --ready-marker-timeout=SECONDS  kill (and restart if -r) if not ready in time.

Client mode (-c, --client):
  -a/-p/-u                    where to connect (see server; bare -u discovers
                              a single ./.replstoke_socket_* ).
  -i, --arginput=DATA         input sent to the REPL.
  -f, --fileinput=PATH        input streamed from PATH ( - is stdin), after -i.
  -x, --suffix=DATA           sent after the input.
  -m, --end-marker-stdout=M   success end-of-response marker on stdout; empty
                              disables. default \"\\n\\n\" (\"\\r\\n\\r\\n\" on windows).
      --end-marker-stderr=M   success end-of-response marker on stderr; empty
                              disables (default).
      --error-marker-stdout=M error end-of-response marker on stdout; a match
                              exits non-zero. empty disables (default).
      --error-marker-stderr=M error end-of-response marker on stderr; a match
                              exits non-zero. empty disables. default \"error\".
      --strip-out-marker      drop the matched marker from stdout output.
      --strip-err-marker      drop the matched marker from stderr output.
      --timeout=SECONDS       give up after SECONDS (fractional ok); exit 124.
      --ctl=MODE              ctl status routing: ignore (default), stdout, stderr.
      --raw                   plain byte reader; match the server.
  (marker/strip/timeout also exist on the server; each side evaluates its own,
   independently - they are not equivalent. typically declare them on the server.)

General:
  -k, --kill[=PIDFILE]        terminate a server by its pidfile (discovers a
                              single ./.replstoke_process_id_* if omitted).
  -h, --help                  show this help.
      --version               show version.

Examples:
  replstoke --server -d -u -e /usr/bin/python3 -i
  replstoke --client -x $'print(\"__END__\")\\n' -m '__END__' -i $'print(3*6)\\n'
  replstoke -k
";
