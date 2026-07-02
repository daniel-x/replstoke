use super::*;
use config::PidfileSpec;

fn parse_args(args: &[&str]) -> AppResult<Config> {
    let owned: Vec<OsString> = args.iter().map(OsString::from).collect();
    parse(&owned)
}

fn server(args: &[&str]) -> ServerConfig {
    match parse_args(args).expect("expected server config") {
        Config::Server(s) => s,
        other => panic!("expected server, got {other:?}"),
    }
}

fn client(args: &[&str]) -> ClientConfig {
    match parse_args(args).expect("expected client config") {
        Config::Client(c) => c,
        other => panic!("expected client, got {other:?}"),
    }
}

fn general(args: &[&str]) -> GeneralAction {
    match parse_args(args).expect("expected general config") {
        Config::General(g) => g,
        other => panic!("expected general, got {other:?}"),
    }
}

#[test]
fn help_and_version_short_circuit() {
    assert_eq!(general(&["--help"]), GeneralAction::Help);
    assert_eq!(general(&["-h"]), GeneralAction::Help);
    assert_eq!(general(&["--version"]), GeneralAction::Version);
    // help wins even with otherwise-invalid combos
    assert_eq!(general(&["-s", "-c", "--help"]), GeneralAction::Help);
}

#[test]
fn server_minimal_defaults_to_tcp() {
    let s = server(&["--server", "-e", "python3", "-i"]);
    assert_eq!(
        s.repl_argv,
        vec![OsString::from("python3"), OsString::from("-i")]
    );
    assert_eq!(
        s.bind,
        Bind::Tcp {
            addr: "127.0.0.1".into(),
            port: 44556
        }
    );
    assert!(!s.restart);
    assert!(!s.raw);
    assert!(s.pidfile.is_none());
}

#[test]
fn double_dash_is_exec_alias() {
    let s = server(&["-s", "--", "python3", "-i", "-u"]);
    assert_eq!(
        s.repl_argv,
        vec![
            OsString::from("python3"),
            OsString::from("-i"),
            OsString::from("-u")
        ]
    );
}

#[test]
fn exec_captures_everything_including_dashes() {
    let s = server(&["-s", "-e", "/usr/bin/python3", "-i", "-u"]);
    assert_eq!(
        s.repl_argv,
        vec![
            OsString::from("/usr/bin/python3"),
            OsString::from("-i"),
            OsString::from("-u")
        ]
    );
}

#[test]
fn server_unix_socket_with_default_path() {
    let s = server(&["-s", "-u", "-e", "python3"]);
    assert_eq!(s.bind, Bind::Unix { path: None });
}

#[test]
fn server_unix_socket_with_attached_path() {
    let s = server(&["-s", "--unixsocket=/tmp/sock", "-e", "python3"]);
    assert_eq!(
        s.bind,
        Bind::Unix {
            path: Some("/tmp/sock".into())
        }
    );
}

#[test]
fn server_addr_and_port() {
    // optional-value -a takes its value only when attached (=VALUE / -aVALUE);
    // -p is a required-value option and accepts a separate token.
    let s = server(&["-s", "--addr=0.0.0.0", "-p", "9000", "-e", "python3"]);
    assert_eq!(
        s.bind,
        Bind::Tcp {
            addr: "0.0.0.0".into(),
            port: 9000
        }
    );
    let s = server(&["-s", "-a0.0.0.0", "-p9000", "-e", "python3"]);
    assert_eq!(
        s.bind,
        Bind::Tcp {
            addr: "0.0.0.0".into(),
            port: 9000
        }
    );
}

#[test]
fn optional_value_does_not_consume_next_token() {
    // bare -a means "default addr"; the following token is not its value, so it
    // is treated as an (unexpected) positional argument, with a helpful hint.
    assert_eq!(
        err(&["-s", "-a", "0.0.0.0", "-e", "python3"]),
        "unexpected argument '0.0.0.0'; did you mean -a0.0.0.0 or --addr=0.0.0.0? \
         optional-value options take their value attached, not as a separate argument"
    );
}

#[test]
fn server_pidfile_variants() {
    let s = server(&["-s", "-d", "-e", "python3"]);
    assert_eq!(s.pidfile, Some(PidfileSpec::Default));
    let s = server(&["-s", "--pidfile=/tmp/p.pid", "-e", "python3"]);
    assert_eq!(s.pidfile, Some(PidfileSpec::Path("/tmp/p.pid".into())));
}

