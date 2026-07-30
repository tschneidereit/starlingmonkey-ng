// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The wasm backend of [`http`](crate::http): the WASIp3 `wasi:http` client for
//! sending, and [`IncomingBody`] wrapping a `wasi:http` body stream.

use futures_lite::StreamExt;
use wasip3::http::types::{
    Fields, Method, Request as WasiRequest, Response as WasiResponse, Scheme,
};
use wasip3::{wit_bindgen, wit_future, wit_stream};

use crate::http::{BodyBytes, Error, OutgoingBody, Request, Response};

type BodyStreamReader = wasip3::wit_bindgen::rt::async_support::StreamReader<u8>;
type BodyStreamWriter = wasip3::wit_bindgen::rt::async_support::StreamWriter<u8>;
type TrailersResult = Result<Option<Fields>, wasip3::http::types::ErrorCode>;
type TrailersWriter = wasip3::wit_bindgen::rt::async_support::FutureWriter<TrailersResult>;
type TrailersReader = wasip3::wit_bindgen::rt::async_support::FutureReader<TrailersResult>;

/// The pieces of an [`OutgoingBody`] handed to the host: the contents stream,
/// the trailers future, and, when we own the bytes, the writer half for
/// [`spawn_body_writer`] to feed.
struct BodyParts {
    contents: Option<BodyStreamReader>,
    /// The trailers future given to the host. For a piped host body this is
    /// the pipe source's trailers reader, so an abort on the incoming body
    /// propagates to the peer instead of looking like a clean end; otherwise
    /// it is a fresh future [`spawn_body_writer`] resolves.
    trailers: TrailersReader,
    writer: Option<(BodyStreamWriter, OutgoingBody)>,
}

/// Split `body` into the pieces the host needs. A host body is handed
/// straight through, trailers and all. A body-less request/response
/// gets no stream at all.
fn body_contents(body: OutgoingBody) -> (BodyParts, TrailersWriter) {
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (contents, piped_trailers, writer) = match body {
        OutgoingBody::Host(response_body) => {
            (Some(response_body.stream), response_body.trailers, None)
        }
        OutgoingBody::Consumed => (None, None, None),
        OutgoingBody::Bytes(bytes) if bytes.is_empty() => (None, None, None),
        other => {
            let (body_tx, body_rx) = wit_stream::new();
            (Some(body_rx), None, Some((body_tx, other)))
        }
    };
    (
        BodyParts {
            contents,
            trailers: piped_trailers.unwrap_or(trailers_rx),
            writer,
        },
        trailers_tx,
    )
}

/// Feed the owned body (in-memory, or streamed through the channel) to its
/// wit stream from a spawned task, then write the trailers: `Ok(None)` on a
/// clean end, an `ErrorCode` when a streamed chunk errored. The trailers
/// future is the only abort signal a wit stream's consumer can observe, so
/// an aborted body must not look like a clean (truncated) one.
fn spawn_body_writer(
    writer_body: Option<(BodyStreamWriter, OutgoingBody)>,
    trailers_tx: TrailersWriter,
) {
    wit_bindgen::spawn(async move {
        let mut error: Option<Error> = None;
        if let Some((mut body_tx, body)) = writer_body {
            match body {
                OutgoingBody::Bytes(bytes) => {
                    // Zero-copy when the bytes are uniquely owned and Vec-backed.
                    let _ = body_tx.write_all(Vec::from(bytes)).await;
                }
                OutgoingBody::Stream(mut receiver) => {
                    while let Some(result) = receiver.next().await {
                        match result {
                            Ok(chunk) => {
                                let _ = body_tx.write_all(chunk).await;
                            }
                            Err(e) => {
                                error = Some(e);
                                break;
                            }
                        }
                    }
                }
                OutgoingBody::Host(_) | OutgoingBody::Consumed => {}
            }
            drop(body_tx);
        }
        let trailers = match error {
            None => Ok(None),
            Some(e) => Err(wasip3::http::types::ErrorCode::InternalError(Some(
                e.to_string(),
            ))),
        };
        let _ = trailers_tx.write(trailers).await;
    });
}

/// Map a method name onto a `wasi:http` method.
///
/// Matched case-sensitively, and anything unrecognized is passed through as
/// `Other`: Fetch only normalizes the case of `DELETE`, `GET`, `HEAD`,
/// `OPTIONS`, `POST` and `PUT`, so `fetch(url, { method: "patch" })` must go
/// on the wire as `patch`.
fn method_of_string(method: &str) -> Method {
    match method {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "CONNECT" => Method::Connect,
        "OPTIONS" => Method::Options,
        "TRACE" => Method::Trace,
        "PATCH" => Method::Patch,
        _ => Method::Other(method.to_string()),
    }
}

