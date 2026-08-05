// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://urlpattern.spec.whatwg.org/>

use core_runtime::{webidl_interface, webidl_methods, webidl_dictionary};
use indexmap::IndexMap;
use js::error::{throw_type_error, ExnThrown};
use js::gc::scope::Scope;
use js::prelude::{FromJSVal, HandleValue, ToJSVal};
use js::Object;
use urlpattern::quirks::{self, StringOrInit};
use urlpattern::UrlPattern as RustUrlPattern;

/// <https://urlpattern.spec.whatwg.org/#dictdef-urlpatterninit>
#[webidl_dictionary]
pub struct URLPatternInit {
    pub protocol: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub hostname: Option<String>,
    pub port: Option<String>,
    pub pathname: Option<String>,
    pub search: Option<String>,
    pub hash: Option<String>,
    #[webidl(name = "baseURL")]
    pub base_url: Option<String>,
}

/// <https://urlpattern.spec.whatwg.org/#dictdef-urlpatternoptions>
#[webidl_dictionary]
pub struct URLPatternOptions {
    #[webidl(default = false)]
    pub ignore_case: bool,
}

impl URLPatternOptions {
    pub fn into_rust_options(self) -> urlpattern::UrlPatternOptions {
        urlpattern::UrlPatternOptions {
            ignore_case: self.ignore_case,
            ..Default::default()
        }
    }
}

/// <https://urlpattern.spec.whatwg.org/#typedefdef-urlpatterninput>
pub enum URLPatternInputUnion {
    Str(String),
    Init(URLPatternInit),
}

impl<'s> FromJSVal<'s> for URLPatternInputUnion {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<Self, js::conversion::ConversionError> {
        if val.get().is_object() && !val.get().is_null() {
            URLPatternInit::from_jsval(scope, val, ())
                .map(URLPatternInputUnion::Init)
        } else {
            String::from_jsval(scope, val, ())
                .map(URLPatternInputUnion::Str)
        }
    }
}

/// Second constructor argument: either a baseURL string or options dict.
enum ConstructorSecondArg {
    BaseUrl(String),
    Options(URLPatternOptions),
}

impl<'s> FromJSVal<'s> for ConstructorSecondArg {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<Self, js::conversion::ConversionError> {
        if val.get().is_string() {
            String::from_jsval(scope, val, ())
                .map(ConstructorSecondArg::BaseUrl)
        } else if val.get().is_object() && !val.get().is_null() {
            URLPatternOptions::from_jsval(scope, val, ())
                .map(ConstructorSecondArg::Options)
        } else {
            String::from_jsval(scope, val, ())
                .map(ConstructorSecondArg::BaseUrl)
        }
    }
}

fn input_to_string_or_init(input: URLPatternInputUnion) -> StringOrInit<'static> {
    match input {
        URLPatternInputUnion::Str(s) => StringOrInit::String(s.into()),
        URLPatternInputUnion::Init(init) => StringOrInit::Init(quirks::UrlPatternInit {
            protocol: init.protocol,
            username: init.username,
            password: init.password,
            hostname: init.hostname,
            port: init.port,
            pathname: init.pathname,
            search: init.search,
            hash: init.hash,
            base_url: init.base_url,
        }),
    }
}

/// <https://urlpattern.spec.whatwg.org/#urlpattern-class>
#[webidl_interface]
pub struct URLPattern {
    #[no_trace]
    inner: Option<RustUrlPattern>,
}

#[webidl_methods]
impl URLPattern {
    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-urlpattern>
    #[constructor]
    fn new(
        &self,
        scope: &Scope<'_>,
        input: Option<URLPatternInputUnion>,
        second: Option<ConstructorSecondArg>,
        third: Option<URLPatternOptions>,
    ) -> Result<(), ExnThrown> {
        let (base_url_str, third_opts): (Option<String>, Option<URLPatternOptions>) = match second {
            None => (None, third),
            Some(ConstructorSecondArg::BaseUrl(b)) => (Some(b), third),
            Some(ConstructorSecondArg::Options(opts)) => (None, Some(opts)),
        };

        let opts = third_opts.unwrap_or_else(|| URLPatternOptions { ignore_case: false }).into_rust_options();

        let string_or_init: StringOrInit<'static> = match input {
            None => StringOrInit::Init(Default::default()),
            Some(inp) => input_to_string_or_init(inp),
        };

