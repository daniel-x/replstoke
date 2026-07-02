//! `--help` and `--version` output

pub const VERSION: &str = concat!("replstoke ", env!("CARGO_PKG_VERSION"));

pub const HELP: &str = "\
replstoke 0.2.0 - wrap a long-running REPL and use it through a one-shot tool

Usage: replstoke [OPTIONS] [-- COMMAND [ARG]...]

Mode is chosen by --server, --client, or neither (general).

Server mode (-s, --server):
  -e, --exec, -- CMD [ARG]... REPL command, rest of the line is its arguments
                              must be last
  -r, --restart               restart the REPL if it crashes or hangs
  -a, --addr[=ADDR]           listen on tcp ADDR (default 127.0.0.1)
  -p, --port=PORT             tcp port, only allowed when -a is also given
                              default 44556
  -u, --unixsocket[=PATH]     listen on a unix socket (default path if omitted)
  -d, --pidfile[=PATH]        write a pidfile, removed on exit
      --raw                   plain byte forwarder, no framing, match the client
  detection (the server's own busy/idle opinion of the REPL, see README):
      --end-marker-stdout=M   response finished (success), marker on stdout
      --end-marker-stderr=M   response finished (success), marker on stderr
      --error-marker-stdout=M   response finished (error), marker on stdout
      --error-marker-stderr=M   response finished (error), marker on stderr
      --strip-marker-stdout   drop the matched marker from stdout
      --strip-marker-stderr   drop the matched marker from stderr
      --response-timeout=SECONDS   bound the wait after a client disconnects
                              mid-request
  readiness (when the REPL has finished booting, input is not forwarded until then):
      --ready-marker-stdout=M   ready when seen on stdout (first wins)
      --ready-marker-stderr=M   ready when seen on stderr (first wins)
      --ready-wait=SECONDS    give the REPL this long to boot, then ready (no markers)
      --ready-marker-timeout=SECONDS  kill (and restart if -r) if not ready in time
      --warmup-input=DATA     input run once after the REPL boots, before serving
                              any client (e.g. preload libraries). its output goes
                              to the server, not clients. needs an end detector:
      --warmup-marker-stdout=M   warmup done when seen on stdout
      --warmup-marker-stderr=M   warmup done when seen on stderr
      --warmup-wait=SECONDS    ...or done after this long
      --warmup-marker-timeout=SECONDS  kill (and restart if -r) if not done in time

Client mode (-c, --client):
  -a/-p/-u                    where to connect (see server, bare -u discovers
                              a single ./.replstoke_socket_* )
  -i, --arginput=DATA         input sent to the REPL
  -f, --fileinput=PATH        input streamed from PATH after -i ( - is stdin)
  -x, --suffix=DATA           sent after the input
  -m, --end-marker-stdout=M   success end-of-response marker on stdout, empty
                              disables. default \"\\n\\n\" (\"\\r\\n\\r\\n\" on windows)
      --end-marker-stderr=M   success end-of-response marker on stderr, empty
                              disables (default)
      --error-marker-stdout=M error end-of-response marker on stdout, a match
                              exits non-zero. empty disables (default)
      --error-marker-stderr=M error end-of-response marker on stderr, a match
                              exits non-zero. empty disables. default \"error\"
      --strip-marker-stdout   drop the matched marker from stdout output
      --strip-marker-stderr   drop the matched marker from stderr output
      --timeout=SECONDS       give up after SECONDS (fractional ok), exit 124
      --ctl=MODE              ctl status routing: ignore (default), stdout, stderr
      --raw                   plain byte reader, match the server
  (marker/strip/timeout also exist on the server, each side evaluates its own,
   independently - they are not equivalent. typically declare them on the server)

General:
  -k, --kill[=PIDFILE]        terminate a server by its pidfile (discovers a
                              single ./.replstoke_process_id_* if omitted)
  -h, --help                  show this help
      --version               show version

Examples:
  replstoke --server -d -u -e /usr/bin/python3 -i -u
  replstoke --client -u --strip-marker-stdout -i $'print(3*6)\\nprint()\\n'
  { echo 'hello'; cat - ; } | telnet 127.0.0.1 44556
  replstoke -k

Full documentation: https://github.com/daniel-x/replstoke
Copyright (c) 2026 Daniel Strecker
Licensed under the MIT License
";
