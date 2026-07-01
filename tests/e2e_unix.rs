//! End-to-end tests: run the real binary as a server wrapping a Python REPL,
//! then drive it with real clients. Unix-only (uses unix sockets and signals).
#![cfg(unix)]

use std::io::Write;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_replstoke");

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn dummyrepl() -> PathBuf {
    manifest_dir().join("dummyrepl.py")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "replstoke_e2e_{}_{}_{}",
        tag,
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Kills the server on drop so a failing test cannot leak processes.
struct ServerHandle {
    child: Child,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_server(args: &[&str]) -> ServerHandle {
    let child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    ServerHandle { child }
}

fn wait_for_socket(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("socket {} did not appear", path.display());
}

fn wait_for_tcp(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(probe) = TcpStream::connect(("127.0.0.1", port)) {
            // The probe transiently occupies the server's single-client slot;
            // close it and let the slot clear before the real client connects.
            drop(probe);
            std::thread::sleep(Duration::from_millis(150));
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("tcp port {port} did not open");
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn client(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run client")
}

fn unix_sock(dir: &Path) -> String {
    dir.join("sock").to_string_lossy().into_owned()
}

// ---- tests -------------------------------------------------------------------

#[test]
fn unix_request_response() {
    let dir = tempdir("req");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&["--client", &format!("--unixsocket={sock}"), "-i", "hello\n"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("input_from_client=hello"),
        "got: {stdout:?}"
    );
    assert!(out.stdout.ends_with(b"\n\n"), "should end with marker");
}

#[test]
fn strip_marker_removes_trailing_blank() {
    let dir = tempdir("strip");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "--strip-out-marker",
        "-i",
        "hi\n",
    ]);
    assert!(out.status.success());
    assert!(
        !out.stdout.ends_with(b"\n\n"),
        "marker should be stripped: {:?}",
        out.stdout
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("input_from_client=hi"));
}

#[test]
fn server_side_strip_out_marker() {
    // The server strips the out marker; the client disables its own markers and
    // reads until its timeout, so we observe exactly what the server forwarded.
    let dir = tempdir("srvstrip");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        "--strip-out-marker",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-m",
        "",
        "--error-marker-stderr",
        "",
        "--timeout",
        "1",
        "-i",
        "hi\n",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("input_from_client=hi"), "got: {stdout:?}");
    assert!(
        !stdout.ends_with("\n\n"),
        "server should have stripped the out marker: {stdout:?}"
    );
}

#[test]
fn arginput_precedes_fileinput() {
    let dir = tempdir("order");
    let sock = unix_sock(&dir);
    let infile = dir.join("in.txt");
    std::fs::write(&infile, b"st\n").unwrap();
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // "fir" + "st\n" must arrive as the single line "first".
    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-i",
        "fir",
        "-f",
        infile.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("input_from_client=first"),
        "got: {:?}",
        out.stdout
    );
}

#[test]
fn suffix_is_appended() {
    let dir = tempdir("suffix");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // no trailing newline in -i; the suffix supplies it.
    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-i",
        "data",
        "-x",
        "\n",
    ]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("input_from_client=data"),
        "got: {:?}",
        out.stdout
    );
}

#[test]
fn ctl_status_on_connect() {
    let dir = tempdir("ctl");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // routed to stdout, the ctl status line precedes the out content
    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "--ctl",
        "stdout",
        "-i",
        "x\n",
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status server_pid="), "got: {stdout:?}");
    assert!(stdout.contains("repl_pid="), "got: {stdout:?}");
}

#[test]
fn raw_mode_merges_streams() {
    let dir = tempdir("raw");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        "--raw",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        "--raw",
        &format!("--unixsocket={sock}"),
        "-i",
        "hello\n",
    ]);
    assert!(out.status.success());
    assert!(out.stdout.ends_with(b"\n\n"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("input_from_client=hello"));
}

