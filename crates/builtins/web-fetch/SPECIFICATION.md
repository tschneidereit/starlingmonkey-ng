# web-fetch — Implementation Specification & Plan

Implementation of the [WHATWG Fetch Standard](https://fetch.spec.whatwg.org/) for
starling-ng. This document captures the design decisions, scope, architecture, the
platform abstraction layer, milestones, and known gotchas. It is the living spec for the
work and is updated whenever requirements or understanding change.

## Scope decisions (confirmed with maintainer, 2026-06-09)

1. **Pragmatic, direct-to-transport `fetch()`.** This is a non-browser runtime: there is
   no DOM document, browsing context, cookie jar, service worker, or origin model. The
   spec's browser-protocol machinery — CORS preflight, the HTTP cache, cookie handling,
   COEP/CORP, redirect taint, referrer policy, mixed-content, deferred-fetch quota — is
   meaningless here and is **not** WPT-verifiable offline (the harness spawns no HTTP
   server). `fetch()` is implemented as: build a request record → hand it to a host
   transport → produce a `Response`. Simple redirect following only.
2. **Both native and wasm**, via a **reusable platform abstraction layer** so other
   builtins that need distinct wasi/native backends can share the pattern. wasm uses
   wasip3's `outgoing-handler`; native uses an async HTTP client (no blocking — the
   runtime forbids `block_on`).
3. *(Amended 2026-08-01.)* The browser-only abstract ops were initially kept as untouched
   `todo!()` scaffolds with their verbatim `// Step N:` comments; they have since been
   **removed outright** (restorable from the `e9f703b` baseline below). Algorithms fully
   inlined elsewhere lost their scaffolds too — the inline sites carry the step comments,
   prefixed `[inlined]`, with the algorithm's spec section linked at each mention. Only
   the soon-to-be-needed ops (MIME type, Range) remain as `todo!()` scaffolds; see the
   `algorithms.rs` module docs for the full policy.

The verbatim-step-comment baseline is commit `e9f703b` ("fetch: raw scaffolding").

## What exists in the runtime (survey results)

Usable as-is (Rust API): `TextEncoder`/`TextDecoder` (encoding_rs), `DOMException` +
`DOMExceptionError` + `throw_dom_exception`, structuredClone `write`/`read`, the `url`
crate (2.5), JS `Promise` ↔ Rust future bridge (`js::Promise`/`PromiseFuture`,
`PromiseOutcome`), the `Task` trait + `EventLoop` (`signal_ready`, `queue`), native
callbacks (`Function::new_callback`).

Exists but needs extension:
- **web-streams** — complete, but its create/read helpers are `pub(crate)`. Needs a small
  **public Rust API** to (a) create a `ReadableStream` already containing a byte buffer
  (closed), (b) create a `ReadableStream` backed by a native (Rust) pull source for
  streaming response bodies, (c) fully-read a `ReadableStream` to a byte sequence from
  Rust. Reusable by Blob/File later.
- **AbortSignal** (web-globals) — can check `aborted()`, read `reason()`,
  `throw_if_aborted()`; **cannot register custom abort algorithms** (field is
  `pub(crate)`, only models listener-removal). Needed only for in-flight `fetch()`
  cancellation (networking milestone), not for the offline surface.

Does not exist (graceful-degrade, out of scope this effort): **Blob/File** (scaffold only
in `specs/web-files`, not wired) → `blob()` rejects/`TypeError`; **FormData** (nonexistent)
→ `formData()` throws `TypeError`; **fetchLater / deferred fetch** (browser-only) → throws.

## Internal concept records (the hidden foundational task)

