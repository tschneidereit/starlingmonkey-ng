// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for WebIDL parameterized types:
//! - `Vec<T>` (sequence)
//! - `Record<V>` (record)
//! - `#[webidl_union]` (union)
//! - `AsyncSequence` (async iterable)

// ============================================================================
// Vec<T> (sequence) tests
// ============================================================================

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

mod sequence_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};

    #[jsclass]
    struct SeqAcceptor {}

    #[jsmethods]
    impl SeqAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        /// Accept a sequence of strings and join them.
        #[method]
        fn join_strings(&self, items: Vec<String>) -> String {
            items.join(",")
        }

        /// Accept a sequence of integers and sum them.
        #[method]
        fn sum_ints(&self, items: Vec<i32>) -> i32 {
            items.iter().sum()
        }

        /// Accept an optional sequence.
        #[method]
        fn optional_seq(&self, items: Option<Vec<String>>) -> String {
            match items {
                Some(v) => v.join(","),
                None => "none".to_string(),
            }
        }

        /// Static method accepting a sequence.
        #[static_method]
        fn count(items: Vec<String>) -> u32 {
            items.len() as u32
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    SeqAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    SeqAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn join_string_array() {
        assert_eq!(
            eval("new SeqAcceptor().joinStrings(['a', 'b', 'c'])"),
            "a,b,c"
        );
    }

    #[test]
    fn join_empty_array() {
        assert_eq!(eval("new SeqAcceptor().joinStrings([])"), "");
    }

    #[test]
    fn sum_int_array() {
        assert_eq!(eval("new SeqAcceptor().sumInts([1, 2, 3])"), "6");
    }

    #[test]
    fn sum_empty_array() {
        assert_eq!(eval("new SeqAcceptor().sumInts([])"), "0");
    }

    #[test]
    fn optional_seq_present() {
        assert_eq!(eval("new SeqAcceptor().optionalSeq(['x', 'y'])"), "x,y");
    }

    #[test]
    fn optional_seq_null_throws() {
        // `null` is a present value (not absent); converting it to a sequence throws (not iterable).
        assert!(throws("new SeqAcceptor().optionalSeq(null)"));
    }

    #[test]
    fn optional_seq_undefined() {
        assert_eq!(eval("new SeqAcceptor().optionalSeq(undefined)"), "none");
    }

    #[test]
    fn static_count() {
        assert_eq!(eval("SeqAcceptor.count(['a', 'b'])"), "2");
    }

    #[test]
    fn iterable_protocol() {
        // Any iterable should work, not just arrays.
        assert_eq!(
            eval("new SeqAcceptor().joinStrings(new Set(['x', 'y']))"),
            "x,y"
        );
    }

    #[test]
    fn non_iterable_throws() {
        assert!(throws("new SeqAcceptor().joinStrings(42)"));
    }
}

// ============================================================================
// Record<V> tests
// ============================================================================