#[test]
fn err_marker_causes_error_exit() {
    let dir = tempdir("errm");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // disable the stdout end marker; the error marker matches dummyrepl's stderr
    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-m",
        "",
        "--error-marker-stderr",
        "input from client",
        "-i",
        "hello\n",
    ]);
    assert!(
        !out.status.success(),
        "err marker should cause non-zero exit"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("input from client"),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ctl_error_on_repl_exit() {
    let dir = tempdir("ctlerr");
    let sock = unix_sock(&dir);
    let script = dir.join("once.py");
    std::fs::write(
        &script,
        "import sys\n\
         sys.stdin.readline()\n\
         sys.stdout.write('resp\\n')\n\
         sys.stdout.flush()\n",
    )
    .unwrap();

    // no --restart: the REPL exits after one line; with an out marker that never
    // matches, the client learns of the exit via a ctl error packet.
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-m",
        "NOMATCH",
        "-i",
        "go\n",
    ]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("server:"),
        "expected ctl error on stderr, got: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn second_client_is_rejected() {
    let dir = tempdir("reject");
    let sock = unix_sock(&dir);
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // First client connects and stays connected without finishing.
    let mut first = UnixStream::connect(&sock).unwrap();
    first.write_all(b"stay\n").unwrap();
    // give the server a moment to register the active client
    std::thread::sleep(Duration::from_millis(200));

    // Second client should be rejected: connection closed before any marker.
    let out = client(&["--client", &format!("--unixsocket={sock}"), "-i", "x\n"]);
    assert!(!out.status.success(), "second client should fail");

    drop(first);
}

#[test]
fn tcp_request_response() {
    let port = free_port();
    let _srv = spawn_server(&[
        "--server",
        "-a",
        "-p",
        &port.to_string(),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_tcp(port);

    let out = client(&["--client", "-a", "-p", &port.to_string(), "-i", "tcp\n"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("input_from_client=tcp"));
}

#[test]
fn kill_terminates_server_and_removes_files() {
    let dir = tempdir("kill");
    let sock = unix_sock(&dir);
    let pidfile = dir.join("srv.pid");
    let mut srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        &format!("--pidfile={}", pidfile.display()),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));
    assert!(pidfile.exists());

    let out = client(&[&format!("--kill={}", pidfile.display())]);
    assert!(out.status.success(), "kill failed: {:?}", out);

    // server should exit; reap it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while srv.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        srv.child.try_wait().unwrap().is_some(),
        "server still running"
    );
    assert!(!pidfile.exists(), "pidfile should be removed");
    assert!(!Path::new(&sock).exists(), "socket should be removed");
}

#[test]
fn restart_respawns_repl() {
    let dir = tempdir("restart");
    let sock = unix_sock(&dir);
    // A REPL that answers one request (with the default "\n\n" marker) then exits.
    let script = dir.join("once.py");
    std::fs::write(
        &script,
        "import sys\n\
         line = sys.stdin.readline()\n\
         sys.stdout.write('once=' + line.strip() + '\\n\\n')\n\
         sys.stdout.flush()\n",
    )
    .unwrap();

    let _srv = spawn_server(&[
        "--server",
        "-r",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out1 = client(&["--client", &format!("--unixsocket={sock}"), "-i", "a\n"]);
    assert!(out1.status.success());
    assert!(String::from_utf8_lossy(&out1.stdout).contains("once=a"));

    // The first REPL exited; with -r the server respawns it for a second client.
    std::thread::sleep(Duration::from_millis(200));
    let out2 = client(&["--client", &format!("--unixsocket={sock}"), "-i", "b\n"]);
    assert!(out2.status.success(), "second request after restart failed");
    assert!(String::from_utf8_lossy(&out2.stdout).contains("once=b"));
}

#[test]
fn closed_before_marker_is_error() {
    let dir = tempdir("noeof");
    let sock = unix_sock(&dir);
    let script = dir.join("once.py");
    std::fs::write(
        &script,
        "import sys\n\
         line = sys.stdin.readline()\n\
         sys.stdout.write('resp\\n\\n')\n\
         sys.stdout.flush()\n",
    )
    .unwrap();

    // No --restart: when the REPL exits, the server shuts down. The client is
    // waiting for a marker that never arrives, so it sees EOF first.
    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "-m",
        "NEVER_APPEARS",
        "-i",
        "x\n",
    ]);
    assert!(
        !out.status.success(),
        "should fail when closed before marker"
    );
    // whatever was received is still emitted
    assert!(String::from_utf8_lossy(&out.stdout).contains("resp"));
}

