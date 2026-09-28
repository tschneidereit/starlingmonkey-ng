// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Standalone algorithms from <https://url.spec.whatwg.org/>

use url::Url;

/// The basic URL parser takes a scalar value string input, with an optional null or base URL base
/// (default null), an optional encoding encoding (default UTF-8), an optional URL url, and an
/// optional state override state override.
pub(crate) fn basic_url_parser(input: &str, base: Option<&Url>) -> Option<Url> {
    match base {
        Some(base_url) => base_url.join(input).ok(),
        None => Url::parse(input).ok(),
    }
}

/// The API URL parser takes a scalar value string url and an optional null-or-scalar value string
/// base (default null).
pub(crate) fn api_url_parser(url: &str, base: Option<&str>) -> Option<Url> {
    let parsed_base = match base {
        Some(base) => Some(basic_url_parser(base, None)?),
        None => None,
    };
    basic_url_parser(url, parsed_base.as_ref())
}

/// <https://url.spec.whatwg.org/#concept-urlencoded-parser>
pub(crate) fn urlencoded_parse(input: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(input.as_bytes())
        .into_owned()
        .collect()
}

/// <https://url.spec.whatwg.org/#concept-urlencoded-serializer>
pub(crate) fn urlencoded_serialize(tuples: &[(String, String)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in tuples {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}
