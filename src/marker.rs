//! Streaming end-of-response marker scanner.
//!
//! Bytes are fed in arbitrary chunks. The scanner emits everything that cannot
//! possibly be part of a future marker match immediately, and holds back the
//! trailing bytes that could begin a marker so that a marker straddling two
//! chunks is still detected. On a full match it reports completion, optionally
//! stripping the marker from the output.
//!
//! A single stream may be watched for several markers at once, each carrying the
//! [`Outcome`] it signals: an *end* marker means the response finished normally,
//! an *error* marker means it finished with an error. The earliest match wins.

/// What a matched marker signals about the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Response finished normally (success).
    End,
    /// Response finished with an error.
    Error,
}

/// A set of markers watched on one stream, each tagged with the [`Outcome`] it
/// signals. Empty markers are dropped, so a fully-empty set disables detection.
pub struct Markers {
    list: Vec<(Vec<u8>, Outcome)>,
}

impl Markers {
    pub fn new(patterns: Vec<(Vec<u8>, Outcome)>) -> Self {
        let list = patterns
            .into_iter()
            .filter(|(m, _)| !m.is_empty())
            .collect();
        Markers { list }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Earliest match in `buf` as `(position, marker_len, outcome)`. On ties the
    /// pattern listed first wins.
    fn find(&self, buf: &[u8]) -> Option<(usize, usize, Outcome)> {
        let mut best: Option<(usize, usize, Outcome)> = None;
        for (marker, outcome) in &self.list {
            if let Some(pos) = find(buf, marker) {
                if best.is_none_or(|(bp, _, _)| pos < bp) {
                    best = Some((pos, marker.len(), *outcome));
                }
            }
        }
        best
    }

    /// Longest suffix of `buf` that is a prefix of some marker (a straddling
    /// partial match to hold back until the next chunk).
    fn overlap(&self, buf: &[u8]) -> usize {
        self.list
            .iter()
            .map(|(m, _)| overlap(buf, m))
            .max()
            .unwrap_or(0)
    }
}

/// Result of feeding a chunk to the [`MarkerScanner`].
#[derive(Debug, PartialEq, Eq)]
pub enum Feed {
    /// Marker not found yet; these bytes may be emitted to the consumer.
    More(Vec<u8>),
    /// Marker found; these bytes are the final output (marker included or
    /// stripped per configuration) and `outcome` is what it signalled.
    Done { outcome: Outcome, bytes: Vec<u8> },
}

pub struct MarkerScanner {
    markers: Markers,
    strip: bool,
    /// Bytes seen but not yet emitted (a possible marker prefix).
    hold: Vec<u8>,
    done: bool,
}

impl MarkerScanner {
    pub fn new(markers: Markers, strip: bool) -> Self {
        assert!(!markers.is_empty(), "marker set must not be empty");
        MarkerScanner {
            markers,
            strip,
            hold: Vec::new(),
            done: false,
        }
    }

    /// Feed a chunk of bytes.
    pub fn feed(&mut self, chunk: &[u8]) -> Feed {
        assert!(!self.done, "feed called after Done");
        let mut buf = std::mem::take(&mut self.hold);
        buf.extend_from_slice(chunk);

        if let Some((pos, len, outcome)) = self.markers.find(&buf) {
            self.done = true;
            let end = pos + len;
            let mut out = buf[..pos].to_vec();
            if !self.strip {
                out.extend_from_slice(&buf[pos..end]);
            }
            return Feed::Done {
                outcome,
                bytes: out,
            };
        }

        // No full match. Hold back the longest suffix of `buf` that is a prefix
        // of some marker; emit the rest.
        let keep = self.markers.overlap(&buf);
        let emit_to = buf.len() - keep;
        let emit = buf[..emit_to].to_vec();
        self.hold = buf[emit_to..].to_vec();
        Feed::More(emit)
    }

    /// Bytes still held back when the stream ends without a marker match.
    pub fn flush(self) -> Vec<u8> {
        self.hold
    }
}

/// Like [`MarkerScanner`] but continuous: it never stops, reporting how many
/// end-of-response markers it has seen so far. Used server-side to track when a
/// REPL has finished a response without consuming bytes after the marker. The
/// server treats every marker the same (any outcome ends a response), so it only
/// counts matches.
pub struct ResponseScanner {
    markers: Markers,
    strip: bool,
    hold: Vec<u8>,
}

impl ResponseScanner {
    /// An empty marker set disables detection (all bytes pass through).
    pub fn new(markers: Markers, strip: bool) -> Self {
        ResponseScanner {
            markers,
            strip,
            hold: Vec::new(),
        }
    }

    /// Feed a chunk. Returns the bytes to forward (with markers stripped if
    /// configured) and the number of markers found in this chunk.
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, usize) {
        if self.markers.is_empty() {
            return (chunk.to_vec(), 0);
        }
        let mut buf = std::mem::take(&mut self.hold);
        buf.extend_from_slice(chunk);

        let mut emit = Vec::with_capacity(buf.len());
        let mut found = 0;
        let mut start = 0;
        while let Some((rel, len, _)) = self.markers.find(&buf[start..]) {
            let pos = start + rel;
            let end = pos + len;
            emit.extend_from_slice(&buf[start..pos]);
            if !self.strip {
                emit.extend_from_slice(&buf[pos..end]);
            }
            found += 1;
            start = end;
        }

        let rest = &buf[start..];
        let keep = self.markers.overlap(rest);
        let emit_to = rest.len() - keep;
        emit.extend_from_slice(&rest[..emit_to]);
        self.hold = rest[emit_to..].to_vec();
        (emit, found)
    }

    /// Bytes still held back when the stream ends.
    pub fn flush(self) -> Vec<u8> {
        self.hold
    }
}

/// Naive substring search; markers are short so this is adequate.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Length of the longest suffix of `buf` that is also a prefix of `marker`,
/// capped at `marker.len() - 1` (a full match is handled separately).
fn overlap(buf: &[u8], marker: &[u8]) -> usize {
    let max = (marker.len() - 1).min(buf.len());
    for len in (1..=max).rev() {
        if buf[buf.len() - len..] == marker[..len] {
            return len;
        }
    }
    0
}

#[cfg(test)]
mod tests;
