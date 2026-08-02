<!-- SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception -->

# fetch body-streaming: optimization analysis (C++ vs. this implementation)

A comparison of the body-streaming paths in the C++ StarlingMonkey
(`starling-cpp/builtins/web/{fetch,streams}`) and this Rust implementation, with
the optimizations each has, the gaps, and a plan for each. "C++" below means the
reference implementation.

## Status legend
- **DONE** — implemented here.
- **PLAN** — analysed, with a concrete approach; not yet implemented.

---

## 1. Eager-pull avoidance on the host body source — DONE

**C++:** `NativeStreamSource::create` overrides the stream's high-water mark to
`0.0` ("To prevent an eager pull … we enqueue a read from the host handle even
though we often have no interest in it at all").

**Here:** `web_streams::body::create_pull_stream` now creates the host `.body`
stream with HWM `0`, so it reads the host only on demand. This matters for the
shortcuts below (an unread body must stay unread) and avoids buffering a chunk
nobody asked for.

## 2. Direct incoming→outgoing shortcut (`fetch(b, {body: a.body})`) — DONE

A response body used directly as another request's body is handed straight to
the outgoing request rather than pumped through JS.

**Here:** `RequestBody::Host(ResponseBody)`. On **wasm** this is a true zero-copy
host pipe — the incoming `wasi:http` body stream is passed as the outgoing
`Request`'s contents, and (observed via raw-socket capture) the host even
preserves `Content-Length`. On **native** reqwest streams the response body into
the outgoing request (chunked). The host-backed `.body` stream is recognised via
`ReadableStream::native_source` (its controller's algorithm receiver, a
`HostBodySource`), and `HostBodySource::take_host_body` hands off the unread body.

This is arguably *better* than the C++ for the direct case: the C++ direct path
reads the body chunk-by-chunk through the `NativeStreamSource` pull; here wasm
does a single zero-copy handoff.

## 3. Complex shortcut: `fetch(b, {body: a.body.pipeThrough(new TransformStream())})` — DONE

Implemented by **propagating the native source through an identity transform**,
reusing the direct-case detection:
- A `ReadableStream` carries a private `native_byte_source` slot (the host source
  object), set by `create_pull_stream` and read by `ReadableStream::native_source`.
- A `TransformStream` with no transform algorithm (identity) links its writable
  to its readable (`WritableStream::identity_transform_readable`, a private slot)
  at construction.
- `readable_stream_pipe_to`, when a native-source stream is piped into an identity
  transform's writable, copies the native source onto the transform's readable.
- `fetch`'s existing check (`requestB.body`'s `native_source` is a `HostBodySource`
  → `take_host_body` → `RequestBody::Host`) then fires for `ts.readable`
  unchanged.

So `a.body.pipeThrough(new TransformStream())` used as a request body hands `a`'s
host body straight through; a **non-identity** transform is not linked, so it
falls back to the pump and its transform runs. HWM 0 keeps `a.body` unread until
the shortcut takes it; the now-idle pipe is a paused, unsettled promise that does
not keep the event loop alive (verified: the process exits). Verified on both
platforms (native chunked, wasm zero-copy Content-Length) with integration tests
for both the identity and non-identity cases; streams WPT unchanged (1354/1355).

The original analysis follows, for reference.

### Original analysis

When a response body is piped through an **identity** `TransformStream` into
another request, the whole incoming body can be appended to the outgoing request
in one host operation, instead of copying chunk-by-chunk through the transform.

**C++ mechanism** (`request-response.cpp`, `native-stream-source.cpp`,
`transform-stream.cpp`):
- A wrapped `pipeTo` records, on the source's `NativeStreamSource`, the
  `TransformStream` it is piped to — **only if** the TS has no transformer
  (`!HasTransformer`, i.e. identity) and the source is host-backed.
- The TS records that its readable end is used as a body
  (`set_readable_used_as_body`).
- The source's **pull algorithm** is backpressure-aware: while the destination
  TS has backpressure (the request body has not been pulled yet) it waits on the
  `backpressureChangePromise` rather than reading, so the source stays unread.
- When finally pulled, if piped to such a TS, it calls
  `HttpOutgoingBody::append(HttpIncomingBody)` — a host call that splices the
  incoming body onto the outgoing one — and closes, instead of reading chunks.

**Why it is not a drop-in here.** Two structural mismatches:
1. *Spec-fidelity:* `ReadableStream`/`TransformStream`/controller structs here
   mirror the spec's internal slots exactly (a project rule). The C++ adds
   private slots (`PipedToTransformStream`, `HasTransformer`,
   `readable_used_as_body`); the equivalent here would be either non-spec struct
   fields (disallowed) or internal object-property markers.
