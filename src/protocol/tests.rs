use super::*;

fn read_all(bytes: &[u8]) -> Vec<Packet> {
    let mut reader = PacketReader::new(bytes);
    let mut out = Vec::new();
    while let Some(p) = reader.read_packet().unwrap() {
        out.push(p);
    }
    out
}

#[test]
fn roundtrip_single() {
    let mut buf = Vec::new();
    encode(&mut buf, STREAM_OUT, b"hi");
    // matches the worked example in PROTOCOL.md
    assert_eq!(buf, b"# RePLstOKE/1.0 00029 out\nhi\n");
    let packets = read_all(&buf);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].stream, "out");
    assert_eq!(packets[0].payload, b"hi");
}

#[test]
fn roundtrip_empty_payload() {
    let mut buf = Vec::new();
    encode(&mut buf, STREAM_ERR, b"");
    let packets = read_all(&buf);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].stream, "err");
    assert!(packets[0].payload.is_empty());
}

#[test]
fn binary_payload_with_markers_inside() {
    // payload contains newlines and even the start-marker bytes
    let payload = b"\n# RePLstOKE/1.0 \x00\xff\n\n".to_vec();
    let mut buf = Vec::new();
    encode(&mut buf, STREAM_OUT, &payload);
    let packets = read_all(&buf);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].payload, payload);
}

#[test]
fn large_payload_is_split() {
    let cap = max_payload(STREAM_OUT);
    let payload = vec![b'x'; cap + 100];
    let mut buf = Vec::new();
    encode(&mut buf, STREAM_OUT, &payload);
    let packets = read_all(&buf);
    assert_eq!(packets.len(), 2);
    let mut joined = Vec::new();
    for p in &packets {
        assert_eq!(p.stream, "out");
        joined.extend_from_slice(&p.payload);
    }
    assert_eq!(joined, payload);
}

#[test]
fn clean_eof_returns_none() {
    let mut reader = PacketReader::new(&b""[..]);
    assert!(reader.read_packet().unwrap().is_none());
}

#[test]
fn garbage_is_bad_framing() {
    let mut reader = PacketReader::new(&b"not a packet at all"[..]);
    match reader.read_packet() {
        Err(ProtocolError::BadFraming(_)) => {}
        other => panic!("expected BadFraming, got {other:?}"),
    }
}

#[test]
fn higher_major_version_is_unsupported() {
    let pkt = b"# RePLstOKE/2.0 00029 out\nhi\n";
    let mut reader = PacketReader::new(&pkt[..]);
    match reader.read_packet() {
        Err(ProtocolError::UnsupportedVersion { major: 2, minor: 0 }) => {}
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn ctl_roundtrip() {
    let mut buf = Vec::new();
    encode(&mut buf, STREAM_CTL, &ctl_status("ready=1 repl_pid=9"));
    let packets = read_all(&buf);
    assert_eq!(packets[0].stream, "ctl");
    assert_eq!(
        parse_ctl(&packets[0].payload),
        Ctl::Status("ready=1 repl_pid=9".into())
    );
}

#[test]
fn ctl_parsing() {
    assert_eq!(parse_ctl(b"status ready=1"), Ctl::Status("ready=1".into()));
    assert_eq!(
        parse_ctl(b"error repl exited"),
        Ctl::Error("repl exited".into())
    );
    assert_eq!(parse_ctl(b"weird thing"), Ctl::Other("weird thing".into()));
    assert_eq!(parse_ctl(b"status"), Ctl::Status(String::new()));
}