The scaffold's `request: Heap<Value>`, `response: Heap<Value>`, `header_list:
Heap<RequestImpl>`, `guard: Heap<RequestImpl>` are **not** real types — the spec's
internal *request*, *response*, *header list*, and *body* records have no Rust
representation. Designing them is the foundation the ~70 ops operate on. These are
scaffold-fixes (not step-comment edits). Planned shapes (`src/concept/`):

- **`HeaderList`** — ordered `Vec<(ByteString, ByteString)>` (ByteString = `Vec<u8>`),
  with the spec ops: contains/get/get-combined/append/delete/set/combine, sort-and-combine,
  set-cookie extraction. Header names compared byte-case-insensitively.
- **`Guard`** — enum `{ Immutable, Request, RequestNoCors, Response, None }`.
- **`Body`** — `{ source: BodySource, length: Option<u64>, stream: Option<Heap<…>> }`
  where `BodySource ∈ { None, Bytes(Vec<u8>) }` (a JS `ReadableStream` body has source
  `None`). The stream is materialized lazily.
- **`RequestRecord`** — method (`ByteString`), url + url list (`url::Url`), header list,
  body, plus the mode/credentials/cache/redirect/etc. enums (already in `enums.rs`),
  cache/keepalive/integrity/priority. Stored boxed and GC-traced via the owning
  `RequestImpl`.
- **`ResponseRecord`** — type (`ResponseType`), status (`u16`), status message
  (`ByteString`), header list, url list, body, plus aborted/timing flags as needed.

Because these contain a `Heap` (the lazily-created stream) and live inside the
GC-traced `RequestImpl`/`ResponseImpl`, they must be traced. Where a record holds no GC
cell it can be a plain `#[no_trace]`-able value.

## Platform abstraction layer

New crate **`crates/platform`** (name TBD: `host-api`/`platform`) exposing
backend-agnostic async host capabilities, the first being HTTP transport. Goal: a single
trait surface that core-runtime/builtins call, with `#[cfg]`-selected backends, so future
builtins (filesystem, sockets, …) follow the same pattern.

```
platform::http::{Request, Response, send}  // async
  - Request  { method, url, headers: Vec<(String,String)>, body: BodyStream }
  - Response { status, headers, body: BodyStream }
  - BodyStream: async chunked byte source/sink (no wasi:io; wasip3 streams on wasm)
```

- **wasm32-wasip2 backend** — wasip3 `outgoing-handler` + wasip3 async streams.
- **native backend** — async client (candidate: `reqwest` with rustls, default-features
  off, no blocking; vetted at the dependency step for latest version + async fit with the
  tokio current-thread runtime already used by `libstarling`).

`fetch()` translates the fetch request record → `platform::http::Request`, awaits
`send`, and translates `platform::http::Response` → a fetch response record + `Response`
object whose body is a native-source `ReadableStream` fed by the transport's `BodyStream`.

## Milestones

Each milestone is self-contained, committed, and GC-validated (the skill's gate:
debugmozjs build → keystone Rust test → GC-zeal → crown → verbatim-comment check → WPT →
fmt + check-all). Commit messages note the WPT delta.

**M0 — wiring + body-bridge spike (de-risk first).**
- Register `web-fetch` in `libstarling` (dep + `register_global_initializer`).
- Fix `globals.rs` to compile (`#[jsglobals]` rejects the `Promise` return style; per spec
  `fetch()` builds promise *p* and returns it, so the signature becomes
  `Result<HandleValue, ExnThrown>` — a scaffold-fix, not a step-comment edit). Make
  `fetch()`/`fetchLater()` graceful (reject/throw, no `todo!()` crash).
- Add the public web-streams Rust API and prove the bridge end-to-end with a keystone
  test: bytes → `ReadableStream` → fully-read → bytes; and a closed stream from bytes that
  JS can read via a reader. If intractable, the milestone shape changes — so this is the
  first code written.

Note: WPT tests + per-method graceful degradation are added **per milestone** (M1/M2/M3),
not in M0 — a non-crashing `Headers`/`Request`/`Response` constructor inherently requires
the real concept records, which are that milestone's foundational work. Recording a WPT
baseline of crashing constructors in M0 would be noise. The browser-only abstract ops
remained untouched `todo!()` throughout the milestones (unreachable in the pragmatic
path); they were removed after the implementation settled — see scope decision 3.

**M1 — Headers.** Concept `HeaderList` + `Guard`; the header abstract ops; the `Headers`
interface (constructor/append/delete/get/getSetCookie/has/set) and the
`iterable<ByteString,ByteString>` (entries/keys/values/forEach/`Symbol.iterator` — verify
macro support or implement manually). Fill, validate, normalize, fill-from-init (record vs
sequence vs Headers). Signal: `fetch/api/headers/*`.

**M2 — Request.** `RequestRecord`; the constructor (35+ steps), all getters, `clone`.
Body extraction (`extract a body with type`) for the offline `BodyInit` cases (string,
Uint8Array/ArrayBuffer/views, URLSearchParams; ReadableStream passthrough). AbortSignal
via `AbortSignal::new` / dependent-signal path. Signal: `fetch/api/request/*`.