#[test]
fn restart_and_raw_flags() {
    let s = server(&["-s", "-r", "--raw", "-e", "python3"]);
    assert!(s.restart);
    assert!(s.raw);
}

#[test]
fn greeting_option_is_gone() {
    assert_eq!(err(&["-s", "-g", "-e", "python3"]), "unknown option '-g'");
}

#[test]
fn client_input_ordering_and_raw_bytes() {
    let c = client(&["-c", "-i", "print(3*6)\n", "-x", "END\n", "-m", "END"]);
    assert_eq!(c.arginput.as_deref(), Some(b"print(3*6)\n".as_slice()));
    assert_eq!(c.suffix, b"END\n");
    assert_eq!(c.end_marker_stdout, b"END");
    assert!(!c.strip_marker_stdout);
    assert!(!c.strip_marker_stderr);
}

#[test]
fn client_default_marker() {
    let c = client(&["-c"]);
    assert_eq!(c.end_marker_stdout, default_end_marker_stdout());
    assert!(c.end_marker_stderr.is_empty());
    assert!(c.error_marker_stdout.is_empty());
    assert!(c.suffix.is_empty());
}

#[test]
fn client_fileinput_stdin_and_path() {
    let c = client(&["-c", "-f", "-"]);
    assert_eq!(c.fileinput, Some(FileInput::Stdin));
    let c = client(&["-c", "-f", "/tmp/in.txt"]);
    assert_eq!(c.fileinput, Some(FileInput::Path("/tmp/in.txt".into())));
}

#[test]
fn client_strip_marker() {
    let c = client(&["-c", "--strip-marker-stdout"]);
    assert!(c.strip_marker_stdout);
    assert!(!c.strip_marker_stderr);

    let c = client(&["-c", "--strip-marker-stderr"]);
    assert!(!c.strip_marker_stdout);
    assert!(c.strip_marker_stderr);
}

#[test]
fn attached_short_value() {
    let c = client(&["-c", "-mEND"]);
    assert_eq!(c.end_marker_stdout, b"END");
}

#[test]
fn long_value_with_equals() {
    let c = client(&["-c", "--end-marker-stdout=END"]);
    assert_eq!(c.end_marker_stdout, b"END");
}

#[test]
fn kill_with_and_without_pidfile() {
    assert_eq!(general(&["-k"]), GeneralAction::Kill(None));
    assert_eq!(
        general(&["--kill=/tmp/x.pid"]),
        GeneralAction::Kill(Some("/tmp/x.pid".into()))
    );
}

// ---- error cases -------------------------------------------------------------

fn err(args: &[&str]) -> String {
    parse_args(args).expect_err("expected error").message
}

#[test]
fn server_and_client_conflict() {
    assert_eq!(err(&["-s", "-c"]), "-s and -c cannot be combined");
}

#[test]
fn addr_and_unix_conflict() {
    assert_eq!(
        err(&["-s", "-a", "-u", "-e", "python3"]),
        "-a and -u cannot be combined"
    );
}

#[test]
fn port_requires_tcp() {
    assert_eq!(
        err(&["-s", "-u", "-p", "9000", "-e", "python3"]),
        "-p is only allowed together with -a"
    );
}

#[test]
fn pidfile_rejected_in_client_mode() {
    assert_eq!(
        err(&["-c", "-d"]),
        "--pidfile is not allowed when running in client mode"
    );
}

#[test]
fn arginput_rejected_in_server_mode() {
    assert_eq!(
        err(&["-s", "-i", "x", "-e", "python3"]),
        "--arginput is not allowed when running in server mode"
    );
}

#[test]
fn server_requires_exec() {
    assert_eq!(
        err(&["-s"]),
        "no REPL command given; use -e or -- followed by the command"
    );
}

#[test]
fn unknown_options() {
    assert_eq!(err(&["--foo"]), "unknown option '--foo'");
    assert_eq!(err(&["-z"]), "unknown option '-z'");
}

#[test]
fn invalid_port() {
    assert_eq!(
        err(&["-s", "-a", "-p", "notaport", "-e", "x"]),
        "invalid port number 'notaport'"
    );
    assert_eq!(
        err(&["-s", "-a", "-p", "99999", "-e", "x"]),
        "invalid port number '99999'"
    );
}

