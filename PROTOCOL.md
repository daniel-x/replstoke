# Protocol specification for a REPL-wrapping command line tool

## Goal

The goal is to allow muxing of stdout, stderr, and status messages from the server to the client.
Without the protocol, the software is only a dumb forwarder and the mixing of stderr into stdout can
cause race conditions. The protocol separates these streams so the client can demux them again.

A secondary goal is to stay friendly to text-based general purpose tools such as `nc` or `telnet`.
This is a *best-effort, readability* property, not a functional guarantee: when the payload is text,
a human watching the stream still sees legible, comment-like header lines and the payload text. It
does **not** mean such tools can demux the streams, and it does **not** mean the output is a clean
passthrough — a raw client now also receives the framing headers interleaved with the data. For
arbitrary binary payloads the stream is not human-readable at all; only a real protocol-aware client
can interpret it.

By default the protocol is always used. For the cases where the old clean-passthrough behavior is
wanted, both the server and the client accept an opt-out option (see *Opting out*).

## Concepts

The foundational layer is still TCP or unix sockets, i.e. stream based communication. This takes
away the burden of ordering and delivery, so here we only need to care about muxing and demuxing
while keeping the format friendly to text tools.

The communication is packet based. Packets of this protocol are completely independent of the
boundaries of TCP/IP packets, so that TCP or unix-socket stream programming can still be used. In
the remainder of this spec, the word *packet* refers to packets of this protocol, not to TCP or IP
packets.

The protocol is binary-safe: a payload may contain arbitrary bytes, including newlines and even the
bytes of the packet markers. Because of this, two properties must be understood up front:

- **`length` is authoritative.** Packet boundaries are determined solely by the `length` field, by
  reading exactly that many bytes. The `packet_start_marker` is a readability aid and a heuristic; it
  is **not** a reliable synchronization point, because a payload can contain those same bytes.
- **There is no guaranteed mid-stream recovery.** A receiver must read the stream from the very
  beginning and stay byte-aligned by consuming exactly `length` bytes per packet. Once alignment is
  lost (or any packet is not parsable), there is no way to reliably resynchronize. The client must
  treat an unparsable stream as a fatal error: print a message to its stderr and exit (see
  *Versioning* for the related version-mismatch case).

This protocol is used only for communication **from the server to the client**. The communication
from the client to the server remains a simple bare data stream of input data; it is never framed.
As a consequence, status is only ever *pushed* by the server (via the `ctl` stream); the client
cannot issue requests over this connection.

## Versioning

The `packet_start_marker` carries a protocol version `major.minor` (see *Packet Format*). The current
version is **1.0**.

There is no version negotiation. The client reads the version from the server's packets and compares
the major version with the highest major version it understands:

- If the server's major version is **higher** than the client understands, the client prints an error
  to its stderr and disconnects. The error message hints that (a) using general purpose text tools to
  read the stream might still work for text payloads, and (b) the protocol can be disabled on both
  the server and the client (see *Opting out*).
- Otherwise the client proceeds. Within the same major version, changes are backward compatible.

## Opting out

Both the server and the client accept the same option to disable the protocol entirely (proposed
name: `--raw`). When disabled, the tool behaves like the original dumb forwarder: the server forwards
the REPL's output as a bare byte stream (with stderr merged into it) and the client reads it raw.

Because there is no negotiation, this option is **not** checked at runtime — it cannot be, as there
is no handshake. If one side uses the protocol and the other does not, errors and garbage output can
occur. This is accepted.

## Packet Format

A packet has the format below, where `+` represents byte-string concatenation.

```
packet := packet_start_marker +
          length + sep +
          stream_name +
          header_end_marker +
          payload +
          packet_end_marker
```

```
packet_start_marker := protocol_id + version + sep
protocol_id         := "# RePLstOKE/"
version             := major + "." + minor          (current: "1.0")
```

```
length := number of bytes of the complete packet, formatted as a 5-digit, zero-left-padded
          decimal number. This count includes everything from the first byte of the
          packet_start_marker to the last byte of the packet_end_marker, including both markers
          and the 5 length digits themselves.
```

`header_end_marker := "\n"`

`packet_end_marker := "\n"`

`sep := " "`

`stream_name := name of the stream the payload belongs to, one of "out", "err", or "ctl"`

`payload := any binary data (possibly empty)`

The header — everything from `packet_start_marker` up to (but not including) the `header_end_marker`
— never contains a `header_end_marker` byte (`"\n"`). This lets a human, or a line-oriented text
tool, spot packet headers in a stream dump. The `protocol_id` is a fixed literal; the `version` that
follows is variable in length and is terminated by the following `sep`, after which the 5-digit
`length` begins. The marker string is case-sensitive and deliberately mixed-case to make accidental
collisions with ordinary REPL output unlikely.

