# chatd

A multi-user TCP chat server written in Rust, where **all networking, threading, and
synchronisation comes from the standard library**. The only dependencies are `serde`
and `serde_json`, and only for encoding and decoding the wire protocol.

This document is a specification and a build plan. It contains no implementation.

This is a learning project. The constraint is the point.

---

## The constraint

| Concern | Source |
| --- | --- |
| Sockets | standard library (`std::net`) |
| Concurrency | standard library threads, one per socket direction |
| Message passing | standard library channels and atomics |
| Buffering | standard library buffered readers and writers |
| Wire format | `serde` and `serde_json` |

No async runtime. No `tokio`, no `mio`, no `crossbeam`. No `epoll` — the standard
library exposes no readiness API of any kind, so the design is shaped entirely by the
fact that a blocking read is the only tool available.

`serde` is the single exception, admitted because hand-rolling a JSON parser teaches
lexing, not networking, and lexing is not what this project is about.

### Why bother

Rust's standard library is a portable OS abstraction plus collections, not a
batteries-included toolkit. Building on it directly surfaces things that runtimes
normally hide:

- Why goroutine-style concurrency exists, felt from the other side.
- What backpressure means when nothing provides it for you.
- Why cancellation is hard: a thread parked in a blocking read cannot be interrupted.
- What a connection actually costs in threads, stack, and file descriptors.

A side benefit: with two dependencies, a clean build takes seconds.

---

## Architecture

```
                       ┌──────────────────────────────┐
   TCP accept ────────►│  accept loop  (main thread)  │
                       └───────────────┬──────────────┘
                                       │ spawns 2 threads per connection
                ┌──────────────────────┴───────────────────────┐
                ▼                                              ▼
   ┌─────────────────────────┐                    ┌─────────────────────────┐
   │ reader thread           │                    │ writer thread           │
   │ blocking line read      │                    │ drains bounded queue    │
   │ decode client frame     │                    │ encode server frame     │
   └───────────┬─────────────┘                    └────────────▲────────────┘
               │ hub event                                     │ server frame
               │ (many producers, one consumer)                │ (bounded queue)
               ▼                                               │
        ┌──────────────────────────────────────────────────────┴─────┐
        │ hub thread                                                 │
        │ owns the roster: client id → outbound queue handle         │
        │ single writer of all shared state — no mutex on the        │
        │ broadcast path                                             │
        └────────────────────────────────────────────────────────────┘
```

Three decisions carry the design:

**Each socket is split in two.** Without a readiness API, a single thread cannot wait
on "socket readable *or* outbound queue non-empty". The stream is duplicated so one
owned half goes to a reader thread and one to a writer thread, and each blocks on
exactly one thing.

**The hub owns the roster outright.** Client state lives in one thread and is reached
only by message passing, so there is no shared map contended by every broadcast. The
hub is the serialisation point for all state changes.

**Every outbound queue is bounded.** When a client stops reading, its queue fills and
the hub declines to enqueue further messages, disconnecting the client instead of
allowing unbounded memory growth. This is the failure mode that unbounded channels let
you ignore until production.

Thread count is `2N + 2` for N connections. That is workable into the low thousands on
Linux; per-thread stack size is configurable if the ceiling needs raising.

---

## Wire protocol

Newline-delimited JSON. One object per line, UTF-8 encoded, terminated by a single
line feed. Framing is a buffered line read, which keeps the protocol debuggable with
any raw TCP client.

Every frame is a JSON object carrying a `type` field that names the variant; remaining
fields are variant-specific. Both directions are modelled as discriminated unions with
`type` as the tag. Unknown fields are rejected rather than ignored, so protocol drift
becomes a typed error instead of silent divergence. Unknown `type` values are likewise
rejected.

### Client to server

| `type` | Fields | Meaning |
| --- | --- | --- |
| `join` | `nick` (string) | Claim a nickname and enter the room. Must be the first frame. |
| `say` | `body` (string) | Broadcast a message to the room. |
| `ping` | — | Liveness probe. |
| `leave` | — | Leave cleanly; server closes the connection. |

### Server to client

| `type` | Fields | Meaning |
| --- | --- | --- |
| `welcome` | `nick` (string), `users` (array of string) | Join accepted; current roster. |
| `message` | `from` (string), `body` (string), `at` (integer) | A broadcast message. `at` is Unix seconds. |
| `joined` | `nick` (string) | Another client entered. |
| `left` | `nick` (string) | Another client departed. |
| `pong` | — | Reply to `ping`. |
| `error` | `code` (string), `detail` (string) | Rejection or fault; may precede a close. |

