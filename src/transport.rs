//! Transport abstraction over TCP and Unix domain sockets so the server and
//! client runtimes are transport-agnostic.
//!
//! Unix sockets are only available through `std` on Unix targets. On other
//! platforms, selecting a unix transport fails fast (no silent TCP fallback).

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use crate::cli::config::Bind;

pub enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

pub enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

#[cfg(not(unix))]
fn unix_unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "unix sockets are not supported on this platform",
    )
}

impl Listener {
    /// Bind/listen. For [`Bind::Unix`] the path must already be resolved to a
    /// concrete value by the caller.
    pub fn bind(bind: &Bind) -> io::Result<Listener> {
        match bind {
            Bind::Tcp { addr, port } => {
                let listener = TcpListener::bind((addr.as_str(), *port))?;
                Ok(Listener::Tcp(listener))
            }
            #[cfg(unix)]
            Bind::Unix { path } => {
                let path = path
                    .as_ref()
                    .expect("unix bind path must be resolved before bind");
                let listener = UnixListener::bind(path)?;
                Ok(Listener::Unix(listener))
            }
            #[cfg(not(unix))]
            Bind::Unix { .. } => Err(unix_unsupported()),
        }
    }

    pub fn accept(&self) -> io::Result<Stream> {
        // The listener is non-blocking so the accept loop can poll. On BSD/macOS
        // the accepted socket inherits that flag, but the per-connection code uses
        // blocking reads/writes, so reset each accepted socket to blocking (a no-op
        // on Linux, where the flag is not inherited).
        match self {
            Listener::Tcp(l) => {
                let (s, _) = l.accept()?;
                s.set_nonblocking(false)?;
                Ok(Stream::Tcp(s))
            }
            #[cfg(unix)]
            Listener::Unix(l) => {
                let (s, _) = l.accept()?;
                s.set_nonblocking(false)?;
                Ok(Stream::Unix(s))
            }
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Listener::Tcp(l) => l.set_nonblocking(nonblocking),
            #[cfg(unix)]
            Listener::Unix(l) => l.set_nonblocking(nonblocking),
        }
    }
}

impl Stream {
    pub fn connect(bind: &Bind) -> io::Result<Stream> {
        match bind {
            Bind::Tcp { addr, port } => {
                let stream = TcpStream::connect((addr.as_str(), *port))?;
                Ok(Stream::Tcp(stream))
            }
            #[cfg(unix)]
            Bind::Unix { path } => {
                let path = path
                    .as_ref()
                    .expect("unix connect path must be resolved before connect");
                let stream = UnixStream::connect(path)?;
                Ok(Stream::Unix(stream))
            }
            #[cfg(not(unix))]
            Bind::Unix { .. } => Err(unix_unsupported()),
        }
    }

    pub fn try_clone(&self) -> io::Result<Stream> {
        match self {
            Stream::Tcp(s) => s.try_clone().map(Stream::Tcp),
            #[cfg(unix)]
            Stream::Unix(s) => s.try_clone().map(Stream::Unix),
        }
    }

    /// Shut down part or all of the connection, unblocking a peer's read/write.
    pub fn shutdown(&self, how: std::net::Shutdown) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.shutdown(how),
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(how),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
        }
    }
}