#[test]
fn startup_crash_loop_gives_up() {
    let dir = tempdir("crashloop");
    let sock = unix_sock(&dir);
    // A "REPL" that exits immediately on every spawn, never serving a client.
    let mut srv = spawn_server(&[
        "--server",
        "-r",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-c",
        "import sys; sys.exit(1)",
    ]);

    // The crash-loop breaker should make the server give up and exit on its own.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut exited = None;
    while Instant::now() < deadline {
        if let Some(status) = srv.child.try_wait().unwrap() {
            exited = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = exited.expect("server should have given up and exited");
    assert!(
        !status.success(),
        "server should exit non-zero on a startup crash-loop"
    );
}

#[test]
fn ready_wait_then_serves() {
    let dir = tempdir("readywait");
    let sock = unix_sock(&dir);
    // No ready marker; the server treats the REPL as ready 0.5s after spawn. A
    // client connecting sooner is refused and retries until then.
    let _srv = spawn_server(&[
        "--server",
        "--ready-wait=0.5",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // The client connects during boot; the server accepts it and holds its input
    // until --ready-wait elapses, then serves it. A single attempt succeeds.
    let out = client(&["--client", &format!("--unixsocket={sock}"), "-i", "hi\n"]);
    assert!(out.status.success(), "should serve after --ready-wait");
    assert!(String::from_utf8_lossy(&out.stdout).contains("input_from_client=hi"));
}

#[test]
fn ready_marker_then_serves() {
    let dir = tempdir("readymark");
    let sock = unix_sock(&dir);
    // dummyrepl prints "##### started" on stderr at boot; the server treats that
    // as the readiness signal.
    let _srv = spawn_server(&[
        "--server",
        "--ready-marker-stderr",
        "##### started",
        &format!("--unixsocket={sock}"),
        "-e",
        dummyrepl().to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&["--client", &format!("--unixsocket={sock}"), "-i", "hi\n"]);
    assert!(
        out.status.success(),
        "should serve once the ready marker is seen"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("input_from_client=hi"));
}

#[test]
fn booting_repl_holds_input_and_hides_boot_output() {
    let dir = tempdir("readyhold");
    let sock = unix_sock(&dir);
    // A REPL that boots slowly: it prints a banner to stdout, then its ready
    // marker to stderr, then echoes lines. The banner must NOT reach the client.
    let script = dir.join("slowboot.py");
    std::fs::write(
        &script,
        "import sys, time\n\
         time.sleep(0.8)\n\
         sys.stdout.write('BANNER_LEAK\\n'); sys.stdout.flush()\n\
         sys.stderr.write('BOOTED\\n'); sys.stderr.flush()\n\
         for line in sys.stdin:\n\
         \x20   sys.stdout.write('echo=' + line.strip() + '\\n\\n'); sys.stdout.flush()\n",
    )
    .unwrap();

    let _srv = spawn_server(&[
        "--server",
        "--ready-marker-stderr",
        "BOOTED",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    // Connect immediately (during boot). The server accepts and holds the input
    // until the REPL is ready, then serves it.
    let out = client(&["--client", &format!("--unixsocket={sock}"), "-i", "hi\n"]);
    assert!(
        out.status.success(),
        "held input should be served after boot"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("echo=hi"), "got: {stdout:?}");
    assert!(
        !stdout.contains("BANNER_LEAK"),
        "boot output must not leak to the client: {stdout:?}"
    );
}

#[test]
fn ready_marker_timeout_kills_stuck_repl() {
    let dir = tempdir("readystuck");
    let sock = unix_sock(&dir);
    // A REPL that never emits the ready marker. With no --restart, the server
    // tears it down after the ready timeout and exits on its own.
    let script = dir.join("stuck.py");
    std::fs::write(&script, "import time\ntime.sleep(60)\n").unwrap();

    let mut srv = spawn_server(&[
        "--server",
        "--ready-marker-stdout",
        "READY",
        "--ready-marker-timeout",
        "1",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);

    let deadline = Instant::now() + Duration::from_secs(8);
    while srv.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        srv.child.try_wait().unwrap().is_some(),
        "server should give up and exit after the ready-marker timeout"
    );
}

#[test]
fn client_timeout_fires() {
    let dir = tempdir("timeout");
    let sock = unix_sock(&dir);
    // A REPL that consumes a line then goes silent, never emitting a marker.
    let script = dir.join("silent.py");
    std::fs::write(
        &script,
        "import sys, time\n\
         sys.stdin.readline()\n\
         time.sleep(60)\n",
    )
    .unwrap();

    let _srv = spawn_server(&[
        "--server",
        &format!("--unixsocket={sock}"),
        "-e",
        "python3",
        "-u",
        script.to_str().unwrap(),
    ]);
    wait_for_socket(Path::new(&sock));

    let out = client(&[
        "--client",
        &format!("--unixsocket={sock}"),
        "--timeout",
        "1",
        "-i",
        "go\n",
    ]);
    assert_eq!(
        out.status.code(),
        Some(124),
        "expected timeout exit code 124"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("timed out"),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