mod record_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};
    use js::Record;

    #[jsclass]
    struct RecordAcceptor {}

    #[jsmethods]
    impl RecordAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        /// Accept a record of string values, return keys joined.
        #[method]
        fn keys(&self, rec: Record<String, String>) -> String {
            rec.keys().cloned().collect::<Vec<_>>().join(",")
        }

        /// Accept a record of string values, return values joined.
        #[method]
        fn values(&self, rec: Record<String, String>) -> String {
            rec.values().cloned().collect::<Vec<_>>().join(",")
        }

        /// Accept an optional record.
        #[method]
        fn optional_record(&self, rec: Option<Record<String, String>>) -> String {
            match rec {
                Some(r) => r.keys().cloned().collect::<Vec<_>>().join(","),
                None => "none".to_string(),
            }
        }

        /// Echo the record back to JS (exercises `Record::to_jsval`).
        #[method]
        fn echo(&self, rec: Record<String, String>) -> Record<String, String> {
            rec
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    RecordAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    RecordAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn record_keys() {
        assert_eq!(eval("new RecordAcceptor().keys({a: '1', b: '2'})"), "a,b");
    }

    #[test]
    fn record_values() {
        assert_eq!(
            eval("new RecordAcceptor().values({x: 'hello', y: 'world'})"),
            "hello,world"
        );
    }

    #[test]
    fn record_preserves_insertion_order() {
        assert_eq!(
            eval("new RecordAcceptor().keys({z: '1', a: '2', m: '3'})"),
            "z,a,m"
        );
    }

    #[test]
    fn empty_record() {
        assert_eq!(eval("new RecordAcceptor().keys({})"), "");
    }

    #[test]
    fn optional_record_present() {
        assert_eq!(eval("new RecordAcceptor().optionalRecord({k: 'v'})"), "k");
    }

    #[test]
    fn optional_record_null_throws() {
        // Per WebIDL, only `undefined`/a missing argument makes an optional argument absent; `null`
        // is a present value, and converting it to a record throws (record conversion requires an
        // object).
        assert!(throws("new RecordAcceptor().optionalRecord(null)"));
    }

    #[test]
    fn record_roundtrip_non_ascii_keys() {
        // Keys must survive the JS→Rust→JS round trip; a Latin-1 write of
        // the UTF-8 key bytes would mojibake the property name.
        assert_eq!(
            eval(
                "const r = new RecordAcceptor().echo({'é🦊': 'vé'}); \
                 Object.keys(r).join(',') + ':' + r['é🦊']"
            ),
            "é🦊:vé"
        );
    }

    #[test]
    fn record_roundtrip_interior_nul_key() {
        // A NUL is a legal property-name code unit; a C-string write would
        // fail and surface as a phantom exception.
        assert_eq!(
            eval(
                "const r = new RecordAcceptor().echo({'a\\u0000b': 'v'}); \
                 Object.keys(r).map(k => k.length).join(',') + ':' + r['a\\u0000b']"
            ),
            "3:v"
        );
    }

    #[test]
    fn non_object_throws() {
        assert!(throws("new RecordAcceptor().keys(42)"));
    }

    #[test]
    fn symbol_keys_throw() {
        // Per WebIDL es-record, every enumerable own key — including symbols — is converted to the
        // key type. Converting a Symbol to a string throws a TypeError, so an enumerable
        // symbol-keyed property makes the whole conversion throw.
        assert!(throws(
            "let o = {a: '1'}; o[Symbol('x')] = '2'; new RecordAcceptor().keys(o)"
        ));
    }

    #[test]
    fn integer_keys_kept() {
        // Integer-index property keys are own enumerable keys; per WebIDL they are converted to
        // their string form and kept (the [[OwnPropertyKeys]] order lists them first, in ascending
        // numeric order), not skipped.
        assert_eq!(eval("new RecordAcceptor().keys({1: 'a', 0: 'b'})"), "0,1");
    }

    #[test]
    fn skips_non_enumerable_keys() {
        assert_eq!(
            eval(
                "let o = {a: '1'}; Object.defineProperty(o, 'b', {value: '2', enumerable: false}); \
                 new RecordAcceptor().keys(o)"
            ),
            "a"
        );
    }
}

// ============================================================================
// Union type tests
// ============================================================================

