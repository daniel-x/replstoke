use super::*;

fn end(marker: &[u8]) -> Markers {
    Markers::new(vec![(marker.to_vec(), Outcome::End)])
}

fn collect(marker: &[u8], strip: bool, chunks: &[&[u8]]) -> (Vec<u8>, bool) {
    let mut scanner = MarkerScanner::new(end(marker), strip);
    let mut out = Vec::new();
    let mut done = false;
    for chunk in chunks {
        match scanner.feed(chunk) {
            Feed::More(bytes) => out.extend_from_slice(&bytes),
            Feed::Done { bytes, .. } => {
                out.extend_from_slice(&bytes);
                done = true;
                return (out, done);
            }
        }
    }
    out.extend_from_slice(&scanner.flush());
    (out, done)
}

#[test]
fn single_chunk_with_marker() {
    let (out, done) = collect(b"\n\n", false, &[b"hello\n\nworld"]);
    assert!(done);
    assert_eq!(out, b"hello\n\n");
}

#[test]
fn strip_removes_marker() {
    let (out, done) = collect(b"\n\n", true, &[b"hello\n\nworld"]);
    assert!(done);
    assert_eq!(out, b"hello");
}

#[test]
fn marker_split_across_chunks() {
    let (out, done) = collect(b"\n\n", false, &[b"hello\n", b"\nworld"]);
    assert!(done);
    assert_eq!(out, b"hello\n\n");
}

#[test]
fn marker_split_byte_by_byte() {
    let (out, done) = collect(b"END", true, &[b"ab", b"E", b"N", b"D", b"more"]);
    assert!(done);
    assert_eq!(out, b"ab");
}

#[test]
fn partial_then_fail_prefix_is_emitted() {
    // "\n" looks like a marker start but is followed by 'x', not '\n'.
    let (out, done) = collect(b"\n\n", false, &[b"a\nx", b"b\n\nc"]);
    assert!(done);
    assert_eq!(out, b"a\nxb\n\n");
}

#[test]
fn no_marker_flushes_all() {
    let (out, done) = collect(b"\n\n", false, &[b"hello ", b"world\n"]);
    assert!(!done);
    assert_eq!(out, b"hello world\n");
}

#[test]
fn marker_at_very_end() {
    let (out, done) = collect(b"__END__", false, &[b"result=42", b"__END__"]);
    assert!(done);
    assert_eq!(out, b"result=42__END__");
}

#[test]
fn held_prefix_then_eof() {
    // ends with a partial marker prefix that never completes
    let (out, done) = collect(b"END", false, &[b"valueEN"]);
    assert!(!done);
    assert_eq!(out, b"valueEN");
}

#[test]
fn repeated_partial_prefixes() {
    let (out, done) = collect(b"aab", false, &[b"aaab"]);
    assert!(done);
    assert_eq!(out, b"aaab");
}

#[test]
fn marker_immediately() {
    let (out, done) = collect(b"X", true, &[b"Xrest"]);
    assert!(done);
    assert_eq!(out, b"");
}

// ---- ResponseScanner (continuous) -------------------------------------------

fn rscan(marker: &[u8], strip: bool, chunks: &[&[u8]]) -> (Vec<u8>, usize) {
    let mut scanner = ResponseScanner::new(end(marker), strip);
    let mut out = Vec::new();
    let mut found = 0;
    for chunk in chunks {
        let (emit, n) = scanner.feed(chunk);
        out.extend_from_slice(&emit);
        found += n;
    }
    out.extend_from_slice(&scanner.flush());
    (out, found)
}

#[test]
fn response_scanner_counts_multiple_markers() {
    let (out, found) = rscan(b"\n\n", false, &[b"a\n\nb\n\nc"]);
    assert_eq!(found, 2);
    assert_eq!(out, b"a\n\nb\n\nc");
}

#[test]
fn response_scanner_continues_past_marker() {
    // unlike MarkerScanner, bytes after the marker are not dropped
    let (out, found) = rscan(b"END", false, &[b"r1END", b"r2END"]);
    assert_eq!(found, 2);
    assert_eq!(out, b"r1ENDr2END");
}

#[test]
fn response_scanner_strips_each_marker() {
    let (out, found) = rscan(b"END", true, &[b"r1ENDr2END"]);
    assert_eq!(found, 2);
    assert_eq!(out, b"r1r2");
}

#[test]
fn response_scanner_straddles_chunks() {
    let (out, found) = rscan(b"\n\n", false, &[b"hi\n", b"\nbye"]);
    assert_eq!(found, 1);
    assert_eq!(out, b"hi\n\nbye");
}

#[test]
fn response_scanner_empty_marker_passes_through() {
    let (out, found) = rscan(b"", false, &[b"anything\n\n"]);
    assert_eq!(found, 0);
    assert_eq!(out, b"anything\n\n");
}

#[test]
fn response_scanner_holds_partial_prefix_until_eof() {
    let (out, found) = rscan(b"END", false, &[b"valueEN"]);
    assert_eq!(found, 0);
    assert_eq!(out, b"valueEN");
}

// ---- multiple markers per stream (end + error) ------------------------------

fn scan_two(chunks: &[&[u8]]) -> (Vec<u8>, Option<Outcome>) {
    let markers = Markers::new(vec![
        (b"OK".to_vec(), Outcome::End),
        (b"ERR".to_vec(), Outcome::Error),
    ]);
    let mut scanner = MarkerScanner::new(markers, false);
    let mut out = Vec::new();
    for chunk in chunks {
        match scanner.feed(chunk) {
            Feed::More(bytes) => out.extend_from_slice(&bytes),
            Feed::Done { outcome, bytes } => {
                out.extend_from_slice(&bytes);
                return (out, Some(outcome));
            }
        }
    }
    out.extend_from_slice(&scanner.flush());
    (out, None)
}

#[test]
fn end_marker_reports_end() {
    let (out, outcome) = scan_two(&[b"result OK tail"]);
    assert_eq!(outcome, Some(Outcome::End));
    assert_eq!(out, b"result OK");
}

#[test]
fn error_marker_reports_error() {
    let (out, outcome) = scan_two(&[b"boom ERR tail"]);
    assert_eq!(outcome, Some(Outcome::Error));
    assert_eq!(out, b"boom ERR");
}

#[test]
fn earliest_marker_wins() {
    // ERR appears before OK, so the response ends as an error.
    let (_out, outcome) = scan_two(&[b"a ERR b OK c"]);
    assert_eq!(outcome, Some(Outcome::Error));
}

#[test]
fn empty_markers_disable_detection() {
    assert!(Markers::new(vec![(Vec::new(), Outcome::End)]).is_empty());
}