2. *Send-time vs. pull-time:* the outgoing body here is decided at *send* time
   (`platform_request` builds a `OutgoingBody`), whereas the C++ splices at *pull*
   time via `HttpOutgoingBody::append`. A send-time detection
   (`requestB.body` is an identity-TS readable whose writable is piped from an
   unread host-backed source → take the host body) is possible, but then the now
   unused `pipeTo` is left dangling and must be torn down; its correctness
   depends on the pipe not having pre-read the source (HWM 0 helps, but the exact
   `pipeTo` backpressure timing must be verified).

**Functional fallback (works today):** with no shortcut, this case still streams
correctly — `requestB.body` (the TS readable) is pumped by `request_body.rs`,
fed by the pipe reading `a.body` (the host source) chunk-by-chunk. Verified
end-to-end. The shortcut is purely a throughput/zero-copy optimization.

**Plan.** Mirror the C++ pull-time model: give the platform outgoing body an
`append(incoming)` operation (wasm: pass the incoming stream as contents — the
mechanism already used for the direct case; native: stream reqwest→reqwest), add
identity-TS + pipe-source tracking via internal markers, and make the
`HostBodySource` pull splice-and-close when piped to an identity TS used as a
body. Gate behind streams-WPT verification (the streams suite passes 1354/1355
and must not regress).

## 4. Request-body backpressure — DONE

`body_channel` is now a **bounded** channel (`BODY_CHANNEL_CAPACITY` chunks). The
pump's send awaits channel capacity: after reading a chunk it issues the send
(driven by the event loop's future driver), and only once the chunk is accepted
does its completion read the next chunk. So the pump paces itself to the peer —
a fast `ReadableStream` request body no longer buffers unboundedly when the peer
is slow. The "wait for capacity" is the bounded `Sender`'s `poll_ready`, awaited
in a per-send future (the same mechanism the host body uses), which sidesteps the
"reactions can't await" problem. The incoming→outgoing shortcut bypasses the pump
entirely, so it is unaffected. Verified on both platforms; GC-zeal clean.

## 5. Allocation reductions — DONE

- **Native response body copy — DONE.** `ResponseBody::read_all`/`next_chunk`
  return `BodyBytes` (a type alias: reqwest's `Bytes` on native, `Vec<u8>` on
  wasm) instead of `Vec<u8>`. On native this drops the `Bytes`→`Vec` copy that
  happened on every consumed body / streamed chunk — the body is read straight
  from reqwest's buffer (`BodyBytes` derefs to `&[u8]` for the consumers). On
  wasm it is unchanged (the `Vec` read from the stream).
- **Rust→JS copy on `.body` reads — DONE.** `ArrayBuffer::from_external` hands
  the host bytes to the engine without copying via `NewExternalArrayBuffer`: the
  engine owns them and, when the chunk's buffer is collected, calls a per-`D`
  free callback that drops the Rust `D` (so allocators match). `.body`'s pull
  enqueues each chunk this way. SpiderMonkey requires external contents aligned
  to `ARRAY_BUFFER_ALIGNMENT` (8); `from_external` checks and copies into a
  normal buffer when unaligned — which happens for a `bytes::Bytes` sub-slice at
  an odd offset, but a fresh host read buffer (and reqwest's first whole-body
  chunk) is aligned, so the common case is no-copy on **both** platforms.
  Verified GC-zeal clean (the external-buffer free-callback lifecycle). This is
  the Rust equivalent of the C++ `NewArrayBufferWithContents` path (the C++
  always succeeds because its host bytes are engine-allocated and aligned).
- *Inherent:* the JS→Rust copy in the request pump
  (`copy_buffer_source_bytes`) is at the host↔JS boundary; eliding it would mean
  detaching the JS chunk's buffer, which has content-observable effects.
  `platform_request` cloning the header list is required (it cannot move out of
  the GC-owned `Headers`). The per-chunk `PromiseFuture` future on the `.body` pull
  is structural (the C++ reuses one `BodyFutureTask`); reducing it is a larger
  change for a small constant-factor gain.
- **`consume` `arrayBuffer()`/`bytes()` copy — DONE.** `convert_owned_bytes_to_js_value`
  takes the whole body by value and backs the `ArrayBuffer`/`Uint8Array` with it
  via `from_external` (no copy when aligned; a whole-body read is). Text/JSON
  still borrow and decode. GC-zeal clean.