mod union_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;
    use core_runtime::webidl_union;

    #[webidl_union]
    pub enum StringOrLong {
        Str(String),
        Long(i32),
    }

    #[webidl_union]
    pub enum StringOrBool {
        Str(String),
        Bool(bool),
    }

    // No string member: the numeric member is the §3.2.25 fallback (ToNumber).
    #[webidl_union]
    pub enum BoolOrLong {
        Bool(bool),
        Long(i32),
    }

    // No string or numeric member: the boolean member is the fallback (ToBoolean).
    #[webidl_union]
    pub enum SeqOrBool {
        Items(Vec<String>),
        Flag(bool),
    }

    #[jsclass]
    struct UnionAcceptor {}

    #[jsmethods]
    impl UnionAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        #[method]
        fn string_or_long(&self, val: StringOrLong) -> String {
            match val {
                StringOrLong::Str(s) => format!("string:{s}"),
                StringOrLong::Long(n) => format!("long:{n}"),
            }
        }

        #[method]
        fn string_or_bool(&self, val: StringOrBool) -> String {
            match val {
                StringOrBool::Str(s) => format!("string:{s}"),
                StringOrBool::Bool(b) => format!("bool:{b}"),
            }
        }

        #[method]
        fn optional_union(&self, val: Option<StringOrLong>) -> String {
            match val {
                Some(StringOrLong::Str(s)) => format!("string:{s}"),
                Some(StringOrLong::Long(n)) => format!("long:{n}"),
                None => "none".to_string(),
            }
        }

        #[method]
        fn bool_or_long(&self, val: BoolOrLong) -> String {
            match val {
                BoolOrLong::Bool(b) => format!("bool:{b}"),
                BoolOrLong::Long(n) => format!("long:{n}"),
            }
        }

        #[method]
        fn seq_or_bool(&self, val: SeqOrBool) -> String {
            match val {
                SeqOrBool::Items(v) => format!("seq:{}", v.len()),
                SeqOrBool::Flag(b) => format!("flag:{b}"),
            }
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    UnionAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn string_or_long_with_number() {
        assert_eq!(eval("new UnionAcceptor().stringOrLong(42)"), "long:42");
    }

    #[test]
    fn string_or_long_with_string() {
        assert_eq!(
            eval("new UnionAcceptor().stringOrLong('hello')"),
            "string:hello"
        );
    }

    #[test]
    fn string_or_bool_with_true() {
        assert_eq!(eval("new UnionAcceptor().stringOrBool(true)"), "bool:true");
    }

    #[test]
    fn string_or_bool_with_string() {
        assert_eq!(
            eval("new UnionAcceptor().stringOrBool('test')"),
            "string:test"
        );
    }

    #[test]
    fn optional_union_null() {
        // `null` is a present value (not absent); a union with a string member converts null via
        // that member, yielding "null".
        assert_eq!(
            eval("new UnionAcceptor().optionalUnion(null)"),
            "string:null"
        );
    }

    #[test]
    fn optional_union_undefined() {
        assert_eq!(eval("new UnionAcceptor().optionalUnion(undefined)"), "none");
    }

    #[test]
    fn optional_union_with_value() {
        assert_eq!(eval("new UnionAcceptor().optionalUnion(99)"), "long:99");
    }

    #[test]
    fn number_priority_over_string() {
        // When a value is a number and union includes both numeric and string,
        // numeric should take priority per spec.
        assert_eq!(eval("new UnionAcceptor().stringOrLong(3.14)"), "long:3");
    }

    #[test]
    fn boolean_priority_over_string() {
        // Boolean should take priority over string per spec.
        assert_eq!(
            eval("new UnionAcceptor().stringOrBool(false)"),
            "bool:false"
        );
    }

    #[test]
    fn numeric_fallback_without_string_member() {
        // BoolOrLong has no string member, so a non-boolean value coerces to the
        // numeric member via ToNumber (WebIDL §3.2.25 fallback).
        assert_eq!(eval("new UnionAcceptor().boolOrLong('5')"), "long:5");
        assert_eq!(eval("new UnionAcceptor().boolOrLong(7)"), "long:7");
        assert_eq!(eval("new UnionAcceptor().boolOrLong(true)"), "bool:true");
    }

    #[test]
    fn boolean_fallback_without_string_or_numeric_member() {
        // SeqOrBool has neither a string nor a numeric member, so a non-iterable
        // value coerces to the boolean member via ToBoolean.
        assert_eq!(eval("new UnionAcceptor().seqOrBool(5)"), "flag:true");
        assert_eq!(eval("new UnionAcceptor().seqOrBool(0)"), "flag:false");
        assert_eq!(
            eval("new UnionAcceptor().seqOrBool(['a', 'b', 'c'])"),
            "seq:3"
        );
    }
}

