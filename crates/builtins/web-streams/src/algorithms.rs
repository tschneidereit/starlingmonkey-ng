// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Standalone algorithms from <https://streams.spec.whatwg.org/>

use js::{
    error::ExnThrown,
    gc::{handle::Heap, scope::Scope},
    heap::RootedTraceableBox,
    native::Value,
    prelude::{CallbackArgs, HandleValue},
    value, Array, Function, Object, Promise,
};

use crate::queuing::{QueueWithSizes, QueuingStrategy, ValueWithSize};

/// Materialise a `TypeError` as a value without throwing it: many stream steps
/// reject a promise "with a `TypeError` exception" rather than throwing.
pub fn make_type_error<'r>(scope: &'r Scope<'_>, message: &std::ffi::CStr) -> HandleValue<'r> {
    js::error::throw_type_error(scope, message);
    // Root the pending exception before clearing it — never hold it as a bare
    // `Value` local.
    let error = scope.root_value(
        js::exception::get_pending(scope)
            .expect("a TypeError was just thrown")
            .get(),
    );
    js::exception::clear(scope);
    error
}

/// Resolve an optional promise slot with undefined, if it is set.
pub(crate) fn resolve_promise_slot_undefined(scope: &Scope<'_>, promise: &Promise<'_>) {
    let undef = scope.root_value(value::undefined());
    promise.resolve(scope, undef).expect("resolve promise");
}

/// Pack two values into a JS object for a two-state reaction payload.
///
/// The elements are set with `set_element` (which takes already-rooted
/// `HandleValue`s) rather than via a stack `HandleValueArray`: building the array
/// with `NewArrayObject` would copy the two values into an unrooted stack slot
/// and only read them *after* allocating the array, so a GC triggered by that
/// allocation could move a young value (e.g. a freshly created promise), leaving
/// the array element pointing at a stale cell.
pub(crate) fn pair_payload<'r>(
    scope: &'r Scope<'_>,
    a: HandleValue<'_>,
    b: HandleValue<'_>,
) -> Result<HandleValue<'r>, ExnThrown> {
    let obj = Object::new_plain(scope)?;
    obj.set_element(scope, 0, a)?;
    obj.set_element(scope, 1, b)?;
    Ok(scope.root_value(obj.as_value()))
}

/// Build the `CreateArrayFromList(«a, b»)` two-element array for a composite
/// cancel reason, root-safely. Like [`pair_payload`], the elements are set with
/// already-rooted `HandleValue`s rather than copied through a stack
/// `HandleValueArray`, which `Array::with_contents` would read only after an
/// allocation that can move a young value out from under the stale copy.
pub(crate) fn composite_reason<'r>(
    scope: &'r Scope<'_>,
    a: HandleValue<'_>,
    b: HandleValue<'_>,
) -> Result<HandleValue<'r>, ExnThrown> {
    let attrs = js::class_spec::JSPROP_ENUMERATE as std::ffi::c_uint;
    let arr = Array::new(scope, 2)?;
    arr.define_element(scope, 0, a, attrs)?;
    arr.define_element(scope, 1, b, attrs)?;
    Ok(scope.root_value(arr.as_value()))
}

/// Unpack a two-value reaction payload.
pub(crate) fn pair_parts<'r>(
    scope: &'r Scope<'_>,
    payload: HandleValue<'_>,
) -> (HandleValue<'r>, HandleValue<'r>) {
    let arr = Object::from_value(scope, *payload).expect("payload is an array");
    (
        arr.get_element(scope, 0).expect("element 0"),
        arr.get_element(scope, 1).expect("element 1"),
    )
}

/// Recover a rooted stream/reader/controller newtype `T` from a reaction payload
/// value. Reaction payloads are wired internally by this crate, so the value is
/// always a JS object of the expected class — a mismatch is a wiring bug, hence
/// the panics.
pub(crate) fn cast_payload<'r, T>(scope: &'r Scope<'_>, payload: HandleValue<'_>) -> T
where
    T: js::builtins::CastTarget<'r, Output = T>,
{
    Object::from_value(scope, *payload)
        .expect("reaction payload is an object")
        .cast::<T>()
        .expect("reaction payload has the expected class")
}

