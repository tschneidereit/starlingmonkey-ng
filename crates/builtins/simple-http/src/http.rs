// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Simple HTTP server builtin for StarlingMonkey.
//!
//! Provides an `http` JS module with a `listen(callback)` function.
//! When the WASIp3 HTTP handler receives a request, it creates a JS `Request`
//! object and calls the registered callback.

use std::cell::RefCell;
use std::rc::Rc;

use core_runtime::class::{create_instance, get_private_mut, throw_error};
use core_runtime::runtime::Runtime;
use core_runtime::{jsclass, jsmethods};
use js::gc::scope::RootScope;
use js::heap::Heap;
use js::native::{HandleValueArray, JSContext, JSObject, Value};
use js::value;

use wasip3::http::types::{ErrorCode, Fields, Method, Response as WasiResponse};
use wasip3::{wit_bindgen, wit_future, wit_stream};

// ============================================================================
// Thread-local state for the JS runtime and callback
// ============================================================================

thread_local! {
    /// The stored JS listener callback (set by `listen(cb)` from JS).
    static LISTENER_CALLBACK: RefCell<Option<Box<Heap<*mut JSObject>>>> =
        RefCell::new(None);

    /// The raw JSContext pointer, stored after runtime init.
    static JS_CX: RefCell<Option<*mut JSContext>> = RefCell::new(None);

    /// The global object, stored after runtime init.
    static JS_GLOBAL: RefCell<Option<Box<Heap<*mut JSObject>>>> =
        RefCell::new(None);

    /// The runtime instance, lazily initialized on first request.
    static HTTP_RUNTIME: RefCell<Option<Rc<Runtime>>> = RefCell::new(None);
}

// ============================================================================
// Request JS Class
// ============================================================================

/// Internal response state built up by the JS callback.
#[derive(Default)]
enum ResponseState {
    /// No response set yet.
    #[default]
    Pending,
    /// Full response with body (from `respond(status, body)`).
    Immediate { status: u16, body: Vec<u8> },
    /// Streaming response in progress (from `respondStreaming(status)`).
    Streaming { status: u16, chunks: Vec<Vec<u8>> },
    /// Streaming response fully buffered (after `closeBody()`).
    Done { status: u16, body: Vec<u8> },
}

#[jsclass(name = "Request")]
#[allow(dead_code)]
pub struct JSRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    #[no_trace]
    response_state: ResponseState,
}

#[jsmethods]
impl JSRequest {
    #[constructor]
    fn new() -> Self {
        Self {
            method: String::new(),
            url: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            response_state: ResponseState::Pending,
        }
    }

    #[static_method]
    pub fn listen(cx: &mut JSContext, args: &CallArgs) -> bool {
        if args.argc_ < 1 || !args.get(0).is_object() {
            unsafe { throw_error(cx, "listen() requires a callback function") };
            return false;
        }

        let callback_obj = args.get(0).to_object();

        let heap = Box::new(Heap::default());
        heap.set(callback_obj);
        LISTENER_CALLBACK.with(|cb| {
            *cb.borrow_mut() = Some(heap);
        });

        args.rval().set(value::undefined());
        true
    }

    #[method]
    fn method(&self) -> String {
        self.method.clone()
    }

    #[method]
    fn url(&self) -> String {
        self.url.clone()
    }

    #[method]
    fn respond(&mut self, status: u16, body: String) {
        self.response_state = ResponseState::Immediate {
            status,
            body: body.into_bytes(),
        };
    }

    #[method]
    fn respond_streaming(&mut self, status: u16) {
        self.response_state = ResponseState::Streaming {
            status,
            chunks: Vec::new(),
        };
    }

    #[method]
    fn write_body(&mut self, chunk: String) {
        if let ResponseState::Streaming { chunks, .. } = &mut self.response_state {
            chunks.push(chunk.into_bytes());
        }
    }

    #[method]
    fn close_body(&mut self) {
        if let ResponseState::Streaming { status, chunks } =
            std::mem::replace(&mut self.response_state, ResponseState::Pending)
        {
            let body: Vec<u8> = chunks.into_iter().flatten().collect();
            self.response_state = ResponseState::Done { status, body };
        }
    }
}

// ============================================================================
// WASIp3 HTTP Handler
// ============================================================================

wasip3::http::service::export!(HttpHandler);

struct HttpHandler;

