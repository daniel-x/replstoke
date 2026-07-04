use super::*;
use std::ffi::OsString;

#[test]
fn cmdname_strips_directory() {
    assert_eq!(cmdname(&OsString::from("/usr/bin/python3")), "python3");
    assert_eq!(cmdname(&OsString::from("python3")), "python3");
    assert_eq!(cmdname(&OsString::from("./foo/bar.py")), "bar.py");
}

#[test]
fn default_paths() {
    assert_eq!(
        default_socket_path("python3", 1234),
        PathBuf::from("./.replstoke_pid1234_python3_socket")
    );
    assert_eq!(
        default_pidfile_path("python3", 1234),
        PathBuf::from("./.replstoke_pid1234_python3_pidfile")
    );
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("replstoke_names_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), b"").unwrap();
}

#[test]
fn discover_single_socket() {
    let dir = tempdir("sock_one");
    touch(&dir, ".replstoke_pid1_python3_socket");
    let got = discover_one_in(&dir, NAME_PREFIX, SOCKET_SUFFIX, "socket").unwrap();
    assert_eq!(got.file_name().unwrap(), ".replstoke_pid1_python3_socket");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn discover_zero_is_error() {
    let dir = tempdir("sock_zero");
    let err = discover_one_in(&dir, NAME_PREFIX, SOCKET_SUFFIX, "socket").unwrap_err();
    assert!(err.message.contains("no socket found"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn discover_many_is_error() {
    let dir = tempdir("sock_many");
    touch(&dir, ".replstoke_pid1_a_socket");
    touch(&dir, ".replstoke_pid2_b_socket");
    let err = discover_one_in(&dir, NAME_PREFIX, SOCKET_SUFFIX, "socket").unwrap_err();
    assert!(err.message.contains("found 2 socket"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn discover_distinguishes_socket_from_pidfile_by_suffix() {
    // Socket and pidfile share the prefix; the suffix keeps discovery unambiguous.
    let dir = tempdir("sock_vs_pid");
    touch(&dir, ".replstoke_pid1_python3_socket");
    touch(&dir, ".replstoke_pid1_python3_pidfile");
    let sock = discover_one_in(&dir, NAME_PREFIX, SOCKET_SUFFIX, "socket").unwrap();
    assert_eq!(sock.file_name().unwrap(), ".replstoke_pid1_python3_socket");
    let pidfile = discover_one_in(&dir, NAME_PREFIX, PIDFILE_SUFFIX, "pidfile").unwrap();
    assert_eq!(pidfile.file_name().unwrap(), ".replstoke_pid1_python3_pidfile");
    std::fs::remove_dir_all(&dir).unwrap();
}