### Error codes

| Code | Cause |
| --- | --- |
| `bad_frame` | Malformed JSON, unknown `type`, or unknown field. |
| `frame_too_large` | Line exceeded the frame cap. Connection is closed. |
| `nick_taken` | Requested nickname is in use. |
| `nick_invalid` | Nickname fails the character or length rules. |
| `not_joined` | A frame other than `join` arrived before joining. |
| `rate_limited` | Client exceeded the message rate budget. |
| `server_shutdown` | Server is terminating. |

### Limits

These are protocol-level guarantees, not implementation details. A client can rely on
them; the server enforces them.

| Limit | Value | Behaviour on breach |
| --- | --- | --- |
| Maximum frame size | 8 KiB | `frame_too_large`, then close |
| Maximum message body | 4 KiB | `bad_frame` |
| Nickname length | 1–32 characters | `nick_invalid` |
| Nickname charset | alphanumeric, `_`, `-` | `nick_invalid` |
| Outbound queue depth | 64 frames | Client disconnected as unresponsive |
| Idle timeout | 120 seconds without a frame | Connection closed |

The frame cap matters more than it looks. Reading a line from an unbounded reader lets
a client drive the server's allocator, so reads are length-limited before parsing
rather than after.

### Session flow

1. Client connects; server accepts and spawns the reader and writer threads.
2. Client sends `join`. Any other frame at this point yields `not_joined`.
3. Server validates the nickname, registers the client with the hub, and replies
   `welcome` with the current roster.
4. Hub broadcasts `joined` to every other client.
5. Client sends `say` frames; the hub fans each one out as `message` to the room,
   including back to the sender so that ordering is server-authoritative.
6. Client sends `leave`, or the connection drops, or the idle timeout fires. The hub
   deregisters the client and broadcasts `left`.

---

## Layout

```
src/
  main.rs        # config, listener, accept loop, shutdown wiring
  hub.rs         # roster, fan-out, hub event handling
  conn.rs        # per-connection reader and writer threads
  protocol.rs    # frame definitions, error codes, limits
  shutdown.rs    # shutdown flag and the registry of live streams
```

---

## Build and run

Requires a stable Rust toolchain. Dependencies are `serde` (with the derive feature)
and `serde_json`, both at version 1.

Build and start the server:

```console
$ cargo build --release
$ cargo run --release -- --bind 127.0.0.1:7878
```

Command-line options:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--bind` | `127.0.0.1:7878` | Listen address and port |
| `--max-clients` | `1024` | Refuse connections beyond this count |
| `--idle-timeout` | `120` | Seconds before an idle connection is closed |

Any raw TCP client will do for testing — connect with `nc` or `telnet` and type
newline-terminated JSON objects by hand. Run the test suite with `cargo test`.

---

## Shutdown

There is no readiness API, and a thread blocked in a read cannot be cancelled, so
joining a reader thread will hang forever unless something breaks the read. Two
standard-library levers exist, and both are used:

- Shutting down a stream from another thread forces the blocked read on that stream to
  return. Every live stream is registered so the shutdown path can reach it.
- A read timeout gives each reader a periodic wake-up to observe the shutdown flag.
  The same mechanism provides idle-connection detection.

The standard library has no signal handling, so an interrupt at the terminal is a hard
kill. Graceful shutdown is triggered through an admin command on the socket. Handling
an interrupt properly would mean declaring the C signal-handling symbols directly,
which is deferred to the roadmap below.

---

## Non-goals

- TLS. There is no cryptography in Rust's standard library, by design.
- Persistence, message history, federation, authentication beyond a nickname.
- An async runtime, or matching one on connection count.

---

## Roadmap

1. **v0.1** — single broadcast room, join/say/leave, bounded outbound queues, clean
   shutdown, frame and nickname validation.
2. **v0.2** — multiple rooms, nickname collision handling, idle timeouts, the full
   error code set, rate limiting.
3. **v0.3** — a load-generating client to find where thread-per-connection actually
   breaks on this machine. Connection count, resident memory, and broadcast latency
   recorded here as measured numbers rather than estimates.
4. **Stretch: event loop.** Replace thread-per-connection with an epoll-driven loop,
   still without adding crates, by declaring the required C symbols directly. Zero
   dependencies, and a much clearer view of what a runtime does.
5. **Stretch: transport encryption.** A hand-rolled key exchange and authenticated
   stream cipher, as a study exercise only. Nothing produced here would be
   constant-time, audited, or fit for real data.