fn string_of_method(method: Method) -> String {
    match method {
        Method::Get => "GET".to_string(),
        Method::Head => "HEAD".to_string(),
        Method::Post => "POST".to_string(),
        Method::Put => "PUT".to_string(),
        Method::Delete => "DELETE".to_string(),
        Method::Connect => "CONNECT".to_string(),
        Method::Options => "OPTIONS".to_string(),
        Method::Trace => "TRACE".to_string(),
        Method::Patch => "PATCH".to_string(),
        Method::Other(other) => other,
    }
}

pub async fn send(request: Request) -> Result<Response, Error> {
    let parsed = url::Url::parse(&request.url).map_err(|e| Error(format!("invalid URL: {e}")))?;
    let scheme = match parsed.scheme() {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        other => Scheme::Other(other.to_string()),
    };
    let authority = match (parsed.host_str(), parsed.port()) {
        (Some(host), Some(port)) => Some(format!("{host}:{port}")),
        (Some(host), None) => Some(host.to_string()),
        (None, _) => None,
    };
    let path_with_query = match parsed.query() {
        Some(query) => format!("{}?{}", parsed.path(), query),
        None => parsed.path().to_string(),
    };

    let mut header_entries: Vec<(String, Vec<u8>)> = request
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), crate::http::isomorphic_encode(value)))
        .collect();
    // An in-memory body has a known length: advertise it with Content-Length (as a
    // non-streaming client would) so the peer frames it correctly. A streamed body of unknown
    // length is sent without one.
    if let OutgoingBody::Bytes(bytes) = &request.body {
        if !bytes.is_empty()
            && !header_entries
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        {
            header_entries.push((
                "Content-Length".to_string(),
                bytes.len().to_string().into_bytes(),
            ));
        }
    }
    let fields =
        Fields::from_list(&header_entries).map_err(|e| Error(format!("invalid headers: {e:?}")))?;

    let (body, trailers_tx) = body_contents(request.body);
    let (wasi_request, _result) = WasiRequest::new(fields, body.contents, body.trailers, None);

    wasi_request
        .set_method(&method_of_string(&request.method))
        .map_err(|()| Error("invalid request method".to_string()))?;
    wasi_request
        .set_scheme(Some(&scheme))
        .map_err(|()| Error("invalid request scheme".to_string()))?;
    wasi_request
        .set_authority(authority.as_deref())
        .map_err(|()| Error("invalid request authority".to_string()))?;
    wasi_request
        .set_path_with_query(Some(&path_with_query))
        .map_err(|()| Error("invalid request path".to_string()))?;

    // Write the body we own (in-memory or channel-streamed) and the trailers once the
    // request exists, so the host is ready to read from the stream. A host-piped body has no
    // writer here: the host reads its stream directly.
    spawn_body_writer(body.writer, trailers_tx);

    // Send the request via the outgoing HTTP client.
    let response = wasip3::http::client::send(wasi_request)
        .await
        .map_err(|e| Error(format!("fetch failed: {e:?}")))?;

    let status = response.get_status_code();
    let headers = response
        .get_headers()
        .copy_all()
        .into_iter()
        .map(|(name, value)| (name, crate::http::isomorphic_decode(&value)))
        .collect();

    let (result_tx, result_rx) = wit_future::new(|| Ok(()));
    let (body_stream, trailers) = WasiResponse::consume_body(response, result_rx);
    // The result future is unused; resolve it so it is not left dangling.
    wit_bindgen::spawn(async move {
        let _ = result_tx.write(Ok(())).await;
    });

    Ok(Response {
        status,
        headers,
        body: IncomingBody {
            stream: body_stream,
            trailers: Some(trailers),
        },
    })
}

