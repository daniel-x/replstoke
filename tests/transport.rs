//! Loopback tests for the transport abstraction (TCP and, on Unix, sockets).

use std::io::{Read, Write};
use std::thread;

use replstoke::cli::config::Bind;
use replstoke::transport::{Listener, Stream};

#[cfg(unix)]
fn echo_roundtrip(bind: Bind) {
    let listener = Listener::bind(&bind).expect("bind");
    let server = thread::spawn(move || {
        let mut conn = listener.accept().expect("accept");
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).expect("server read");
        conn.write_all(&buf).expect("server write");
    });

    let mut client = Stream::connect(&bind).expect("connect");
    client.write_all(b"hello").expect("client write");
    let mut got = [0u8; 5];
    client.read_exact(&mut got).expect("client read");
    assert_eq!(&got, b"hello");

    server.join().unwrap();
}

#[test]
fn tcp_loopback() {
    // port 0 lets the OS pick a free port; read it back for the client.
    let listener = Listener::bind(&Bind::Tcp {
        addr: "127.0.0.1".into(),
        port: 0,
    })
    .unwrap();
    let port = match &listener {
        Listener::Tcp(l) => l.local_addr().unwrap().port(),
        #[cfg(unix)]
        _ => unreachable!(),
    };
    let server = thread::spawn(move || {
        let mut conn = listener.accept().unwrap();
        let mut buf = [0u8; 3];
        conn.read_exact(&mut buf).unwrap();
        conn.write_all(&buf).unwrap();
    });
    let bind = Bind::Tcp {
        addr: "127.0.0.1".into(),
        port,
    };
    let mut client = Stream::connect(&bind).unwrap();
    client.write_all(b"abc").unwrap();
    let mut got = [0u8; 3];
    client.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"abc");
    server.join().unwrap();
}

#[test]
fn try_clone_splits_read_write() {
    let listener = Listener::bind(&Bind::Tcp {
        addr: "127.0.0.1".into(),
        port: 0,
    })
    .unwrap();
    let port = match &listener {
        Listener::Tcp(l) => l.local_addr().unwrap().port(),
        #[cfg(unix)]
        _ => unreachable!(),
    };
    let server = thread::spawn(move || {
        let mut conn = listener.accept().unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).unwrap();
        conn.write_all(&buf).unwrap();
    });
    let bind = Bind::Tcp {
        addr: "127.0.0.1".into(),
        port,
    };
    let mut writer = Stream::connect(&bind).unwrap();
    let mut reader = writer.try_clone().unwrap();
    writer.write_all(b"ping").unwrap();
    let mut got = [0u8; 4];
    reader.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"ping");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn unix_loopback() {
    let dir = std::env::temp_dir().join(format!("replstoke_xport_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("sock");
    let _ = std::fs::remove_file(&path);
    echo_roundtrip(Bind::Unix {
        path: Some(path.clone()),
    });
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}