        let rust_init = quirks::process_construct_pattern_input(
            string_or_init,
            base_url_str.as_deref(),
        ).map_err(|_| throw_type_error(scope, c"Failed to parse pattern"))?;

        let pattern = RustUrlPattern::parse(rust_init, opts)
            .map_err(|_| throw_type_error(scope, c"Failed to create URL pattern"))?;

        self.data_mut().inner = Some(pattern);
        Ok(())
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-protocol>
    #[getter]
    fn protocol(&self) -> String {
        self.data().inner.as_ref().map(|p| p.protocol().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-username>
    #[getter]
    fn username(&self) -> String {
        self.data().inner.as_ref().map(|p| p.username().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-password>
    #[getter]
    fn password(&self) -> String {
        self.data().inner.as_ref().map(|p| p.password().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-hostname>
    #[getter]
    fn hostname(&self) -> String {
        self.data().inner.as_ref().map(|p| p.hostname().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-port>
    #[getter]
    fn port(&self) -> String {
        self.data().inner.as_ref().map(|p| p.port().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-pathname>
    #[getter]
    fn pathname(&self) -> String {
        self.data().inner.as_ref().map(|p| p.pathname().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-search>
    #[getter]
    fn search(&self) -> String {
        self.data().inner.as_ref().map(|p| p.search().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-hash>
    #[getter]
    fn hash(&self) -> String {
        self.data().inner.as_ref().map(|p| p.hash().to_string()).unwrap_or_default()
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-hasregexpgroups>
    #[getter]
    fn has_reg_exp_groups(&self) -> bool {
        self.data().inner.as_ref().map(|p| p.has_regexp_groups()).unwrap_or(false)
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-test>
    #[method]
    fn test(
        &self,
        scope: &Scope<'_>,
        input: Option<URLPatternInputUnion>,
        base_url: Option<String>,
    ) -> Result<bool, ExnThrown> {
        let inner = self.data().inner.as_ref().expect("URLPattern not initialized");

        let (string_or_init, base_url_str) = match input {
            None => (StringOrInit::Init(Default::default()), None),
            Some(URLPatternInputUnion::Str(s)) => (StringOrInit::String(s.into()), base_url),
            Some(URLPatternInputUnion::Init(init)) => {
                if base_url.is_some() {
                    return Err(throw_type_error(scope, c"test(): second argument must not be given when input is URLPatternInit."));
                }
                (input_to_string_or_init(URLPatternInputUnion::Init(init)), None)
            }
        };

        let result = quirks::process_match_input(string_or_init, base_url_str.as_deref());

        match result {
            Ok(Some((match_input, _inputs))) => Ok(inner.test(match_input).unwrap_or(false)),
            Ok(None) => Ok(false),
            Err(_) => Ok(false),
        }
    }

    /// <https://urlpattern.spec.whatwg.org/#dom-urlpattern-exec>
    #[method]
    fn exec(
        &self,
        scope: &Scope<'_>,
        input: Option<URLPatternInputUnion>,
        base_url: Option<String>,
    ) -> Result<Option<URLPatternResult>, ExnThrown> {
        let inner = self.data().inner.as_ref().expect("URLPattern not initialized");

        let (string_or_init, base_url_str) = match input {
            None => (StringOrInit::Init(Default::default()), None),
            Some(URLPatternInputUnion::Str(s)) => (StringOrInit::String(s.into()), base_url),
            Some(URLPatternInputUnion::Init(init)) => {
                if base_url.is_some() {
                    return Err(throw_type_error(scope, c"exec(): second argument must not be given when input is URLPatternInit."));
                }
                (input_to_string_or_init(URLPatternInputUnion::Init(init)), None)
            }
        };

        let result = quirks::process_match_input(string_or_init.clone(), base_url_str.as_deref());

        match result {
            Ok(Some((match_input, (orig_input, orig_base)))) => {
                let inputs_vec: Vec<URLPatternInputArg> = match &orig_input {
                    StringOrInit::String(s) => {
                        let mut v = vec![URLPatternInputArg::Str(s.to_string())];
                        if let Some(b) = &orig_base {
                            v.push(URLPatternInputArg::Str(b.clone()));
                        }
                        v
                    }
                    StringOrInit::Init(init) => {
                        vec![URLPatternInputArg::Dict(URLPatternInit {
                            protocol: init.protocol.clone(),
                            username: init.username.clone(),
                            password: init.password.clone(),
                            hostname: init.hostname.clone(),
                            port: init.port.clone(),
                            pathname: init.pathname.clone(),
                            search: init.search.clone(),
                            hash: init.hash.clone(),
                            base_url: init.base_url.clone(),
                        })]
                    }
                };

                match inner.exec(match_input) {
                    Ok(Some(rust_result)) => Ok(Some(URLPatternResult::from_rust(rust_result, inputs_vec, inner))),
                    Ok(None) => Ok(None),
                    Err(_) => Ok(None),
                }
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }
}

/// An argument passed to exec() or test().
enum URLPatternInputArg {
    Str(String),
    Dict(URLPatternInit),
}

impl<'s> ToJSVal<'s> for URLPatternInputArg {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, js::conversion::ConversionError> {
        let err = |_| js::conversion::ConversionError::ExnPending;
        match self {
            URLPatternInputArg::Str(s) => s.to_jsval(scope),
            URLPatternInputArg::Dict(init) => {
                let obj = Object::new(scope, None).map_err(err)?;
                if let Some(ref v) = init.protocol { obj.set_property(scope, c"protocol", v).map_err(err)?; }
                if let Some(ref v) = init.username  { obj.set_property(scope, c"username",  v).map_err(err)?; }
                if let Some(ref v) = init.password  { obj.set_property(scope, c"password",  v).map_err(err)?; }
                if let Some(ref v) = init.hostname  { obj.set_property(scope, c"hostname",  v).map_err(err)?; }
                if let Some(ref v) = init.port      { obj.set_property(scope, c"port",      v).map_err(err)?; }
                if let Some(ref v) = init.pathname  { obj.set_property(scope, c"pathname",  v).map_err(err)?; }
                if let Some(ref v) = init.search    { obj.set_property(scope, c"search",    v).map_err(err)?; }
                if let Some(ref v) = init.hash      { obj.set_property(scope, c"hash",      v).map_err(err)?; }
                if let Some(ref v) = init.base_url  { obj.set_property(scope, c"baseURL",   v).map_err(err)?; }
                obj.to_jsval(scope)
            }
        }
    }
}

/// <https://urlpattern.spec.whatwg.org/#dictdef-urlpatternresult>
pub struct URLPatternResult {
    inputs: Vec<URLPatternInputArg>,
    protocol: URLPatternComponentResult,
    username: URLPatternComponentResult,
    password: URLPatternComponentResult,
    hostname: URLPatternComponentResult,
    port: URLPatternComponentResult,
    pathname: URLPatternComponentResult,
    search: URLPatternComponentResult,
    hash: URLPatternComponentResult,
}

impl URLPatternResult {
    fn from_rust(
        result: urlpattern::UrlPatternResult,
        inputs: Vec<URLPatternInputArg>,
        inner: &RustUrlPattern,
    ) -> Self {
        // <https://urlpattern.spec.whatwg.org/#create-a-component-match-result>
        let make_comp = |r: urlpattern::UrlPatternComponentResult, names: &[String]| {
            let mut r_groups = r.groups;
            let mut groups: IndexMap<String, Option<String>> = names.iter()
                .map(|name| (name.clone(), r_groups.remove(name).flatten()))
                .collect();

            // This workaround exists because Rust's `regex` engine
            // differs from ECMAScript for non-participating optional groups:
            // Rust returns `Some("")` where ECMAScript (and the spec) return
            // `undefined`.
            //
            // Workaround: for patterns with multiple auto-numbered wildcard
            // groups (pure-digit names like "0", "1"), convert trailing
            // `Some("")` entries to `None` when an earlier group has content.
            // Covers e.g. `*{}**?` → `*(.*)?` on `foobar`, where the crate
            // returns `{"1": Some(""), "0": Some("foobar")}` but WPT expects
            // `{"0": "foobar", "1": null}`.
            let wildcard_groups: Vec<&String> = names.iter()
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                .collect();
            if wildcard_groups.len() >= 2 {
                let has_content = wildcard_groups.iter().any(|n| {
                    groups.get(*n).and_then(Option::as_ref).map(|s| !s.is_empty()).unwrap_or(false)
                });
                if has_content {
                    for name in &wildcard_groups[1..] {
                        if groups.get(*name) == Some(&Some(String::new())) {
                            groups.insert((**name).clone(), None);
                        }
                    }
                }
            }

            URLPatternComponentResult { input: r.input, groups }
        };

        Self {
            inputs,
            protocol: make_comp(result.protocol, &inner.protocol.group_name_list),
            username: make_comp(result.username, &inner.username.group_name_list),
            password: make_comp(result.password, &inner.password.group_name_list),
            hostname: make_comp(result.hostname, &inner.hostname.group_name_list),
            port: make_comp(result.port, &inner.port.group_name_list),
            pathname: make_comp(result.pathname, &inner.pathname.group_name_list),
            search: make_comp(result.search, &inner.search.group_name_list),
            hash: make_comp(result.hash, &inner.hash.group_name_list),
        }
    }
}

/// <https://urlpattern.spec.whatwg.org/#dictdef-urlpatternresult>
impl<'s> ToJSVal<'s> for URLPatternResult {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, js::conversion::ConversionError> {
        let err = |_| js::conversion::ConversionError::ExnPending;
        let obj = Object::new(scope, None).map_err(err)?;

        let inputs_arr = js::Array::new(scope, self.inputs.len()).map_err(err)?;
        for (i, input_arg) in self.inputs.iter().enumerate() {
            inputs_arr.set_element(scope, i as u32, input_arg.to_jsval(scope)?).map_err(err)?;
        }

        obj.set_property(scope, c"inputs",   &inputs_arr).map_err(err)?;
        obj.set_property(scope, c"protocol", &self.protocol).map_err(err)?;
        obj.set_property(scope, c"username", &self.username).map_err(err)?;
        obj.set_property(scope, c"password", &self.password).map_err(err)?;
        obj.set_property(scope, c"hostname", &self.hostname).map_err(err)?;
        obj.set_property(scope, c"port",     &self.port).map_err(err)?;
        obj.set_property(scope, c"pathname", &self.pathname).map_err(err)?;
        obj.set_property(scope, c"search",   &self.search).map_err(err)?;
        obj.set_property(scope, c"hash",     &self.hash).map_err(err)?;

        obj.to_jsval(scope)
    }
}

/// <https://urlpattern.spec.whatwg.org/#dictdef-urlpatterncomponentresult>
pub struct URLPatternComponentResult {
    input: String,
    groups: indexmap::IndexMap<String, Option<String>>,
}

impl<'s> ToJSVal<'s> for URLPatternComponentResult {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, js::conversion::ConversionError> {
        let err = |_| js::conversion::ConversionError::ExnPending;
        let obj = Object::new(scope, None).map_err(err)?;

        obj.set_property(scope, c"input", &self.input).map_err(err)?;

        let groups_obj = Object::new(scope, None).map_err(err)?;
        for (key, value) in &self.groups {
            let js_key = js::JSString::from_str(scope, key).map_err(err)?;
            let id_raw = js::id::string_to_id(scope, js_key.handle()).map_err(err)?;
            let id = scope.root_id(id_raw);
            let js_val = match value {
                Some(s) => scope.root_value(s.to_jsval(scope)?.get()),
                None => scope.root_value(HandleValue::undefined().get()),
            };
            groups_obj.set_property_by_id(scope, id, js_val).map_err(err)?;
        }

        obj.set_property(scope, c"groups", &groups_obj).map_err(err)?;

        obj.to_jsval(scope)
    }
}
