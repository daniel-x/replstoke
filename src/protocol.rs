//! Framed server-to-client protocol. See `PROTOCOL.md`.
//!
//! Packets are length-prefixed and binary-safe. Each carries one of the streams
//! `out`, `err`, or `ctl`. The readable header is a convenience for text
//! payloads; framing integrity relies solely on the length field.

use std::io::{self, BufReader, Read};

pub const PROTOCOL_ID: &[u8] = b"# RePLstOKE/";
pub const VERSION_MAJOR: u32 = 1;
pub const VERSION_MINOR: u32 = 0;

const SEP: u8 = b' ';
const HEADER_END: u8 = b'\n';
const PACKET_END: u8 = b'\n';
const LENGTH_DIGITS: usize = 5;

/// Largest possible whole packet, bounded by the fixed 5-digit length field.
pub const MAX_PACKET: usize = 99_999;

pub const STREAM_OUT: &str = "out";
pub const STREAM_ERR: &str = "err";
pub const STREAM_CTL: &str = "ctl";

fn version_str() -> String {
    format!("{VERSION_MAJOR}.{VERSION_MINOR}")
}

fn overhead(stream_name: &str) -> usize {
    PROTOCOL_ID.len() + version_str().len() + 1 + LENGTH_DIGITS + 1 + stream_name.len() + 1 + 1
}

/// Maximum payload that fits in a single packet for the given stream.
pub fn max_payload(stream_name: &str) -> usize {
    MAX_PACKET - overhead(stream_name)
}

/// Append one or more packets carrying `payload` for `stream_name` to `out`,
/// splitting across packets when the payload exceeds the per-packet maximum.
pub fn encode(out: &mut Vec<u8>, stream_name: &str, payload: &[u8]) {
    let cap = max_payload(stream_name);
    if payload.is_empty() {
        encode_one(out, stream_name, &[]);
        return;
    }
    let mut off = 0;
    while off < payload.len() {
        let end = (off + cap).min(payload.len());
        encode_one(out, stream_name, &payload[off..end]);
        off = end;
    }
}

fn encode_one(out: &mut Vec<u8>, stream_name: &str, payload: &[u8]) {
    let total = overhead(stream_name) + payload.len();
    debug_assert!(total <= MAX_PACKET);
    out.extend_from_slice(PROTOCOL_ID);
    out.extend_from_slice(version_str().as_bytes());
    out.push(SEP);
    let len_str = format!("{total:0LENGTH_DIGITS$}");
    out.extend_from_slice(len_str.as_bytes());
    out.push(SEP);
    out.extend_from_slice(stream_name.as_bytes());
    out.push(HEADER_END);
    out.extend_from_slice(payload);
    out.push(PACKET_END);
}

/// Build a `ctl` status payload from a key=value field string.
pub fn ctl_status(fields: &str) -> Vec<u8> {
    format!("status {fields}").into_bytes()
}

/// Build a terminal `ctl` error payload.
pub fn ctl_error(message: &str) -> Vec<u8> {
    format!("error {message}").into_bytes()
}

/// Parsed control message.
#[derive(Debug, PartialEq, Eq)]
pub enum Ctl {
    Status(String),
    Error(String),
    /// Unknown control type; treated as informational.
    Other(String),
}

pub fn parse_ctl(payload: &[u8]) -> Ctl {
    let s = String::from_utf8_lossy(payload).into_owned();
    let (kind, rest) = match s.split_once(' ') {
        Some((k, r)) => (k, r.to_string()),
        None => (s.as_str(), String::new()),
    };
    match kind {
        "status" => Ctl::Status(rest),
        "error" => Ctl::Error(rest),
        _ => Ctl::Other(s),
    }
}

#[derive(Debug)]
pub struct Packet {
    pub stream: String,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    /// Stream is not valid framing (truncated, garbage, version mismatch by raw peer).
    BadFraming(String),
    /// Server speaks a major version this client does not understand.
    UnsupportedVersion {
        major: u32,
        minor: u32,
    },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Io(e) => write!(f, "{e}"),
            ProtocolError::BadFraming(m) => write!(f, "malformed protocol stream: {m}"),
            ProtocolError::UnsupportedVersion { major, minor } => {
                write!(
                    f,
                    "server protocol version {major}.{minor} is newer than this client supports"
                )
            }
        }
    }
}

