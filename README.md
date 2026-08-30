# chat_rust

A multi-user TCP chat server written in Rust, where **all networking, threading, and
synchronisation comes from the standard library**. The only dependencies are `serde`
and `serde_json`, and only for encoding and decoding the wire protocol.

This is a learning project. The constraint is the point.

---

## The constraint

| Concern | Implementation |
| --- | --- |
| Sockets | `std::net::{TcpListener, TcpStream}` |
| Concurrency | `std::thread`, one thread per socket direction |
| Message passing | `std::sync::mpsc`, `std::sync::atomic` |
| Buffering | `std::io::{BufReader, BufWriter}` |
| Wire format | `serde` + `serde_json` |

No async runtime. No `tokio`, no `mio`, no `crossbeam`. No `epoll` — the standard
library exposes no readiness API of any kind, so the design is shaped entirely by the
fact that a blocking `read()` is the only tool available.

`serde` is the single exception, admitted because hand-rolling a JSON parser teaches
lexing, not networking, and lexing is not what this project is about.

### Why bother

Rust's standard library is a portable OS abstraction plus collections, not a
batteries-included toolkit. Building on it directly surfaces things that runtimes
normally hide:

- Why goroutine-style concurrency exists, felt from the other side.
- What backpressure means when nothing provides it for you.
- Why cancellation is hard: a thread parked in `read()` cannot be interrupted.
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
   │ BufReader::read_line    │                    │ drains SyncSender queue │
   │ serde_json::from_str    │                    │ serde_json::to_writer   │
   └───────────┬─────────────┘                    └────────────▲────────────┘
               │ HubEvent                                      │ ServerFrame
               │ (mpsc, many producers)                        │ (sync_channel, bounded)
               ▼                                               │
        ┌──────────────────────────────────────────────────────┴──────┐
        │ hub thread                                                  │
        │ owns HashMap<ClientId, SyncSender<ServerFrame>>             │
        │ single writer of all shared state — no Mutex on the         │
        │ broadcast path                                              │
        └─────────────────────────────────────────────────────────────┘
```

Three decisions carry the design:

**`TcpStream::try_clone()` splits the socket.** Without a readiness API, a single
thread cannot wait on "socket readable or outbound queue non-empty". Cloning the
stream hands one owned half to a reader thread and one to a writer thread, and each
blocks on exactly one thing.

**The hub owns the roster outright.** Client state lives in one thread and is reached
only by message passing, so there is no `Arc<Mutex<HashMap>>` contended by every
broadcast. The hub is the serialisation point.

**Per-client queues are bounded.** Each client gets a `sync_channel(OUTBOX_DEPTH)`.
When a client stops reading, its queue fills and the hub's send fails or blocks
depending on the policy chosen — the slow client is disconnected rather than allowed
to grow unbounded memory. This is the failure mode a runtime with unbounded channels
lets you ignore until production.

Thread count is `2N + 2` for N connections. That is fine into the low thousands on
Linux; `thread::Builder::stack_size` lowers the per-thread cost if needed.

---

## Wire protocol

Newline-delimited JSON (JSON Lines) over TCP. One object per line, UTF-8, `\n`
terminated. Framing is `BufReader::read_line`, which means the protocol is fully
debuggable with `nc`.

Frames are internally tagged enums, so the type system carries the protocol:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Join { nick: String },
    Say  { body: String },
    Ping,
    Leave,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Welcome { nick: String, users: Vec<String> },
    Message { from: String, body: String, at: u64 },
    Joined  { nick: String },
    Left    { nick: String },
    Pong,
    Error   { code: ErrorCode, detail: String },
}
```

Adding a variant makes every non-exhaustive `match` a compile error, which is the
intended migration workflow. `deny_unknown_fields` turns protocol drift into a typed
rejection instead of a silently ignored field.

**Limits.** Lines are capped (`MAX_FRAME_BYTES`); a client exceeding the cap is
disconnected rather than allowed to drive the server's allocator. `read_line` on an
unbounded reader is a denial-of-service vector and is not used without a `take()`.

### Example session

```console
$ nc 127.0.0.1 7878
{"type":"join","nick":"rahul"}
{"type":"welcome","nick":"rahul","users":["asha"]}
{"type":"say","body":"hello"}
{"type":"message","from":"asha","body":"hi","at":1756483200}
```

---

## Layout

```
src/
  main.rs        # config, listener, accept loop, shutdown wiring
  hub.rs         # roster, fan-out, HubEvent handling
  conn.rs        # per-connection reader and writer threads
  protocol.rs    # ClientFrame, ServerFrame, ErrorCode, limits
  shutdown.rs    # AtomicBool + registry of streams to shutdown()
```

---

## Build and run

```console
$ cargo run --release -- --bind 127.0.0.1:7878
```

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## Shutdown

There is no `select`, and a thread blocked in `read()` cannot be cancelled, so
`join()` on a reader thread will hang forever unless something breaks the read. Two
std-only levers exist, and both are used:

- `TcpStream::shutdown(Shutdown::Both)` from another thread forces the blocked read to
  return. Every live stream is registered so the shutdown path can reach it.
- `set_read_timeout` gives readers a periodic wake-up to observe the shutdown flag,
  which also serves as idle-connection detection.

The standard library has no signal handling, so `Ctrl-C` is a hard kill. Graceful
shutdown is triggered through an admin command on the socket. Wiring `SIGINT` would
require declaring the libc symbol directly — see below.

---

## Non-goals

- TLS. There is no crypto in Rust's standard library, by design.
- Persistence, history, federation, authentication beyond a nickname.
- An async runtime, or matching one on connection count.

---

## Roadmap

1. **v0.1** — single broadcast room, join/say/leave, bounded outboxes, clean shutdown.
2. **v0.2** — rooms, nickname collision handling, idle timeouts, structured errors.
3. **v0.3** — a load-generating client to find where thread-per-connection actually
   breaks on this machine, with numbers recorded here.
4. **Stretch: event loop.** Replace thread-per-connection with `epoll`, still without
   crates, by declaring the syscalls directly:

   ```rust
   unsafe extern "C" {
       fn epoll_create1(flags: core::ffi::c_int) -> core::ffi::c_int;
   }
   ```

   Zero dependencies, and a much clearer view of what a runtime does.
5. **Stretch: transport encryption.** A hand-rolled X25519 + ChaCha20-Poly1305
   handshake, as a study exercise only. Nothing here is constant-time, audited, or fit
   for any real data.