/// <https://streams.spec.whatwg.org/#validate-and-normalize-high-water-mark>
/// ExtractHighWaterMark(strategy, defaultHWM) performs the following steps:
pub(crate) fn extract_high_water_mark(
    scope: &Scope<'_>,
    strategy: &Option<QueuingStrategy<'_>>,
    default_hwm: f64,
) -> Result<f64, ExnThrown> {
    // Step 1: If _strategy_["``highWaterMark``"] does not `exist`, return _defaultHWM_.
    let high_water_mark = match strategy.as_ref().and_then(|s| s.high_water_mark) {
        None => return Ok(default_hwm),
        // Step 2: Let _highWaterMark_ be _strategy_["``highWaterMark``"].
        Some(hwm) => hwm,
    };
    // Step 3: If _highWaterMark_ is NaN or _highWaterMark_ < 0, throw a ``RangeError`` exception.
    if high_water_mark.is_nan() || high_water_mark < 0.0 {
        return Err(js::error::throw_range_error(
            scope,
            c"highWaterMark must be a non-negative number",
        ));
    }
    // Step 4: Return _highWaterMark_.
    Ok(high_water_mark)
}

/// <https://streams.spec.whatwg.org/#make-size-algorithm-from-size-function>
/// ExtractSizeAlgorithm(strategy) performs the following steps:
///
/// In this implementation a controller's size algorithm is stored as a JS
/// callable. A missing `size` member — "an algorithm that returns 1" — becomes
/// a native callback returning 1 ([`default_size_algorithm`]); otherwise the
/// algorithm is the `size` callback itself, later `invoked` with argument list
/// « chunk ». Either way the stored value is callable while the stream is in
/// use (it is only reset to `undefined` by `…ClearAlgorithms`).
pub(crate) fn extract_size_algorithm(
    scope: &Scope<'_>,
    strategy: &Option<QueuingStrategy<'_>>,
) -> Result<Value, ExnThrown> {
    // Step 1: If _strategy_["``size``"] does not `exist`, return an algorithm that returns 1.
    // Step 2: Return an algorithm that performs the following steps, taking a _chunk_ argument:
    //         Return the result of `invoking` _strategy_["``size``"] with argument list « _chunk_
    //         ».
    // `size` is a `QueuingStrategySize` callback type: held as an `Object`, so its callability —
    // checked by the WebIDL conversion of callback function types — is enforced here. Both branches
    // return a callable value; the controllers store and invoke it uniformly, so `[[strategySizeAlgorithm]]`
    // is never undefined while the stream is in use.
    match strategy.as_ref().and_then(|s| s.size.as_ref()) {
        Some(size_fn) if size_fn.is_callable() => Ok(size_fn.as_value()),
        Some(_) => Err(js::error::throw_type_error(
            scope,
            c"queuing strategy size must be a function",
        )),
        None => Function::new_callback(
            scope,
            c"size",
            1,
            default_size_algorithm,
            value::undefined(),
        )
        .map(|f| f.as_value()),
    }
}

/// The default `QueuingStrategySize` algorithm — `ExtractSizeAlgorithm` step 1's
/// "an algorithm that returns 1". Used when a strategy omits `size`.
fn default_size_algorithm(
    _scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    _payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    Ok(value::from_f64(1.0))
}

