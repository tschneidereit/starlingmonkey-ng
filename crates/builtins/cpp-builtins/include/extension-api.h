// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
#ifndef EXTENSION_API_H
#define EXTENSION_API_H

/**
 * extension-api.h — Builtin compatibility layer (Phase 4).
 *
 * This header provides the same public interface as the original extension-api.h,
 * but all Engine methods delegate to Rust via extern "C" FFI.
 *
 * C++ builtins include this header unchanged and call the same methods they
 * always have. Under the hood, those methods forward to Rust-owned state in
 * starling-runtime/src/engine.rs.
 *
 * What stays in C++:
 *   - AsyncTask class (builtins subclass it; GC tracing needs C++ SM API)
 *   - Event loop task storage (PersistentRooted GC tracing)
 *   - builtin.h (pure C++ SM template metaprogramming)
 *
 * What moves to Rust:
 *   - Engine lifecycle, config, state
 *   - Script loading, module resolution
 *   - Promise rejection tracking
 *   - String encoding/decoding
 */

#include <optional>
#include <vector>
#include "builtin.h"
#include "jsapi.h"
#include "mozilla/WeakPtr.h"

using JS::RootedObject;
using JS::RootedString;
using JS::RootedValue;

using JS::HandleObject;
using JS::HandleValue;
using JS::HandleValueArray;
using JS::MutableHandleValue;

using JS::PersistentRooted;
using JS::PersistentRootedVector;

using std::optional;

using PollableHandle = int32_t;
constexpr PollableHandle INVALID_POLLABLE_HANDLE = -1;
constexpr PollableHandle IMMEDIATE_TASK_HANDLE = -2;

// ── Rust FFI declarations ────────────────────────────────────────────────────
//
// These symbols are defined in starling-runtime (Rust) and linked into the
// final wasm binary.

extern "C" {
void *starling_engine_get_cx();
void *starling_engine_get_global();
void *starling_engine_get_init_global();
uint8_t starling_engine_get_state();
bool starling_engine_debug_logging();
bool starling_engine_debugging_enabled();
bool starling_engine_wpt_mode();
void starling_engine_abort(const uint8_t *reason, uint32_t reason_len);
uint64_t starling_engine_get_script_value();
bool starling_engine_define_builtin_module(const uint8_t *id, uint32_t id_len, uint64_t value);
const uint8_t *starling_engine_init_location(uint32_t *out_len);
bool starling_engine_dump_value(uint64_t val);
bool starling_engine_print_stack();
void starling_engine_dump_pending_exception(const uint8_t *desc, uint32_t desc_len);
bool starling_engine_has_unhandled_rejections();
void starling_engine_report_unhandled_rejections();
void starling_engine_clear_unhandled_rejections();
bool starling_engine_has_pending_async_tasks();
void starling_engine_finish_pre_init();
}

// SM shim functions for error formatting (defined in sm_shim.cpp)
extern "C" {
void sm_dump_error(JSContext *cx, uint64_t error_bits);
void sm_dump_promise_rejection(JSContext *cx, uint64_t reason_bits, JSObject *promise);
}

// Event loop FFI (defined in event_loop.cpp, calls through to Rust)
extern "C" {
int32_t host_api_register_task(int32_t waiter_handle);
void host_api_cancel_task(int32_t task_id);
void host_api_incr_interest();
void host_api_decr_interest();
}

namespace api {

class AsyncTask;

enum class EngineState : uint8_t {
  Uninitialized = 0,
  EngineInitializing = 1,
  ScriptPreInitializing = 2,
  Initialized = 3,
  Aborted = 4,
};

/**
 * Engine — thin FFI wrapper class.
 *
 * All methods delegate to Rust-owned state. The Engine class itself holds no
 * significant state; it exists solely to preserve the existing C++ API surface
 * for builtins.
 *
 * Historical note: Engine used to own the JSContext, globals, and config
 * directly. Now those live in Rust's Engine struct, and this class provides
 * accessor methods that call through FFI.
 */
class Engine {
public:
  /// Get the Engine* from a JSContext (stored as context private data).
  static Engine *get(JSContext *cx) {
    return static_cast<Engine *>(JS_GetContextPrivate(cx));
  }

  /// Get the JSContext.
  static JSContext *cx() {
    return static_cast<JSContext *>(starling_engine_get_cx());
  }

  /// Get the content global object.
  static HandleObject global() {
    // The Rust side holds a PersistentRooted. We return a HandleObject
    // by constructing one from the raw pointer.
    // NOTE: This relies on HandleObject being a thin pointer wrapper,
    // which is true for SpiderMonkey's Handle types when the pointer
    // points to a PersistentRooted's storage.
    static JS::PersistentRootedObject global_;
    JSObject *raw = static_cast<JSObject *>(starling_engine_get_global());
    if (!global_.initialized()) {
      global_.init(cx(), raw);
    } else {
      global_ = raw;
    }
    return global_;
  }

  /// Get the initializer script's global.
  static HandleObject init_script_global() {
    static JS::PersistentRootedObject init_global_;
    JSObject *raw = static_cast<JSObject *>(starling_engine_get_init_global());
    if (!init_global_.initialized()) {
      init_global_.init(cx(), raw);
    } else {
      init_global_ = raw;
    }
    return init_global_;
  }