mod union_lifetime_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};
    use core_runtime::webidl_union;
    use js::ArrayBuffer;
    use js::Object;

    /// Union with a scope-rooted variant and a primitive variant.
    #[webidl_union]
    pub enum BufferOrString<'a> {
        Buffer(ArrayBuffer<'a>),
        Str(String),
    }

    /// Union with two scope-rooted variants of different specificity. The
    /// `ArrayBuffer` branch is declared first so it wins for typed buffers;
    /// other objects fall through to the generic `Object` branch.
    #[webidl_union]
    pub enum BufferOrObject<'a> {
        Buffer(ArrayBuffer<'a>),
        Obj(Object<'a>),
    }

    #[jsclass]
    struct LifetimeUnionAcceptor {}

    #[jsmethods]
    impl LifetimeUnionAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        #[method]
        fn buffer_or_string(&self, val: BufferOrString<'_>) -> String {
            match val {
                BufferOrString::Buffer(b) => format!("buffer:{}", b.byte_length()),
                BufferOrString::Str(s) => format!("string:{s}"),
            }
        }

        #[method]
        fn optional_buffer_or_string(&self, val: Option<BufferOrString<'_>>) -> String {
            match val {
                Some(BufferOrString::Buffer(b)) => format!("buffer:{}", b.byte_length()),
                Some(BufferOrString::Str(s)) => format!("string:{s}"),
                None => "none".to_string(),
            }
        }

        #[method]
        fn buffer_or_object(&self, val: BufferOrObject<'_>) -> String {
            match val {
                BufferOrObject::Buffer(b) => format!("buffer:{}", b.byte_length()),
                BufferOrObject::Obj(_) => "object".to_string(),
            }
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    LifetimeUnionAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    LifetimeUnionAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn buffer_variant_with_array_buffer() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrString(new ArrayBuffer(8))"),
            "buffer:8"
        );
    }

    #[test]
    fn string_variant_with_string() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrString('hello')"),
            "string:hello"
        );
    }

    #[test]
    fn string_variant_with_number() {
        // No numeric branch is declared, so the string fallback applies.
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrString(42)"),
            "string:42"
        );
    }

    #[test]
    fn string_variant_when_object_does_not_match_buffer() {
        // Plain objects don't satisfy the ArrayBuffer branch and fall through
        // to the string fallback (after `Object.prototype.toString`).
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrString({})"),
            "string:[object Object]"
        );
    }

    #[test]
    fn optional_buffer_or_string_null() {
        // `null` is a present value (not absent); a union with a string member converts null via
        // that member, yielding "null".
        assert_eq!(
            eval("new LifetimeUnionAcceptor().optionalBufferOrString(null)"),
            "string:null"
        );
    }

    #[test]
    fn optional_buffer_or_string_undefined() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().optionalBufferOrString(undefined)"),
            "none"
        );
    }

    #[test]
    fn optional_buffer_or_string_with_buffer() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().optionalBufferOrString(new ArrayBuffer(4))"),
            "buffer:4"
        );
    }

    #[test]
    fn declared_order_picks_buffer_first() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrObject(new ArrayBuffer(16))"),
            "buffer:16"
        );
    }

    #[test]
    fn declared_order_falls_back_to_object() {
        assert_eq!(
            eval("new LifetimeUnionAcceptor().bufferOrObject({a: 1})"),
            "object"
        );
    }

    #[test]
    fn buffer_or_object_throws_on_non_object() {
        assert!(throws(
            "new LifetimeUnionAcceptor().bufferOrObject('not an object')"
        ));
    }
}

// ============================================================================
// Sequence-or-record union tests (e.g. URLSearchParams init shape)
// ============================================================================

mod union_sequence_record_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};
    use core_runtime::webidl_union;
    use js::conversion::Record;

    /// Mirrors the WebIDL signature
    /// `(sequence<sequence<USVString>> or record<USVString, USVString> or USVString)`
    /// used by `URLSearchParams`.
    #[webidl_union]
    pub enum SeqOrRecordOrString {
        Pairs(Vec<Vec<String>>),
        Record(Record<String, String>),
        Str(String),
    }

    #[jsclass]
    struct SeqRecordAcceptor {}

    #[jsmethods]
    impl SeqRecordAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        #[method]
        fn classify(&self, val: SeqOrRecordOrString) -> String {
            match val {
                SeqOrRecordOrString::Pairs(pairs) => {
                    let mut parts = Vec::new();
                    for pair in pairs {
                        parts.push(pair.join("="));
                    }
                    format!("pairs:{}", parts.join("&"))
                }
                SeqOrRecordOrString::Record(record) => {
                    let mut entries: Vec<String> = record
                        .into_iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    entries.sort();
                    format!("record:{}", entries.join("&"))
                }
                SeqOrRecordOrString::Str(s) => format!("string:{s}"),
            }
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    SeqRecordAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    SeqRecordAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn array_of_pairs_picks_sequence() {
        assert_eq!(
            eval("new SeqRecordAcceptor().classify([['a','1'], ['b','2']])"),
            "pairs:a=1&b=2"
        );
    }

    #[test]
    fn plain_object_picks_record() {
        assert_eq!(
            eval("new SeqRecordAcceptor().classify({a:'1', b:'2'})"),
            "record:a=1&b=2"
        );
    }

    #[test]
    fn string_picks_string() {
        assert_eq!(
            eval("new SeqRecordAcceptor().classify('hello')"),
            "string:hello"
        );
    }

    #[test]
    fn custom_iterable_picks_sequence() {
        // A non-Array iterable with @@iterator must still be routed to the
        // sequence branch.
        assert_eq!(
            eval(
                "let it = { [Symbol.iterator]: function*() { yield ['a','1']; yield ['b','2']; } };\
                 new SeqRecordAcceptor().classify(it)"
            ),
            "pairs:a=1&b=2"
        );
    }

    #[test]
    fn iterable_with_non_iterable_element_throws() {
        // WebIDL §3.2.25: once @@iterator is detected the sequence branch is
        // selected; if an element can't be converted to the inner sequence
        // type, the conversion *throws* — it does not silently fall through
        // to the record branch.
        assert!(throws("new SeqRecordAcceptor().classify([1, 2])"));
    }

    #[test]
    fn iterable_throwing_iterator_propagates() {
        // If the iterator throws, the failure propagates as an exception
        // rather than being silently swallowed.
        assert!(throws(
            "let it = { [Symbol.iterator]: function() { return { next() { throw new Error('boom'); } }; } };\
             new SeqRecordAcceptor().classify(it)"
        ));
    }
}

