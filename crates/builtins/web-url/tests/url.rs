// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::test_util::{eval_with_setup, throws_with_setup};

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        web_url::add_to_global(scope, global);
    });
}

fn eval(code: &str) -> String {
    eval_with_setup(setup, code)
}

fn throws(code: &str) -> bool {
    throws_with_setup(setup, code)
}

// ── URL constructor ──

#[test]
fn url_constructor_basic() {
    assert_eq!(
        eval("new URL('https://example.com/').href"),
        "https://example.com/"
    );
}

#[test]
fn url_constructor_with_base() {
    assert_eq!(
        eval("new URL('/path', 'https://example.com').href"),
        "https://example.com/path"
    );
}

#[test]
fn url_constructor_invalid_throws() {
    assert!(throws("new URL('not a url')"));
}

#[test]
fn url_constructor_invalid_base_throws() {
    assert!(throws("new URL('/path', 'not a url')"));
}

// ── URL properties ──

#[test]
fn url_origin() {
    assert_eq!(
        eval("new URL('https://example.com:8080/p').origin"),
        "https://example.com:8080"
    );
}

#[test]
fn url_protocol() {
    assert_eq!(eval("new URL('https://example.com/').protocol"), "https:");
}

#[test]
fn url_username() {
    assert_eq!(
        eval("new URL('https://user@example.com/').username"),
        "user"
    );
}

#[test]
fn url_password() {
    assert_eq!(
        eval("new URL('https://user:pass@example.com/').password"),
        "pass"
    );
}

#[test]
fn url_host_with_port() {
    assert_eq!(
        eval("new URL('https://example.com:8080/').host"),
        "example.com:8080"
    );
}

#[test]
fn url_host_default_port() {
    assert_eq!(eval("new URL('https://example.com/').host"), "example.com");
}

#[test]
fn url_hostname() {
    assert_eq!(
        eval("new URL('https://example.com:8080/').hostname"),
        "example.com"
    );
}

#[test]
fn url_port() {
    assert_eq!(eval("new URL('https://example.com:8080/').port"), "8080");
}

#[test]
fn url_port_default_empty() {
    assert_eq!(eval("new URL('https://example.com/').port"), "");
}

#[test]
fn url_pathname() {
    assert_eq!(eval("new URL('https://example.com/a/b').pathname"), "/a/b");
}

#[test]
fn url_search() {
    assert_eq!(eval("new URL('https://example.com/?q=1').search"), "?q=1");
}

#[test]
fn url_search_empty() {
    assert_eq!(eval("new URL('https://example.com/').search"), "");
}

#[test]
fn url_hash() {
    assert_eq!(eval("new URL('https://example.com/#frag').hash"), "#frag");
}

#[test]
fn url_hash_empty() {
    assert_eq!(eval("new URL('https://example.com/').hash"), "");
}

// ── URL setters ──

#[test]
fn url_set_href() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.href = 'https://b.com/path'; u.href"),
        "https://b.com/path"
    );
}

#[test]
fn url_set_href_invalid_throws() {
    assert!(throws(
        "let u = new URL('https://a.com/'); u.href = 'invalid'"
    ));
}

#[test]
fn url_set_protocol() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.protocol = 'http:'; u.protocol"),
        "http:"
    );
}

#[test]
fn url_set_username() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.username = 'bob'; u.username"),
        "bob"
    );
}

#[test]
fn url_set_password() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.password = 'secret'; u.password"),
        "secret"
    );
}

#[test]
fn url_set_host() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.host = 'b.com:9090'; u.host"),
        "b.com:9090"
    );
}

#[test]
fn url_set_hostname() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.hostname = 'b.com'; u.hostname"),
        "b.com"
    );
}

#[test]
fn url_set_port() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.port = '3000'; u.port"),
        "3000"
    );
}

#[test]
fn url_set_pathname() {
    assert_eq!(
        eval("let u = new URL('https://a.com/old'); u.pathname = '/new'; u.pathname"),
        "/new"
    );
}

#[test]
fn url_set_search() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.search = '?x=1'; u.search"),
        "?x=1"
    );
}

#[test]
fn url_set_hash() {
    assert_eq!(
        eval("let u = new URL('https://a.com/'); u.hash = '#sec'; u.hash"),
        "#sec"
    );
}

// ── URL methods ──