#[test]
fn empty_marker_disables() {
    // empty marker is now allowed and disables stdout end-marker detection
    let c = client(&["-c", "-m", ""]);
    assert!(c.end_marker_stdout.is_empty());
}

#[test]
fn err_marker_and_ctl_and_raw() {
    let c = client(&["-c"]);
    assert_eq!(c.error_marker_stderr, b"error");
    assert_eq!(c.ctl, config::CtlRoute::Ignore);
    assert!(!c.raw);

    let c = client(&["-c", "--error-marker-stderr=FAIL", "--ctl=stderr", "--raw"]);
    assert_eq!(c.error_marker_stderr, b"FAIL");
    assert_eq!(c.ctl, config::CtlRoute::Stderr);
    assert!(c.raw);
}

#[test]
fn all_four_markers_parse() {
    let c = client(&[
        "-c",
        "--end-marker-stdout=EO",
        "--end-marker-stderr=EE",
        "--error-marker-stdout=XO",
        "--error-marker-stderr=XE",
    ]);
    assert_eq!(c.end_marker_stdout, b"EO");
    assert_eq!(c.end_marker_stderr, b"EE");
    assert_eq!(c.error_marker_stdout, b"XO");
    assert_eq!(c.error_marker_stderr, b"XE");
}

#[test]
fn invalid_ctl_mode() {
    assert_eq!(
        err(&["-c", "--ctl=both"]),
        "invalid --ctl mode 'both'; expected ignore, stdout, or stderr"
    );
}

#[test]
fn timeout_parsing() {
    let c = client(&["-c", "--timeout=2.5"]);
    assert_eq!(c.timeout, Some(std::time::Duration::from_secs_f64(2.5)));
    let c = client(&["-c"]);
    assert_eq!(c.timeout, None);
}

#[test]
fn invalid_timeout() {
    assert_eq!(
        err(&["-c", "--timeout=abc"]),
        "invalid timeout 'abc'; expected a positive number of seconds"
    );
    assert_eq!(
        err(&["-c", "--timeout=0"]),
        "invalid timeout '0'; expected a positive number of seconds"
    );
}

#[test]
fn server_accepts_detection_options() {
    let s = server(&[
        "-s",
        "--response-timeout=1.5",
        "--end-marker-stdout=END",
        "--error-marker-stderr=ERR",
        "--strip-marker-stdout",
        "-e",
        "python3",
    ]);
    assert_eq!(s.response_timeout, Some(std::time::Duration::from_secs_f64(1.5)));
    assert_eq!(s.end_marker_stdout, b"END");
    assert_eq!(s.error_marker_stderr, b"ERR");
    assert!(s.strip_marker_stdout);
    assert!(!s.strip_marker_stderr);
}

#[test]
fn server_detection_defaults() {
    let s = server(&["-s", "-e", "python3"]);
    assert_eq!(s.end_marker_stdout, default_end_marker_stdout());
    assert_eq!(s.error_marker_stderr, b"error");
    assert!(s.end_marker_stderr.is_empty());
    assert!(s.error_marker_stdout.is_empty());
    assert!(!s.strip_marker_stdout);
    assert!(s.response_timeout.is_none());
}

#[test]
fn server_accepts_ready_options() {
    let s = server(&[
        "-s",
        "--ready-marker-stdout=UP",
        "--ready-marker-stderr=BOOTED",
        "--ready-marker-timeout=3",
        "-e",
        "python3",
    ]);
    assert_eq!(s.ready_marker_stdout, b"UP");
    assert_eq!(s.ready_marker_stderr, b"BOOTED");
    assert_eq!(
        s.ready_marker_timeout,
        Some(std::time::Duration::from_secs_f64(3.0))
    );
    assert!(s.ready_wait.is_none());
}

#[test]
fn server_ready_wait_alone() {
    let s = server(&["-s", "--ready-wait=0.5", "-e", "python3"]);
    assert_eq!(s.ready_wait, Some(std::time::Duration::from_secs_f64(0.5)));
    assert!(s.ready_marker_stdout.is_empty());
    assert!(s.ready_marker_stderr.is_empty());
}

#[test]
fn server_ready_defaults_empty() {
    let s = server(&["-s", "-e", "python3"]);
    assert!(s.ready_marker_stdout.is_empty());
    assert!(s.ready_marker_stderr.is_empty());
    assert!(s.ready_wait.is_none());
    assert!(s.ready_marker_timeout.is_none());
}