/// Reads framed packets from a byte stream.
pub struct PacketReader<R: Read> {
    inner: BufReader<R>,
}

impl<R: Read> PacketReader<R> {
    pub fn new(inner: R) -> Self {
        PacketReader {
            inner: BufReader::new(inner),
        }
    }

    /// Read the next packet. `Ok(None)` means a clean end of stream at a packet
    /// boundary.
    pub fn read_packet(&mut self) -> Result<Option<Packet>, ProtocolError> {
        let mut first = [0u8; 1];
        match self.inner.read(&mut first) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => return Err(ProtocolError::Io(e)),
        }
        if first[0] != PROTOCOL_ID[0] {
            return Err(ProtocolError::BadFraming(
                "missing packet start marker".into(),
            ));
        }
        let mut rest = vec![0u8; PROTOCOL_ID.len() - 1];
        fill(&mut self.inner, &mut rest)?;
        if rest != PROTOCOL_ID[1..] {
            return Err(ProtocolError::BadFraming("bad packet start marker".into()));
        }

        let version = read_until_sep(&mut self.inner, 16)?;
        let (major, minor) = parse_version(&version)?;
        if major > VERSION_MAJOR {
            return Err(ProtocolError::UnsupportedVersion { major, minor });
        }

        let mut len_bytes = [0u8; LENGTH_DIGITS];
        fill(&mut self.inner, &mut len_bytes)?;
        let total = parse_len(&len_bytes)?;

        let consumed = PROTOCOL_ID.len() + version.len() + 1 + LENGTH_DIGITS;
        // remaining bytes: SEP + stream_name + HEADER_END + payload + PACKET_END
        if total < consumed + 3 {
            return Err(ProtocolError::BadFraming("packet length too small".into()));
        }
        let mut buf = vec![0u8; total - consumed];
        fill(&mut self.inner, &mut buf)?;

        if buf[0] != SEP {
            return Err(ProtocolError::BadFraming(
                "missing separator before stream name".into(),
            ));
        }
        let he = buf
            .iter()
            .position(|&b| b == HEADER_END)
            .ok_or_else(|| ProtocolError::BadFraming("missing header terminator".into()))?;
        if *buf.last().unwrap() != PACKET_END {
            return Err(ProtocolError::BadFraming(
                "missing packet terminator".into(),
            ));
        }
        let stream = String::from_utf8_lossy(&buf[1..he]).into_owned();
        let payload = buf[he + 1..buf.len() - 1].to_vec();
        Ok(Some(Packet { stream, payload }))
    }
}

fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<(), ProtocolError> {
    match r.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            Err(ProtocolError::BadFraming("truncated packet".into()))
        }
        Err(e) => Err(ProtocolError::Io(e)),
    }
}

fn read_until_sep(r: &mut impl Read, max: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        fill(r, &mut b)?;
        if b[0] == SEP {
            return Ok(out);
        }
        out.push(b[0]);
        if out.len() > max {
            return Err(ProtocolError::BadFraming("version field too long".into()));
        }
    }
}

fn parse_version(v: &[u8]) -> Result<(u32, u32), ProtocolError> {
    let bad = || ProtocolError::BadFraming("bad version".into());
    let s = std::str::from_utf8(v).map_err(|_| bad())?;
    let (maj, min) = s.split_once('.').ok_or_else(bad)?;
    Ok((
        maj.parse().map_err(|_| bad())?,
        min.parse().map_err(|_| bad())?,
    ))
}

fn parse_len(b: &[u8]) -> Result<usize, ProtocolError> {
    let s = std::str::from_utf8(b).map_err(|_| ProtocolError::BadFraming("bad length".into()))?;
    s.parse()
        .map_err(|_| ProtocolError::BadFraming("bad length".into()))
}

#[cfg(test)]
mod tests;