#[test]
fn url_to_string() {
    assert_eq!(
        eval("new URL('https://example.com/path?q=1#h').toString()"),
        "https://example.com/path?q=1#h"
    );
}

#[test]
fn url_to_json() {
    assert_eq!(
        eval("new URL('https://example.com/').toJSON()"),
        "https://example.com/"
    );
}

// ── URL.parse ──

#[test]
fn url_parse_valid() {
    assert_eq!(
        eval("URL.parse('https://example.com/').href"),
        "https://example.com/"
    );
}

#[test]
fn url_parse_invalid_returns_null() {
    assert_eq!(eval("URL.parse('not a url')"), "null");
}

#[test]
fn url_parse_with_base() {
    assert_eq!(
        eval("URL.parse('/path', 'https://example.com').href"),
        "https://example.com/path"
    );
}

// ── URL.canParse ──

#[test]
fn url_can_parse_valid() {
    assert_eq!(eval("URL.canParse('https://example.com/')"), "true");
}

#[test]
fn url_can_parse_invalid() {
    assert_eq!(eval("URL.canParse('not a url')"), "false");
}

// ── URLSearchParams constructor ──

#[test]
fn usp_construct_empty() {
    assert_eq!(eval("new URLSearchParams().toString()"), "");
}

#[test]
fn usp_construct_from_string() {
    assert_eq!(eval("new URLSearchParams('a=1&b=2').toString()"), "a=1&b=2");
}

#[test]
fn usp_construct_from_string_strips_question_mark() {
    assert_eq!(eval("new URLSearchParams('?a=1').toString()"), "a=1");
}

#[test]
fn usp_construct_from_sequence() {
    assert_eq!(
        eval("new URLSearchParams([['a', '1'], ['b', '2']]).toString()"),
        "a=1&b=2"
    );
}

#[test]
fn usp_construct_from_record() {
    // Order may vary for records; check both entries exist.
    assert_eq!(
        eval("new URLSearchParams({a: '1', b: '2'}).toString()"),
        "a=1&b=2"
    );
}

#[test]
fn usp_construct_invalid_pair_throws() {
    assert!(throws("new URLSearchParams([['only_one']])"));
}

#[test]
fn usp_construct_from_custom_iterable() {
    assert_eq!(
        eval(
            "let init = { [Symbol.iterator]: function* () { yield ['a', 'b']; yield ['c', 'd']; } };\
             new URLSearchParams(init).get('a')"
        ),
        "b"
    );
}

#[test]
fn usp_construct_record_preserves_null_code_point() {
    assert_eq!(
        eval("new URLSearchParams({['a\\0b']: 42}).toString()"),
        "a%00b=42"
    );
}

// ── URLSearchParams methods ──

#[test]
fn usp_get() {
    assert_eq!(eval("new URLSearchParams('a=1&b=2').get('a')"), "1");
}

#[test]
fn usp_get_missing_returns_null() {
    assert_eq!(eval("new URLSearchParams('a=1').get('x')"), "null");
}

#[test]
fn usp_get_all() {
    assert_eq!(
        eval("JSON.stringify(new URLSearchParams('a=1&a=2&b=3').getAll('a'))"),
        "[\"1\",\"2\"]"
    );
}

#[test]
fn usp_has() {
    assert_eq!(eval("new URLSearchParams('a=1').has('a')"), "true");
}

#[test]
fn usp_has_missing() {
    assert_eq!(eval("new URLSearchParams('a=1').has('b')"), "false");
}

#[test]
fn usp_has_with_value() {
    assert_eq!(eval("new URLSearchParams('a=1&a=2').has('a', '2')"), "true");
}

#[test]
fn usp_append() {
    assert_eq!(
        eval("let p = new URLSearchParams('a=1'); p.append('b', '2'); p.toString()"),
        "a=1&b=2"
    );
}

#[test]
fn usp_delete() {
    assert_eq!(
        eval("let p = new URLSearchParams('a=1&b=2&a=3'); p.delete('a'); p.toString()"),
        "b=2"
    );
}

#[test]
fn usp_delete_with_value() {
    assert_eq!(
        eval("let p = new URLSearchParams('a=1&a=2&a=3'); p.delete('a', '2'); p.toString()"),
        "a=1&a=3"
    );
}