impl wasip3::exports::http::handler::Guest for HttpHandler {
    async fn handle(
        request: wasip3::http::types::Request,
    ) -> Result<WasiResponse, ErrorCode> {
        // Extract info from the WASI request.
        let method = method_to_string(&request.get_method());
        let url = request.get_path_with_query().unwrap_or_default();
        let wasi_headers = request.get_headers();
        let headers: Vec<(String, String)> = wasi_headers
            .copy_all()
            .into_iter()
            .map(|(k, v)| (k, String::from_utf8_lossy(&v).to_string()))
            .collect();

        // Read request body.
        let (res_writer, res_reader) = wit_future::new(|| Ok(()));
        let (body_stream, _trailers_future) =
            wasip3::http::types::Request::consume_body(request, res_reader);
        let body_bytes = read_stream(body_stream).await;
        drop(res_writer);

        // Call the JS listener and get the response info.
        let rt = HTTP_RUNTIME.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if borrow.is_none() {
                *borrow = Some(Runtime::init_from_env());
            }
            borrow.as_ref().unwrap().clone()
        });
        let scope = rt.default_global();
        let (status, response_body) = call_js_listener(scope.cx_mut().raw_cx(), &method, &url, &headers, &body_bytes);

        // Build WASIp3 response.
        let (body_writer, body_reader) = wit_stream::new();
        let (trailers_writer, trailers_reader) = wit_future::new(|| Ok(None));

        let (response, _result_future) =
            WasiResponse::new(Fields::new(), Some(body_reader), trailers_reader);
        let _ = response.set_status_code(status);

        wit_bindgen::spawn(async move {
            let mut body_writer = body_writer;
            let _ = body_writer.write_all(response_body).await;
            drop(body_writer);
            let _ = trailers_writer.write(Ok(None)).await;
        });

        Ok(response)
    }
}

/// Read all bytes from a WASI stream.
async fn read_stream(mut stream: wit_bindgen::rt::async_support::StreamReader<u8>) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        match stream.next().await {
            Some(chunk) => result.push(chunk),
            None => break,
        }
    }
    result
}

/// Call the stored JS listener callback with a Request object.
///
/// Returns (status_code, body_bytes).
fn call_js_listener(
    cx: &mut JSContext,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> (u16, Vec<u8>) {
    let has_callback = LISTENER_CALLBACK.with(|cb| cb.borrow().is_some());
    if !has_callback {
        return (500, b"No listener registered".to_vec());
    }

    unsafe {
        let global_obj = JS_GLOBAL.with(|g| g.borrow().as_ref().map(|h| h.get()));
        let global_obj = match global_obj {
            Some(obj) if !obj.is_null() => obj,
            _ => return (500, b"No global object".to_vec()),
        };

        // Enter the realm of the global object and create a scope for rooting.
        let scope = RootScope::new_with_realm(
            cx,
            std::ptr::NonNull::new_unchecked(global_obj),
        );

        // Create a JSRequest with the incoming request data.
        let js_req = JSRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: headers.to_vec(),
            body: body.to_vec(),
            response_state: ResponseState::Pending,
        };
        let req_obj = match create_instance::<JSRequest>(&scope, js_req) {
            Ok(o) => o,
            Err(_) => return (500, b"Failed to create Request object".to_vec()),
        };

        let req_val = scope.root_value(value::from_object(req_obj.as_raw()));

        // Get the callback.
        let callback_obj =
            LISTENER_CALLBACK.with(|cb| cb.borrow().as_ref().map(|h| h.get()));
        let callback_obj = match callback_obj {
            Some(obj) if !obj.is_null() => obj,
            _ => return (500, b"Callback was collected".to_vec()),
        };

        let callback_val = scope.root_value(value::from_object(callback_obj));
        let global = scope.root_object(std::ptr::NonNull::new_unchecked(global_obj));

        // Call: callback(request)
        let args = HandleValueArray {
            length_: 1,
            elements_: &*req_val as *const Value,
        };

        let result =
            js::Function::call_value(&scope, global, callback_val, &args);

        if result.is_err() {
            if js::exception::is_pending(&scope) {
                js::exception::clear(&scope);
                eprintln!("JS listener threw an exception");
            }
            return (500, b"JS listener threw an exception".to_vec());
        }

        // Extract the response from the Request's private data.
        match get_private_mut::<JSRequest>(req_obj.as_raw()) {
            Some(req) => {
                match std::mem::replace(&mut req.response_state, ResponseState::Pending) {
                    ResponseState::Immediate { status, body } => (status, body),
                    ResponseState::Done { status, body } => (status, body),
                    ResponseState::Streaming { status, chunks } => {
                        let body: Vec<u8> = chunks.into_iter().flatten().collect();
                        (status, body)
                    }
                    ResponseState::Pending => {
                        (500, b"No response was set by the listener".to_vec())
                    }
                }
            }
            None => (500, b"Failed to read Request private data".to_vec()),
        }
    }
}

fn method_to_string(method: &Method) -> String {
    match method {
        Method::Get => "GET".to_string(),
        Method::Post => "POST".to_string(),
        Method::Put => "PUT".to_string(),
        Method::Delete => "DELETE".to_string(),
        Method::Head => "HEAD".to_string(),
        Method::Options => "OPTIONS".to_string(),
        Method::Patch => "PATCH".to_string(),
        Method::Connect => "CONNECT".to_string(),
        Method::Trace => "TRACE".to_string(),
        Method::Other(s) => s.clone(),
    }
}
