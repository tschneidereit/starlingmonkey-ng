// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://html.spec.whatwg.org/multipage/workers.html#worker-locations>
//!
//! `WorkerLocation` exposes the runtime's current location URL via
//! `globalThis.location`. The HTML spec sources this URL from the surrounding
//! `WorkerGlobalScope`. Starling has no `WorkerGlobalScope`, so a single
//! process-wide URL — typically read from an incoming HTTP request or configured
//! via `--init-location` — backs every `WorkerLocation` accessor instead.
//! If no URL has been configured, every accessor throws a `TypeError`.

use std::cell::RefCell;

use core_runtime::jsmethods;
use core_runtime::webidl_interface;
use js::error::TypeError;
use js::gc::scope::Scope;
use js::Object;

thread_local! {
    /// The URL backing `globalThis.location`.
    static LOCATION_URL: RefCell<Option<url::Url>> = const { RefCell::new(None) };
}

/// Configure the URL exposed via `globalThis.location`.
pub fn set_init_location(url: url::Url) {
    LOCATION_URL.with(|cell| *cell.borrow_mut() = Some(url));
}

/// Clear the configured location URL.
///
/// Subsequent accessor calls will throw until [`set_init_location`] is called
/// again. Provided primarily for tests.
pub fn clear_init_location() {
    LOCATION_URL.with(|cell| cell.borrow_mut().take());
}

/// Run `f` against the configured URL, or yield a `TypeError` if no URL is set.
///
/// `field` is the accessor name; it appears in the error message so JS callers
/// can identify which property triggered the error.
fn with_url<T>(field: &str, f: impl FnOnce(&url::Url) -> T) -> Result<T, TypeError> {
    LOCATION_URL.with(|cell| {
        cell.borrow().as_ref().map(f).ok_or_else(|| {
            TypeError(format!(
                "location.{field} can only be used during request handling, \
                 or if an initialization-time location was set using `--init-location`"
            ))
        })
    })
}

/// The `WorkerLocation` interface.
///
/// `Symbol.toStringTag` is `"Location"` rather than the WebIDL default of
/// `"WorkerLocation"`, matching the C++ runtime so JS code observes
/// `Object.prototype.toString.call(location) === "[object Location]"`.
#[webidl_interface(to_string_tag = "Location")]
pub struct WorkerLocation {}