  EngineState state() {
    return static_cast<EngineState>(starling_engine_get_state());
  }

  bool debugging_enabled() {
    return starling_engine_debugging_enabled();
  }

  bool wpt_mode() {
    return starling_engine_wpt_mode();
  }

  const mozilla::Maybe<std::string> &init_location() const {
    // Cache the value from Rust to return a stable reference.
    static mozilla::Maybe<std::string> cached = mozilla::Nothing();
    uint32_t len = 0;
    const uint8_t *ptr = starling_engine_init_location(&len);
    if (ptr && len > 0) {
      cached = mozilla::Some(std::string(reinterpret_cast<const char *>(ptr), len));
    } else {
      cached = mozilla::Nothing();
    }
    return cached;
  }

  void finish_pre_initialization() {
    starling_engine_finish_pre_init();
  }

  bool define_builtin_module(const char *id, HandleValue builtin) {
    return starling_engine_define_builtin_module(
        reinterpret_cast<const uint8_t *>(id),
        strlen(id),
        builtin.asRawBits());
  }

  bool eval_toplevel(std::string_view path, MutableHandleValue result);
  bool eval_toplevel(JS::SourceText<mozilla::Utf8Unit> &source, std::string_view path,
                     MutableHandleValue result);
  bool run_initialization_script();

  /// Run the event loop (synchronous fallback for pre-init; async loop in Rust for p3).
  bool run_event_loop();

  static void incr_event_loop_interest() {
    host_api_incr_interest();
  }

  static void decr_event_loop_interest() {
    host_api_decr_interest();
  }

  static HandleValue script_value() {
    static JS::PersistentRootedValue script_val_;
    uint64_t raw = starling_engine_get_script_value();
    JS::Value v = JS::Value::fromRawBits(raw);
    if (!script_val_.initialized()) {
      script_val_.init(cx(), v);
    } else {
      script_val_ = v;
    }
    return script_val_;
  }

  static bool has_pending_async_tasks() {
    return starling_engine_has_pending_async_tasks();
  }

  static void queue_async_task(const RefPtr<AsyncTask>& task);

  bool cancel_async_task(const RefPtr<AsyncTask>& task);

  static bool has_unhandled_promise_rejections() {
    return starling_engine_has_unhandled_rejections();
  }

  void report_unhandled_promise_rejections() {
    starling_engine_report_unhandled_rejections();
  }

  static void clear_unhandled_promise_rejections() {
    starling_engine_clear_unhandled_rejections();
  }

  void abort(const char *reason) {
    starling_engine_abort(
        reinterpret_cast<const uint8_t *>(reason),
        strlen(reason));
  }

  static bool debug_logging_enabled() {
    return starling_engine_debug_logging();
  }

  static bool dump_value(JS::Value val, FILE *fp = stdout) {
    return starling_engine_dump_value(val.asRawBits());
  }

  static bool print_stack(FILE *fp = stderr) {
    return starling_engine_print_stack();
  }

  static void dump_error(HandleValue error, FILE *fp = stderr) {
    sm_dump_error(cx(), error.get().asRawBits());
  }

  static void dump_pending_exception(const char *description = "", FILE *fp = stderr) {
    starling_engine_dump_pending_exception(
        reinterpret_cast<const uint8_t *>(description),
        strlen(description));
  }

  static void dump_promise_rejection(HandleValue reason, HandleObject promise, FILE *fp = stderr) {
    sm_dump_promise_rejection(cx(), reason.get().asRawBits(), promise.get());
  }
};


using TaskCompletionCallback = bool (*)(JSContext* cx, HandleObject receiver);

/**
 * AsyncTask — base class for async operations.
 *
 * This class stays in C++ because:
 * 1. Builtins subclass it (TimerTask, BodyFutureTask, etc.)
 * 2. GC tracing requires C++ SpiderMonkey API (JSTracer)
 * 3. RefCounted + SupportsWeakPtr are C++ reference-counting primitives
 *
 * The task_id_ links to the Rust task queue (set by EventLoop::queue_async_task).
 */
class AsyncTask : public js::RefCounted<AsyncTask>, public mozilla::SupportsWeakPtr {
protected:
  PollableHandle handle_ = -1;
  /// Unique task id assigned by the Rust task queue when this task is registered.
  int32_t task_id_ = -1;

public:
  AsyncTask() = default;
  virtual ~AsyncTask() = default;

  AsyncTask(const AsyncTask &) = delete;
  AsyncTask(AsyncTask &&) = delete;

  AsyncTask &operator=(const AsyncTask &) = delete;
  AsyncTask &operator=(AsyncTask &&) = delete;

  virtual bool run(Engine *engine) = 0;
  virtual bool cancel(Engine *engine) = 0;

  [[nodiscard]] virtual PollableHandle id() {
    MOZ_ASSERT(handle_ != INVALID_POLLABLE_HANDLE);
    return handle_;
  }

  [[nodiscard]] virtual uint64_t deadline() {
    return 0;
  }

  /// Get the Rust-side task_id for this task.
  [[nodiscard]] int32_t task_id() const { return task_id_; }
  /// Set the Rust-side task_id (called by EventLoop::queue_async_task).
  void set_task_id(int32_t id) { task_id_ = id; }

  virtual void trace(JSTracer *trc) = 0;
};

} // namespace api

#endif // EXTENSION_API_H