#[test]
fn usp_set() {
    assert_eq!(
        eval("let p = new URLSearchParams('a=1&a=2'); p.set('a', '3'); p.toString()"),
        "a=3"
    );
}

#[test]
fn usp_set_new_key() {
    assert_eq!(
        eval("let p = new URLSearchParams(); p.set('x', '1'); p.toString()"),
        "x=1"
    );
}

#[test]
fn usp_sort() {
    assert_eq!(
        eval("let p = new URLSearchParams('c=3&a=1&b=2'); p.sort(); p.toString()"),
        "a=1&b=2&c=3"
    );
}

#[test]
fn usp_size() {
    assert_eq!(eval("new URLSearchParams('a=1&b=2&c=3').size"), "3");
}

#[test]
fn usp_entries_iterator_method() {
    assert_eq!(
        eval(
            "let p = new URLSearchParams('a=1&b=2'); \
             let it = p.entries(); \
             let first = it.next().value; \
             first[0] + '=' + first[1]"
        ),
        "a=1"
    );
}

#[test]
fn usp_keys_and_values_methods() {
    assert_eq!(
        eval(
            "let p = new URLSearchParams('a=1&b=2'); \
             JSON.stringify([Array.from(p.keys()), Array.from(p.values())])"
        ),
        "[[\"a\",\"b\"],[\"1\",\"2\"]]"
    );
}

// ── URL + URLSearchParams integration ──

#[test]
fn url_search_params_reflects_url_query() {
    assert_eq!(
        eval("new URL('https://example.com/?a=1&b=2').searchParams.get('a')"),
        "1"
    );
}

#[test]
fn url_search_params_updates_url() {
    assert_eq!(
        eval("let u = new URL('https://example.com/'); u.searchParams.append('x', '1'); u.search"),
        "?x=1"
    );
}

#[test]
fn url_set_search_updates_params() {
    assert_eq!(
        eval(
            "let u = new URL('https://example.com/?old=1'); u.search = '?new=2'; u.searchParams.get('new')"
        ),
        "2"
    );
}

#[test]
fn url_set_href_updates_search_params() {
    assert_eq!(
        eval(
            "let u = new URL('https://a.com/?x=1'); u.href = 'https://b.com/?y=2'; u.searchParams.get('y')"
        ),
        "2"
    );
}

#[test]
fn usp_for_of_sees_live_updates() {
    assert_eq!(
        eval(
            "let a = new URL('http://a.b/c?a=1&b=2&c=3&d=4');\
             let seen = [];\
             for (const i of a.searchParams) {\
               a.search = 'x=1&y=2&z=3';\
               seen.push(i[0] + '=' + i[1]);\
             }\
             seen.join(',')"
        ),
        "a=1,y=2,z=3"
    );
}

#[test]
fn usp_construct_record_normalizes_keys_last_wins() {
    // WebIDL `record<USVString, USVString>` conversion normalizes keys to USV
    // and is map-shaped: when two JS keys normalize to the same USVString, the
    // later entry overwrites the earlier one. Mirrors the WPT test
    // "Construct with 2 unpaired surrogates (no trailing)".
    assert_eq!(
        eval(
            "let p = new URLSearchParams({'\\uD835x': '1', 'xx': '2', '\\uD83Dx': '3'});\
             JSON.stringify([...p])"
        ),
        "[[\"\u{fffd}x\",\"3\"],[\"xx\",\"2\"]]"
    );
}

#[test]
fn usp_delete_on_opaque_path_preserves_trailing_space_encoding() {
    assert_eq!(
        eval(
            "const url = new URL('data:space    ?test');\
             url.searchParams.delete('test');\
             url.pathname"
        ),
        "space   %20"
    );
    assert_eq!(
        eval(
            "const url = new URL('data:space    ?test');\
             url.searchParams.delete('test');\
             url.href"
        ),
        "data:space   %20"
    );
}

#[test]
fn usp_delete_on_opaque_path_with_fragment_preserves_trailing_space_encoding() {
    assert_eq!(
        eval(
            "const url = new URL('data:space    ?test#test');\
             url.searchParams.delete('test');\
             url.pathname"
        ),
        "space   %20"
    );
    assert_eq!(
        eval(
            "const url = new URL('data:space    ?test#test');\
             url.searchParams.delete('test');\
             url.href"
        ),
        "data:space   %20#test"
    );
}
