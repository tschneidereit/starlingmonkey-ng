// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for WebIDL parameterized types:
//! - `Vec<T>` (sequence)
//! - `Record<V>` (record)
//! - `#[webidl_union]` (union)
//! - `AsyncSequence` (async iterable)

// ============================================================================
// Vec<T> (sequence) tests
// ============================================================================

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
    fn optional_seq_null() {
        assert_eq!(eval("new SeqAcceptor().optionalSeq(null)"), "none");
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
        fn keys(&self, rec: Record<String>) -> String {
            rec.keys().cloned().collect::<Vec<_>>().join(",")
        }

        /// Accept a record of string values, return values joined.
        #[method]
        fn values(&self, rec: Record<String>) -> String {
            rec.values().cloned().collect::<Vec<_>>().join(",")
        }

        /// Accept an optional record.
        #[method]
        fn optional_record(&self, rec: Option<Record<String>>) -> String {
            match rec {
                Some(r) => r.keys().cloned().collect::<Vec<_>>().join(","),
                None => "none".to_string(),
            }
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
    fn optional_record_null() {
        assert_eq!(eval("new RecordAcceptor().optionalRecord(null)"), "none");
    }

    #[test]
    fn non_object_throws() {
        assert!(throws("new RecordAcceptor().keys(42)"));
    }

    #[test]
    fn skips_symbol_keys() {
        // Symbol keys should be ignored in record conversion.
        assert_eq!(
            eval("let o = {a: '1'}; o[Symbol('x')] = '2'; new RecordAcceptor().keys(o)"),
            "a"
        );
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
        assert_eq!(eval("new UnionAcceptor().optionalUnion(null)"), "none");
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
        assert_eq!(
            eval("new LifetimeUnionAcceptor().optionalBufferOrString(null)"),
            "none"
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
    fn optional_null() {
        assert_eq!(eval("new AsyncAcceptor().optionalAsync(null)"), "none");
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
}