Because `length` is fixed at 5 digits, the largest possible packet is 99999 bytes, which bounds a
single payload to roughly 99972 bytes. This cap is accepted. For minimum latency the sender (the
server) should emit a packet as soon as data is available rather than filling packets to the cap.

### Example

A one-line stdout payload `hi` is sent as the 29-byte packet:

```
# RePLstOKE/1.0 00029 out\nhi\n
```

(16 bytes start marker + 5 length + 1 sep + 3 stream_name + 1 header_end + 2 payload + 1 packet_end.)

## Streams

- **`out`** — the REPL's stdout.
- **`err`** — the REPL's stderr.
- **`ctl`** — control messages generated by the server itself (not by the REPL). These carry status
  information and terminal error signals. See *Control (ctl) packets*.

A client should ignore packets whose `stream_name` it does not recognize, so future stream kinds do
not break older clients.

## Control (ctl) packets

A `ctl` packet's payload is a single line of UTF-8 text: a lowercase `type` token, optionally
followed by a space and a type-specific message. Two types are defined; any other type is treated as
informational and ignored by default.

```
ctl_payload := type [ sep + message ]
type        := "status" | "error"
message     := human-readable text, optionally with space-separated key=value fields; no newline
```

### `status` (informational)

Sent by the server **on connect** (this entirely replaces the old greeting line) and again whenever
the server's readiness changes. It is informational only — the client never changes its control flow
based on a `status` packet. By default the client **ignores** `status` packets; a command line option
lets it route them to its own stdout or stderr instead. The message carries free-form
`key=value` fields; the set is intentionally loose and may include, for example:

```
status ready=1 server_pid=1234 repl_pid=1235 requests=0 uptime_s=42 listening=127.0.0.1:44556
```

`ready=0` means the REPL is not serving yet (e.g. still starting up). The client does **not** act on
this — it may send its input immediately regardless. Flow control while the server is not ready is
handled entirely at the stream layer: the server simply defers reading and processing the client's
input until it is ready (the bytes wait in the socket buffer). No "please wait" instruction is sent,
and no waiting logic is implemented in the client.

### `error` (terminal)

Sent by the server when it has determined a problem it cannot recover from while a client is
connected — for example the REPL repeatedly crashed during startup, a restart failed, or the REPL
exited and `--restart` is not in effect. On receiving an `error` ctl packet the client prints the
message to its own stderr, disconnects, and terminates with a non-zero status, regardless of the
`status` routing option. The server then closes the connection.

## Stream reassembly

A logical stream is reconstructed by concatenating, in order, the payloads of all packets carrying
the same `stream_name`. The result is byte-exact: the reassembled `out` (resp. `err`) stream is
identical to what the REPL wrote to its stdout (resp. stderr). The framing exists only on the wire
and is removed by the client.

Output larger than fits in one packet is therefore split across several consecutive packets of the
same stream; packet ordering within the byte stream preserves the order of the data.

## End-of-response and markers

End-of-response detection operates on the **reassembled** `out` and `err` streams (not on the raw
wire bytes). Because stdout and stderr are now separate streams, the marker is split in two:

- an **out marker**, matched against the reassembled `out` stream — default: `"\n\n"` on unix/linux
  and `"\r\n\r\n"` on windows;
- an **err marker**, matched against the reassembled `err` stream — default: `"error"`.

Either marker can be disabled by setting it to the empty string.

The client stops reading and terminates the request at whichever marker is seen first:

- out marker found → normal completion (the client exits successfully);
- err marker found → error completion (the client exits with a non-zero status), which lets a caller
  detect an error response instead of waiting for an out marker that may never arrive.

In both cases the data received so far is still written out. The `--strip-marker` behavior removes
the matched marker from the emitted stream as before. If both markers are disabled, the client reads
until the server closes the connection.

When the protocol is disabled (`--raw`), there is only one combined byte stream; the out marker
applies to it, and the err marker has no effect.

## Notes on efficiency

On the sender side the format is cheap to produce: a reusable template already holds the
`packet_start_marker`, the seps, and the `header_end_marker`/`packet_end_marker`; only the `length`,
`stream_name`, and `payload` vary per packet.

On the receiver side, parsing is cheap and binary-safe: each packet begins with the marker and the
5-digit `length`, so the receiver reads the small fixed-ish header, learns the total packet size, and
reads exactly that many bytes into a buffer before extracting the `stream_name` and `payload`. The
exact reader algorithm is an implementation detail and is not fixed by this spec.

And, provided the payload is text, the framed stream can still be read and skimmed with text-based
general purpose tools.