#[jsmethods]
impl WorkerLocation {
    /// <https://html.spec.whatwg.org/multipage/workers.html#workerlocation>
    ///
    /// `WorkerLocation` has no IDL `[Constructor]`; JS code is not permitted
    /// to construct one.
    #[constructor]
    fn new(&self, _scope: &Scope<'_>) -> Result<(), TypeError> {
        Err(TypeError("Illegal constructor".into()))
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-href>
    #[getter]
    fn href(&self) -> Result<String, TypeError> {
        // Step 1: Return this's WorkerGlobalScope object's url, serialized.
        with_url("href", |url| url.as_str().to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-origin>
    #[getter]
    fn origin(&self) -> Result<String, TypeError> {
        // Step 1: Return the serialization of this's WorkerGlobalScope object's url's origin.
        with_url("origin", |url| url.origin().ascii_serialization())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-protocol>
    #[getter]
    fn protocol(&self) -> Result<String, TypeError> {
        // Step 1: Return this's WorkerGlobalScope object's url's scheme, followed by ":".
        with_url("protocol", |url| url::quirks::protocol(url).to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-host>
    #[getter]
    fn host(&self) -> Result<String, TypeError> {
        // Step 1: Let _url_ be `this`'s ``WorkerGlobalScope` object`'s `url`.
        // Step 2: If _url_'s `host` is null, return the empty string.
        // Step 3: If _url_'s `port` is null, return _url_'s `host`, `serialized`.
        // Step 4: Return _url_'s `host`, `serialized`, followed by "`:`" and _url_'s `port`,
        //         `serialized`.
        // `url::quirks::host` performs all four steps in one call.
        with_url("host", |url| url::quirks::host(url).to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-hostname>
    #[getter]
    fn hostname(&self) -> Result<String, TypeError> {
        // Step 1: Let _host_ be `this`'s ``WorkerGlobalScope` object`'s `url`'s `host`.
        // Step 2: If _host_ is null, return the empty string.
        // Step 3: Return _host_, `serialized`.
        with_url("hostname", |url| url::quirks::hostname(url).to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-port>
    #[getter]
    fn port(&self) -> Result<String, TypeError> {
        // Step 1: Let _port_ be `this`'s ``WorkerGlobalScope` object`'s `url`'s `port`.
        // Step 2: If _port_ is null, return the empty string.
        // Step 3: Return _port_, `serialized`.
        with_url("port", |url| url::quirks::port(url).to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-pathname>
    #[getter]
    fn pathname(&self) -> Result<String, TypeError> {
        // Step 1: Return the result of URL path serializing this's WorkerGlobalScope object's url.
        with_url("pathname", |url| url.path().to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-search>
    #[getter]
    fn search(&self) -> Result<String, TypeError> {
        // Step 1: Let _query_ be `this`'s ``WorkerGlobalScope` object`'s `url`'s `query`.
        // Step 2: If _query_ is either null or the empty string, return the empty string.
        // Step 3: Return "`?`", followed by _query_.
        with_url("search", |url| url::quirks::search(url).to_string())
    }

    /// <https://html.spec.whatwg.org/multipage/workers.html#dom-workerlocation-hash>
    #[getter]
    fn hash(&self) -> Result<String, TypeError> {
        // Step 1: Let _fragment_ be `this`'s ``WorkerGlobalScope` object`'s `url`'s `fragment`.
        // Step 2: If _fragment_ is either null or the empty string, return the empty string.
        // Step 3: Return "`#`", followed by _fragment_.
        with_url("hash", |url| url::quirks::hash(url).to_string())
    }

    /// IDL `stringifier;` — returns the same string as the `href` getter.
    #[allow(clippy::wrong_self_convention)]
    #[method(name = "toString")]
    fn to_string(&self) -> Result<String, TypeError> {
        self.href()
    }
}

/// Register the `WorkerLocation` class on `global` and install the singleton
/// `location` instance as a property on it.
///
/// The URL backing the singleton's accessors is the one set via
/// [`set_init_location`]; if none has been set when an accessor runs, it throws.
pub fn add_to_global<'s>(scope: &'s Scope<'_>, global: Object<'s>) {
    WorkerLocation::add_to_global(scope, global);

    // SAFETY: `WorkerLocation::add_to_global` registered the class on the
    // current global immediately above.
    let location = unsafe {
        js::class::create_instance_with::<WorkerLocationImpl>(scope, |_| WorkerLocationImpl {})
    }
    .expect("failed to allocate WorkerLocation singleton");

    global
        .set_property(scope, c"location", location)
        .expect("failed to define globalThis.location");
}

#[cfg(test)]
mod tests {
    use core_runtime::{runtime, test_util::eval_with_setup};

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                super::clear_init_location();
                runtime::register_global_initializer(super::add_to_global);
            },
            code,
        )
    }

    fn eval_with_url(location_url: &str, code: &str) -> String {
        eval_with_setup(
            || {
                super::set_init_location(url::Url::parse(location_url).unwrap());
                runtime::register_global_initializer(super::add_to_global);
            },
            code,
        )
    }

    #[test]
    fn location_exists_on_global() {
        assert_eq!(
            eval_with_url("https://example.com/", "typeof location"),
            "object"
        );
    }

    #[test]
    fn constructor_throws() {
        // WorkerLocation has no IDL constructor; `new WorkerLocation()` must throw.
        assert_eq!(
            eval(
                "try { new WorkerLocation(); 'no-throw' } \
                 catch(e) { (e instanceof TypeError) + ',' + e.message }"
            ),
            "true,Illegal constructor"
        );
    }

    #[test]
    fn to_string_tag_is_location() {
        assert_eq!(
            eval_with_url(
                "https://example.com/",
                "Object.prototype.toString.call(location)"
            ),
            "[object Location]"
        );
    }

    #[test]
    fn href_returns_serialized_url() {
        assert_eq!(
            eval_with_url("https://example.com/path?q=1#frag", "location.href"),
            "https://example.com/path?q=1#frag"
        );
    }

    #[test]
    fn origin_returns_origin() {
        assert_eq!(
            eval_with_url("https://example.com:8443/path", "location.origin"),
            "https://example.com:8443"
        );
    }

    #[test]
    fn protocol_returns_scheme_with_colon() {
        assert_eq!(
            eval_with_url("https://example.com/", "location.protocol"),
            "https:"
        );
    }

    #[test]
    fn host_and_hostname() {
        assert_eq!(
            eval_with_url("https://example.com:8443/", "location.host"),
            "example.com:8443"
        );
        assert_eq!(
            eval_with_url("https://example.com:8443/", "location.hostname"),
            "example.com"
        );
    }

    #[test]
    fn port_includes_explicit_only() {
        assert_eq!(
            eval_with_url("https://example.com:8443/", "location.port"),
            "8443"
        );
        // Default ports are omitted by the URL parser.
        assert_eq!(eval_with_url("https://example.com/", "location.port"), "");
    }

    #[test]
    fn pathname_search_hash() {
        assert_eq!(
            eval_with_url("https://example.com/foo/bar?x=1#frag", "location.pathname"),
            "/foo/bar"
        );
        assert_eq!(
            eval_with_url("https://example.com/foo/bar?x=1#frag", "location.search"),
            "?x=1"
        );
        assert_eq!(
            eval_with_url("https://example.com/foo/bar?x=1#frag", "location.hash"),
            "#frag"
        );
    }

    #[test]
    fn empty_search_and_hash_return_empty_string() {
        assert_eq!(eval_with_url("https://example.com/", "location.search"), "");
        assert_eq!(eval_with_url("https://example.com/", "location.hash"), "");
    }

    #[test]
    fn to_string_returns_href() {
        assert_eq!(
            eval_with_url(
                "https://example.com/path?q=1",
                "location.toString() === location.href"
            ),
            "true"
        );
    }

    #[test]
    fn unset_location_throws_typeerror() {
        // With no init-location configured, every accessor throws.
        assert_eq!(
            eval(
                "try { location.href; 'no-throw' } \
                 catch(e) { (e instanceof TypeError) + ',' + e.message.includes('--init-location') }"
            ),
            "true,true"
        );
        assert_eq!(
            eval(
                "try { location.toString(); 'no-throw' } \
                 catch(e) { e instanceof TypeError }"
            ),
            "true"
        );
    }
}