#[test]
fn ready_wait_and_marker_are_exclusive() {
    assert_eq!(
        err(&[
            "-s",
            "--ready-wait=1",
            "--ready-marker-stdout=UP",
            "-e",
            "python3"
        ]),
        "--ready-wait cannot be combined with --ready-marker-stdout or --ready-marker-stderr"
    );
}

#[test]
fn ready_marker_timeout_requires_a_marker() {
    assert_eq!(
        err(&["-s", "--ready-marker-timeout=2", "-e", "python3"]),
        "--ready-marker-timeout requires --ready-marker-stdout or --ready-marker-stderr"
    );
    // with --ready-wait but no marker, the timeout is still not allowed
    assert_eq!(
        err(&[
            "-s",
            "--ready-wait=1",
            "--ready-marker-timeout=2",
            "-e",
            "python3"
        ]),
        "--ready-marker-timeout requires --ready-marker-stdout or --ready-marker-stderr"
    );
}

#[test]
fn server_accepts_warmup() {
    let s = server(&[
        "-s",
        "--warmup-input",
        "import numpy\n",
        "--warmup-marker-stdout",
        "OK",
        "--warmup-wait=2",
        "-e",
        "python3",
    ]);
    assert_eq!(s.warmup, b"import numpy\n");
    assert_eq!(s.warmup_marker_stdout, b"OK");
    assert_eq!(
        s.warmup_wait,
        Some(std::time::Duration::from_secs_f64(2.0))
    );
}

#[test]
fn warmup_works_with_raw() {
    // --warmup-input is NOT prohibited with --raw.
    let s = server(&[
        "-s",
        "--raw",
        "--warmup-input",
        "x\n",
        "--warmup-wait=1",
        "-e",
        "python3",
    ]);
    assert!(s.raw);
    assert_eq!(s.warmup, b"x\n");
}

#[test]
fn warmup_requires_end_detection() {
    let msg = err(&["-s", "--warmup-input", "x\n", "-e", "python3"]);
    assert!(
        msg.starts_with("--warmup-input requires a way to detect its end"),
        "got: {msg}"
    );
}

#[test]
fn warmup_detection_requires_warmup() {
    assert_eq!(
        err(&["-s", "--warmup-wait=1", "-e", "python3"]),
        "--warmup-marker-* / --warmup-wait require --warmup-input"
    );
}

#[test]
fn warmup_rejected_in_client_mode() {
    assert_eq!(
        err(&["-c", "--warmup-input", "x\n"]),
        "--warmup-input is not allowed when running in client mode"
    );
}

#[test]
fn ready_options_rejected_in_client_mode() {
    assert_eq!(
        err(&["-c", "--ready-marker-stdout=UP"]),
        "--ready-marker-stdout is not allowed when running in client mode"
    );
    assert_eq!(
        err(&["-c", "--ready-wait=1"]),
        "--ready-wait is not allowed when running in client mode"
    );
}

#[test]
fn response_timeout_rejected_in_client_mode() {
    assert_eq!(
        err(&["-c", "--response-timeout=1"]),
        "--response-timeout is not allowed when running in client mode"
    );
}

#[test]
fn timeout_rejected_in_server_mode() {
    assert_eq!(
        err(&["-s", "--timeout=1", "-e", "python3"]),
        "--timeout is not allowed when running in server mode"
    );
}

#[test]
fn optional_value_hint_for_kill() {
    // P4: a positional after a bare optional-value option suggests the attached form
    assert_eq!(
        err(&["-k", "/tmp/x.pid"]),
        "unexpected argument '/tmp/x.pid'; did you mean -k/tmp/x.pid or --kill=/tmp/x.pid? \
         optional-value options take their value attached, not as a separate argument"
    );
}

#[test]
fn flag_with_value_rejected() {
    assert_eq!(
        err(&["--server=x"]),
        "option '--server' does not take a value"
    );
}

#[test]
fn no_mode_selected() {
    assert_eq!(
        err(&[]),
        "no mode selected; use --server, --client, --kill, or --help"
    );
}

#[test]
fn general_rejects_mode_options() {
    assert_eq!(
        err(&["-p", "9000"]),
        "-p is only valid in server or client mode"
    );
}