**M3 — Response + body consumption.** `ResponseRecord`; constructor, getters, `clone`,
static `error`/`redirect`/`json`; `initialize a response`. Body mixin via `consume body`
+ `fully read`: `arrayBuffer`/`bytes`/`text`/`json`. `blob`/`formData` graceful-throw.
Signal: `fetch/api/response/*`, `fetch/api/body/*` (subset).

**M4 — platform crate + `fetch()` networking.** Status: **`fetch()` works on BOTH native and
wasm**, verified against the live WPT server. Done: M4a (stream-body reading), M4b (async-future
event-loop driver), M4c (`crates/platform` HTTP layer), M4d (native reqwest backend + `fetch()`
wiring), M4e (WPT-server harness), M4f (wasip3 wasm backend via `wasi:http/client.send`).
Native verified via the loopback test and real HTTPS from the CLI; wasm verified via wasmtime +
`wpt serve` (matches native, including GET/POST bodies, status, headers, UTF-8).

**Response bodies stream from the host (M4g, done).** `platform::send` returns once the status
and headers are available; the body is a `IncomingBody` read lazily from the host. Consuming it
(`text`/`json`/`arrayBuffer`/`bytes`) reads the host bytes directly, skipping the JS
`ReadableStream` (the consume shortcut). `.body` is a `ReadableStream` whose native source reads
one host chunk per pull and enqueues it, so streaming/large/unbounded bodies are delivered
progressively and never buffered whole (the controller is held across the host read with
`js::promise::RootedObject`).

**In-flight abort (M4g, done).** `fetch` registers an abort algorithm on the request's
`AbortSignal` (step 11). On abort it rejects the fetch promise with the abort reason, cancels the
in-flight request future (`js::promise::cancel_pending_future`, which closes the connection), and
— if the response was already delivered — errors the `.body` stream and cancels the in-flight
chunk read so pending reads reject. `AbortSignal`'s abort algorithm grew a general
`RunSteps(callback)` variant (`web_globals::signals::algorithms::add_abort_algorithm`). Verified
by integration tests (pre-abort and abort-during-body-read); the WPT abort tests are
Blob/FormData- or stash-coordination-bound, so they stay `SKIP`.

**WPT coverage.** The C++ harness's fetch tests are activated (71 enabled, 38 more listed and
skipped); out-of-scope/unfixable ones carry a `SKIP(reason)` prefix (Blob/FormData, HTTP cache, HTTP/2, redirect referrer/origin,
keepalive, referrer policy, multipart, `.asis`/non-ASCII/case-preserving header cases the native
client normalizes, `.sub` substitution the harness lacks). A comparison against the C++
expectations confirms this session introduced no regressions; the remaining C++-pass/we-fail
subtests are all Blob/FormData or CORS/SRI/`.sub`/`host_info`. Record conversion is generic over
the key type (`Record<K, V>`), so each enumerable own key runs the key type's own conversion: in a
`record<ByteString, ByteString>` an enumerable symbol key fails `ByteString` conversion (a
`TypeError`) and an out-of-range key throws before its value is read, matching the WebIDL operation
order — both `headers-record` proxy-trap conformance cases pass.

**Streaming outgoing bodies (M4g, done).** A body whose source is a `ReadableStream` is streamed
to the host rather than sent empty: a JS-side pump (`outgoing_body.rs`) drains the stream with a
reader in a loop of native reactions and sends each `BufferSource` chunk through a channel; the
platform transport reads that channel as it sends (reqwest `Body::wrap_stream` on native, the
WASIp3 request stream on wasm). The body is never buffered whole; a non-`BufferSource` chunk
aborts it. `platform::http::RequestBody` is `Bytes | Stream | Host | Consumed`; the redirect loop
replays byte bodies but a body-preserving redirect of a consumed stream is a network error.
Verified on both platforms (a raw-socket capture shows the identical chunked output).

`outgoing_body::outgoing_body_from_stream` is the single entry point for both directions an
outgoing body travels — a `fetch` request's body (`Request::platform_request`) and a `Response`
returned by a `fetch` handler (`Response::take_send_body`). It applies the incoming→outgoing
shortcut first (an unread host body is handed straight to the transport, leaving the donor stream
locked and disturbed as the pump path would) and falls back to the pump.