// ============================================================================
// AsyncSequence tests
// ============================================================================

mod async_sequence_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};
    use js::AsyncSequence;

    #[jsclass]
    struct AsyncAcceptor {}

    #[jsmethods]
    impl AsyncAcceptor {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        /// Accept an async iterable and report whether it's async.
        #[method]
        fn is_async(&self, seq: AsyncSequence) -> bool {
            seq.is_async()
        }

        /// Accept an optional async iterable.
        #[method]
        fn optional_async(&self, seq: Option<AsyncSequence>) -> String {
            match seq {
                Some(s) => if s.is_async() { "async" } else { "sync" }.to_string(),
                None => "none".to_string(),
            }
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    AsyncAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    AsyncAcceptor::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn sync_iterable_detected() {
        // Arrays have Symbol.iterator but not Symbol.asyncIterator.
        assert_eq!(eval("new AsyncAcceptor().isAsync([1, 2, 3])"), "false");
    }

    #[test]
    fn async_iterable_detected() {
        assert_eq!(
            eval(
                "let obj = { [Symbol.asyncIterator]() { return { next() { return { done: true } } } } }; \
                 new AsyncAcceptor().isAsync(obj)"
            ),
            "true"
        );
    }

    #[test]
    fn optional_null_throws() {
        // Per WebIDL, only `undefined`/a missing argument makes an optional argument absent; `null`
        // is a present value, and converting it to a sequence/async iterable throws (null is not
        // iterable).
        assert!(throws("new AsyncAcceptor().optionalAsync(null)"));
    }

    #[test]
    fn optional_with_iterable() {
        assert_eq!(eval("new AsyncAcceptor().optionalAsync([1])"), "sync");
    }

    #[test]
    fn non_iterable_throws() {
        assert!(throws("new AsyncAcceptor().isAsync(42)"));
    }

    #[test]
    fn plain_object_throws() {
        assert!(throws("new AsyncAcceptor().isAsync({})"));
    }

    #[test]
    fn throwing_getter_propagates() {
        // GetMethod: a Get error propagates; it must not be swallowed and
        // fall back to Symbol.iterator.
        assert_eq!(
            eval(
                "let o = { get [Symbol.asyncIterator]() { throw new Error('trap') }, \
                           [Symbol.iterator]() { return { next() { return { done: true } } } } }; \
                 try { new AsyncAcceptor().isAsync(o); 'no throw' } catch (e) { e.message }"
            ),
            "trap"
        );
    }

    #[test]
    fn non_callable_method_throws() {
        // GetMethod: a present, non-callable method is a TypeError, not a
        // fallback to Symbol.iterator.
        assert_eq!(
            eval(
                "let o = { [Symbol.asyncIterator]: 42, \
                           [Symbol.iterator]() { return { next() { return { done: true } } } } }; \
                 try { new AsyncAcceptor().isAsync(o); 'no throw' } catch (e) { e.constructor.name }"
            ),
            "TypeError"
        );
    }
}