/// <https://streams.spec.whatwg.org/#dequeue-value>
/// DequeueValue(container) performs the following steps:
pub(crate) fn dequeue_value<'r>(
    scope: &'r Scope<'_>,
    container: &mut impl QueueWithSizes,
) -> HandleValue<'r> {
    // Step 1: Assert: _container_ has [[queue]] and [[queueTotalSize]] internal slots.
    //         (Guaranteed by the `QueueWithSizes` trait bound.)
    // Step 2: Assert: _container_.[[queue]] is not `empty`.
    debug_assert!(!container.queue().is_empty());
    // Step 3: Let _valueWithSize_ be _container_.[[queue]][0].
    // Step 4: `Remove` _valueWithSize_ from _container_.[[queue]].
    // Once popped, `value_with_size` is no longer reached by the queue's trace
    // hook, so it must be rooted: extracting `value` below roots it, and rooting
    // can itself trigger a compacting GC under zeal — which would stale this
    // entry's untraced `Heap<Value>` and crash its drop write barrier. The
    // `RootedTraceableBox` keeps the entry traced across that window.
    let value_with_size = RootedTraceableBox::new(container.queue_mut().pop_front().unwrap());
    // Step 5: Set _container_.[[queueTotalSize]] to _container_.[[queueTotalSize]] −
    //         _valueWithSize_’s `size`.
    let mut total = container.queue_total_size() - value_with_size.size;
    // Step 6: If _container_.[[queueTotalSize]] < 0, set _container_.[[queueTotalSize]] to 0. (This
    //         can occur due to rounding errors.)
    if total < 0.0 {
        total = 0.0;
    }
    container.set_queue_total_size(total);
    // Step 7: Return _valueWithSize_’s `value`.
    value_with_size.value.get(scope)
}

/// <https://streams.spec.whatwg.org/#enqueue-value-with-size>
/// EnqueueValueWithSize(container, value, size) performs the following steps:
pub(crate) fn enqueue_value_with_size(
    scope: &Scope<'_>,
    container: &mut impl QueueWithSizes,
    value: HandleValue<'_>,
    size: f64,
) -> Result<(), ExnThrown> {
    // Step 1: Assert: _container_ has [[queue]] and [[queueTotalSize]] internal slots.
    //         (Guaranteed by the `QueueWithSizes` trait bound.)
    // Step 2: If ! `IsNonNegativeNumber`(_size_) is false, throw a ``RangeError`` exception.
    if !is_non_negative_number(size) {
        return Err(js::error::throw_range_error(
            scope,
            c"Size must be a finite, non-negative number",
        ));
    }
    // Step 3: If _size_ is +∞, throw a ``RangeError`` exception.
    if size == f64::INFINITY {
        return Err(js::error::throw_range_error(
            scope,
            c"Size must be a finite, non-negative number",
        ));
    }
    // Step 4: `Append` a new `value-with-size` with `value` _value_ and `size` _size_ to
    //         _container_.[[queue]].
    container.queue_mut().push_back(ValueWithSize {
        value: Heap::from(value.get()),
        size,
        is_close_sentinel: false,
    });
    // Step 5: Set _container_.[[queueTotalSize]] to _container_.[[queueTotalSize]] + _size_.
    let total = container.queue_total_size() + size;
    container.set_queue_total_size(total);
    Ok(())
}

/// <https://streams.spec.whatwg.org/#reset-queue>
/// ResetQueue(container) performs the following steps:
pub(crate) fn reset_queue(container: &mut impl QueueWithSizes) {
    // Step 1: Assert: _container_ has [[queue]] and [[queueTotalSize]] internal slots.
    //         (Guaranteed by the `QueueWithSizes` trait bound.)
    // Step 2: Set _container_.[[queue]] to a new empty `list`.
    container.queue_mut().clear();
    // Step 3: Set _container_.[[queueTotalSize]] to 0.
    container.set_queue_total_size(0.0);
}

/// <https://streams.spec.whatwg.org/#is-non-negative-number>
/// IsNonNegativeNumber(v) performs the following steps:
///
/// Every caller in this implementation already holds an `f64`: queue sizes flow
/// through the size algorithm's `unrestricted double` return type, which applies
/// `ToNumber`. Step 1 ("is not a Number") is therefore vacuous and folded into
/// the `f64` representation.
pub(crate) fn is_non_negative_number(v: f64) -> bool {
    // Step 1: If _v_ `is not a Number`, return false.
    // Step 2: If _v_ is NaN, return false.
    if v.is_nan() {
        return false;
    }
    // Step 3: If _v_ < 0, return false.
    if v < 0.0 {
        return false;
    }
    // Step 4: Return true.
    true
}

/// A promise resolved with undefined.
pub(crate) fn resolved_undefined_promise<'r>(scope: &'r Scope<'_>) -> Promise<'r> {
    let undef = scope.root_value(value::undefined());
    Promise::new_resolved_with_value(scope, undef).expect("resolved promise")
}