**Streaming incoming request bodies (done).** An incoming server request's body is read off the
transport as the handler consumes it, exactly as a `fetch` response's is: `Request` carries the
same `host_body`/`body_source` pair as `Response`, `Request::from_incoming` takes an unread
`platform::http::ResponseBody` rather than a `Vec<u8>`, and the consume shortcut, `.body`
materialization, `clone`, and the incoming→outgoing shortcut all apply to it. On native the
body is fed from the serve loop's connection reader through a bounded channel
(`platform::http::incoming_body_channel`), so a client uploading faster than the handler consumes
is paced by TCP backpressure; on wasm the `wasi:http` body stream is handed over unread. A
failure mid-body (a stalled or truncated upload, a malformed chunk) aborts the body with an error
rather than ending it, so the handler's stream rejects instead of seeing a short body as
complete. On wasm that error arrives on the body's trailers future, which `IncomingBody` keeps
and consults once the stream ends — a `wasi:http` body stream itself reports only *that* it
ended, never why.

Remaining: broader network-test coverage.

The wasm backend (`platform/src/http.rs`) imports `wasi:http/client.send` via
`wit_bindgen::generate!` (vendored WIT in `platform/wit/`, `wasi:http/types`+`wasi:clocks/types`
remapped to the `wasip3` crate so there's no type duplication, `async: true`). It builds the
outgoing request (`Fields::from_list` + `Request::new` + `set_method`/`set_scheme`/
`set_authority`/`set_path_with_query`, body via a stream with an explicit `Content-Length`), and
reads the response status/headers/body. The harness auto-runs `NET`-prefixed tests against
`wpt serve`; `response-null-body` is `SKIP-WASM` (wasmtime's wasi-http strictly rejects the
test's deliberately-malformed 204/304-with-body, which reqwest tolerates — a host-client
divergence, not a guest bug). Details below.

**M4 prerequisites & plan (discovered during M4a):**

1. **Async-future event-loop driver (PREREQUISITE — does not exist yet).** The JSPromise ↔
   Rust-future bridge in `crates/js/src/promise.rs` (`PromiseFuture`, `__spawn_promise`,
   `take_pending_futures`, `PromiseOutcome`) is *unwired*: `take_pending_futures` is never
   called and no builtin returns `PromiseFuture`. Before any async networking, `core-runtime`'s
   `run_to_completion`/`EventLoop` must grow a driver that polls the pending futures, settles
   their paired JS promises via `PromiseOutcome`, holds event-loop *interest* while a future
   is in flight, and wakes via `signal_ready`. This is self-contained and testable WITHOUT
   networking (a timer-backed future that resolves a promise), so build+verify it first.

2. **`crates/platform` HTTP abstraction** — backend-agnostic async `send(request) -> response`
   with chunked body streams. `#[cfg]`-selected backends:
   - **native**: `hyper` 1.x + `hyper-util` + `hyper-rustls` (pure-Rust TLS) on the existing
     tokio current-thread runtime. (No HTTP client is in the tree today.)
   - **wasm32-wasip2**: wasip3 `http::outgoing-handler` (`OutgoingRequest`/`Fields`/`handle` →
     `IncomingResponse`), mirroring `simple-http`'s server use of wasip3 async streams.

3. **`fetch()` networking** — per spec, create promise *p*, build a `platform::http::Request`
   from the request record (method, current URL, header list, body bytes via M4a's reader for
   stream bodies), `await send`, translate the response into a `Response` whose body is a
   native-source `ReadableStream` (buffered first cut: a byte-source body). Simple redirect
   following. In-flight abort needs the AbortSignal custom-algorithm hook (web-globals
   `abort_algorithms` is `pub(crate)` and listener-only today — extend it).

4. **Verification — a real WPT server is runnable.** The C++ harness
   (`starling-cpp/tests/wpt-harness/run-wpt.mjs`) starts the WPT suite's own server via
   `execFile("<wpt>/wpt", ["--no-h2", "serve"])` and runs tests against
   `http://web-platform.test:8000/`. Port the Rust harness (`tests/wpt-harness/run-wpt.mjs`)
   to optionally start `wpt serve` (needs the `web-platform.test` hosts entries + Python WPT)
   so the network `fetch/api/**` tests become the correctness signal. A native in-process
   loopback `TcpListener` integration test is a lighter complementary check.

The incoming→outgoing streaming shortcut (see the C++ reference section) is an M4+ optimization
layered on the native-source stream once buffered `fetch()` works.

## Gotchas / risks

- A `todo!()` reachable from JS becomes `MOZ_CRASH` and zeroes the whole WPT file. The M0
  graceful-degrade pass is mandatory before anything else is testable.
- The scaffold's abstract-op signatures are all `fn foo()` (no params) — codegen dropped
  them. Adding the real params per spec is a scaffold-fix, not a step-comment edit. **Never
  remove/alter the `// Step N:` comments.**
- WebIDL interfaces must pair `#[webidl_interface]` + `#[webidl_methods]` (enumerable
  methods); verify the scaffold's impls use `#[webidl_methods]`.
- GC: records hold a lazily-created stream `Heap`; trace correctly. When moving a `Heap`
  out of a traced container to a stack local, root then `drop` before any allocation
  (compacting-GC write-barrier hazard). Validate allocation-heavy paths under
  `JS_GC_ZEAL=14;1`.
- Lazy standard-constructor resolution (ArrayBuffer/Uint8Array created by body consumption)
  can GC-crash on first use under moving GC — pre-resolve in `add_to_global` if it
  surfaces.
- "a promise resolved with" ≠ `Promise.resolve` (extra tick) — relevant to consume-body
  reactions.

## Configurability: browser-security constraints (server-side)

StarlingMonkey is primarily a **server-side** runtime, where the browser-security Fetch
constraints (forbidden request/response headers, forbidden methods, no-CORS header/method
safelisting) are usually unwanted — a server must be able to set `Host`, `Content-Length`,
a `CONNECT` method, etc. These are gated behind a single switch,
`web_fetch::config::set_enforce_request_restrictions(bool)` (`config.rs`):

- **Default: enabled** (browser-compatible), which is what WPT expects.
- When **disabled**, `validate`'s forbidden-request/response-header checks, the constructor's
  forbidden-method and no-CORS-method checks, and the no-CORS header safelisting are all
  skipped. HTTP-correctness rules (header name/value validity, the `immutable` guard) are
  **always** enforced.
- Verified by `tests/config.rs` (the only path WPT can't cover, since WPT always runs with
  restrictions on). An embedder/CLI flag to flip the default is a future wiring step.

## Body-record design (the GC-clean split)

The spec's body is `{ stream, source, length }`. Storing the `ReadableStream` *inside* a
plain `Body` value would put a `Heap` in stack locals during construction — a real
moving-GC hazard (crown rejects it; see `docs/rooting.md`). So the stream is **hoisted out**:

- `concept::Body` = `{ source: BodySource (Null | Bytes), length, source_disturbed }` — plain
  data, stored `#[no_trace]` on the interface object.
- The stream is a separate `body_stream: Option<Heap<ReadableStreamImpl>>` **field** of the
  `Request`/`Response` interface object (macro-traced). `extract a body` returns the stream
  as a rooted `ReadableStream` handle; the constructor stores it into `body_stream` via
  `Heap::from`. A byte-source body materializes its stream lazily on `.body` access
  (`algorithms::body_stream_value`), via M0's `web_streams::body::readable_stream_from_bytes`.
- `consume body` fast-paths byte sources (no stream read); reading a `ReadableStream`-source
  body from Rust (the incremental read loop) is deferred (currently rejects).

## C++ reference: body-streaming shortcuts (informs M2–M4)

The old C++ StarlingMonkey (`starling-cpp/builtins/web/`) is the reference for body
handling. Key architecture to mirror:

- **Body = a (lazily-created) `ReadableStream` slot + a host body handle.** A byte-buffer
  body is materialized into a single-chunk stream lazily (exactly M0's
  `readable_stream_from_bytes`). An incoming (host) body's stream is backed by a *native
  source* that pulls 8 KiB chunks from the host handle on demand.
- **Incoming→outgoing shortcut** (`maybe_stream_body`, request-response.cpp): if a body
  owner is an incoming (host) body being sent as an outgoing request/response body, the
  host `incoming_body` is appended directly to the `outgoing_body` at the host API — *no JS
  chunking*. Detection: `is_incoming(body_owner)`.
- **Native stream source + identity-transform see-through**: a `ReadableStream` backed by a
  host source is detectable (`stream_has_native_source`); when such a stream is piped
  through a `TransformStream` with **no transformer** (`!HasTransformer` → an identity
  transform), the pipe is marked (`set_readable_used_as_body` / `PipedToTransformStream`)
  so the pull algorithm still does the direct host-level `append_body`, seeing *through* the
  identity transform. Backpressure defers the pull until the destination is ready.
- **Headers modes**: C++ keeps `HostOnly`/`CachedInContent`/`ContentOnly` to avoid pulling
  every header from the host per iteration. Our M1 is `ContentOnly` always (a Rust
  `Vec<(name, value)>`); host-caching is an M4 optimization layered in only if needed.

For M4 this means web-streams needs a **native-source `ReadableStream`** API (pull from a
Rust/host byte source) plus native-source + identity-transform detection so `fetch()` can
shortcut incoming→outgoing. With WASIp3 async streams the ideal is to pass the original
stream through, but the JS builtins must still carry the shortcut markers to enable it.
Recorded in agent memory for the M4 session.

## Out of scope (this effort)

CORS (preflight + checks), HTTP cache, cookies, COEP/CORP, redirect taint, referrer
policy, mixed content, service workers, deferred-fetch (`fetchLater`/quota), Blob/File and
FormData body types, `text_stream` (needs `TextDecoderStream` piping — present in repo but
deferred), blob:/file: scheme fetch. The browser-only scaffolds for these were removed
(restorable from `e9f703b`); what remains as `todo!()` in `algorithms.rs` is the ops that
will gain callers with Blob/FormData and Range support — `get the MIME type`, MIME
extraction and its `get, decode, and split` dependency, and single-range parsing. The
pipeline ops have no scaffolds; their inlined slices link the spec directly.

`data:` URLs *are* implemented (`algorithms::data_url_processor`,
`response::response_from_data_url`, dispatched from `globals::fetch`): they resolve
in-process with no transport, so they cost nothing to support and
`fetch/api/basic/scheme-data.any.js` covers them.

## Known gaps

Deliberate, with the reason each was not closed:

- **`statusText` is always the empty string.** Neither backend surfaces the `reason-phrase`
  the peer sent: hyper discards it while parsing and `wasi:http` has no field for it. It
  cannot be reconstructed from the status code either — a server may send any phrase, and
  WPT's own `status.py` sends `OMG` — so filling in the registered phrase would report a
  value the peer never sent, and `fetch/api/basic/status.h2.any.js` catches exactly that
  (the harness serves `--no-h2`, so it runs over HTTP/1.1 and is in effect an HTTP/1.x
  `statusText` test). Recovering the real phrase means owning the connection layer; see the
  keep-alive note in `platform/src/http.rs`, which wants the same thing.
- **No request timeout.** The reqwest client sets none, so a server that accepts a
  connection and never replies hangs the fetch until the signal aborts it. A default would
  break long-poll and streaming bodies, so this belongs in embedder configuration rather
  than being hardcoded — it should join `config.rs` when that grows.
- **`fetch`'s abort algorithm is retained while a response body is still readable.** That is
  required — aborting mid-read must error the stream — and it is detached when the body is
  consumed, when the fetch settles with nothing abortable, or when the signal aborts. A
  response whose body is *never* read or cancelled keeps its algorithm until the signal is
  collected. Closing that needs a body-completion hook on the `.body` stream itself.

## Known platform deviation: header value bytes 0x01–0x08, 0x0B–0x1F

The Fetch spec permits any byte in a header value except NUL/CR/LF (plus no
leading/trailing whitespace), and our own `Headers` validation implements exactly that.
The native transport stack cannot carry them, though: the `http` crate underneath
reqwest/hyper rejects every byte below 0x20 except TAB in *both* `HeaderValue::from_str`
and `from_bytes` (its `is_valid` is byte ≥ 32, ≠ 127, or TAB), so swapping the conversion
does not help — sending such values would require replacing the hyper stack. The
`header-values-normalize.any.js` subtests for `%01`–`%08`/`%0E`–`%1F` are therefore
recorded as expected FAILs; revisit if hyper relaxes `is_valid` (an opt-in for obs-text
control bytes has been discussed upstream) or if the native transport is ever replaced.