/// Read an incoming `wasi:http` request into its parts (method, full URL, headers, body).
pub async fn read_incoming_request(
    request: WasiRequest,
) -> (String, String, Vec<(String, String)>, IncomingBody) {
    let method = string_of_method(request.get_method());
    let scheme = match request.get_scheme() {
        Some(Scheme::Https) => "https".to_string(),
        Some(Scheme::Other(other)) => other,
        _ => "http".to_string(),
    };
    let authority = request
        .get_authority()
        .unwrap_or_else(|| "localhost".to_string());
    let path = request
        .get_path_with_query()
        .unwrap_or_else(|| "/".to_string());
    let url = format!("{scheme}://{authority}{path}");
    let headers = request
        .get_headers()
        .copy_all()
        .into_iter()
        .map(|(name, value)| (name, crate::http::isomorphic_decode(&value)))
        .collect();

    // Take the body's stream without reading it.
    let (result_tx, result_rx) = wit_future::new(|| Ok(()));
    let (body_stream, trailers) = WasiRequest::consume_body(request, result_rx);
    wit_bindgen::spawn(async move {
        let _ = result_tx.write(Ok(())).await;
    });
    (
        method,
        url,
        headers,
        IncomingBody {
            stream: body_stream,
            trailers: Some(trailers),
        },
    )
}

/// Build a `wasi:http` response from parts.
///
/// The body is written from a spawned task: an in-memory body inline,
/// a host body handed straight through, a stream body forwarded chunk by chunk.
pub fn build_outgoing_response(
    status: u16,
    headers: Vec<(String, String)>,
    body: OutgoingBody,
) -> WasiResponse {
    let header_entries: Vec<(String, Vec<u8>)> = headers
        .iter()
        .map(|(name, value)| (name.clone(), crate::http::isomorphic_encode(value)))
        .collect();
    let fields = Fields::from_list(&header_entries).unwrap_or_else(|_| Fields::new());

    let (body, trailers_tx) = body_contents(body);
    let (response, _result) = WasiResponse::new(fields, body.contents, body.trailers);
    let _ = response.set_status_code(status);

    spawn_body_writer(body.writer, trailers_tx);
    response
}

/// A body read lazily from the host's incoming `wasi:http` body stream: an HTTP
/// client response, or an incoming request body on the serve path.
pub struct IncomingBody {
    stream: BodyStreamReader,
    /// The body's trailers future, taken when the body ends.
    ///
    /// `wasi:http` communicates errors that abort the stream through the trailers
    /// future, so we store it here to consult once the body stream closed.
    trailers: Option<TrailersReader>,
}

impl IncomingBody {
    /// Read the entire body to a byte sequence.
    pub async fn read_all(self) -> Result<BodyBytes, Error> {
        // `collect` consumes the stream, so destructure first and consult the trailers directly
        // rather than through `end_of_body` (which needs `self`).
        let IncomingBody { stream, trailers } = self;
        let bytes = bytes::Bytes::from(stream.collect().await);
        if let Some(trailers) = trailers {
            trailers
                .await
                .map_err(|code| Error(format!("the body was aborted: {code:?}")))?;
        }
        Ok(bytes)
    }

    /// Read the next chunk, or `Ok(None)` at end of body.
    pub async fn next_chunk(&mut self) -> Result<Option<BodyBytes>, Error> {
        use wasip3::wit_bindgen::rt::async_support::StreamResult;
        let buffer = Vec::with_capacity(8192);
        let (status, buffer) = self.stream.read(buffer).await;
        match status {
            // Values were transferred, so the stream is still open. Report them — even a
            // zero-length read, which must *not* fall through to `end_of_body`: the trailers
            // future only resolves once the stream is closed, so awaiting it here would hang.
            StreamResult::Complete(_) => Ok(Some(bytes::Bytes::from(buffer))),
            // The writer dropped its handle: the body has ended. Whether it ended cleanly or
            // the peer aborted it is only knowable from the trailers.
            StreamResult::Dropped => {
                if !buffer.is_empty() {
                    return Ok(Some(bytes::Bytes::from(buffer)));
                }
                self.end_of_body().await.map(|()| None)
            }
            // The read was cancelled, which is a teardown rather than an end of body. The
            // stream was never reported closed, so the trailers may never resolve — do not
            // await them.
            StreamResult::Cancelled => Ok(None),
        }
    }

    /// The body's stream has ended: resolve the trailers future to find out
    /// whether it ended cleanly or the peer aborted it, and surface an abort as
    /// an error so a truncated body is not mistaken for a complete one.
    ///
    /// Only valid once the stream has been reported closed.
    ///
    /// This follows the documentation for `wasi:http`'s `consume-body`:
    ///    Once the stream is reported as closed, callers should await the returned
    ///    future to determine whether the body was received successfully. The future
    ///    will only resolve after the stream is reported as closed.
    async fn end_of_body(&mut self) -> Result<(), Error> {
        let Some(trailers) = self.trailers.take() else {
            return Ok(());
        };
        match trailers.await {
            Ok(_) => Ok(()),
            Err(code) => Err(Error(format!("the body was aborted: {code:?}"))),
        }
    }
}
