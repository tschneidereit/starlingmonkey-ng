// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

// Implementation of a subset of the Node.js `process` global object.

use js::conversion::{FromJSVal, ToJSVal};
use js::error::TypeError;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::heap::RootedTraceableBox;
use js::native::{CallArgs, Value};
use js::Object;
use std::io::{BufRead, Write as _};

use core_runtime::event_loop::{with_active_event_loop, Task, TaskId};
use core_runtime::jsnamespace;

/// `process.version` — the Node.js version string we report for compatibility.
/// Per the Node.js docs this always carries a leading `v`; `process.versions.node`
/// exposes the same version without it.
const NODE_VERSION: &str = "v20.11.0";
const NODE_VERSION_BARE: &str = "20.11.0";

fn platform() -> &'static str {
    #[allow(unreachable_code)]
    {
        #[cfg(target_os = "linux")]
        return "linux";
        #[cfg(target_os = "macos")]
        return "darwin";
        #[cfg(target_os = "windows")]
        return "win32";
        #[cfg(target_arch = "wasm32")]
        return "wasm32";
        std::env::consts::OS
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        "arm" => "arm",
        other => other,
    }
}

fn build_versions_object<'s>(scope: &'s Scope<'_>) -> Object<'s> {
    let obj = js::Object::new_plain(scope).unwrap();
    let _ = obj.set_property(scope, c"node", NODE_VERSION_BARE.to_string());
    let _ = obj.set_property(scope, c"starling", env!("CARGO_PKG_VERSION").to_string());
    obj
}

fn build_argv<'s>(scope: &'s Scope<'s>) -> js::Array<'s> {
    let args: Vec<String> = std::env::args().collect();
    let arr = js::Array::new(scope, args.len()).unwrap();
    for (i, arg) in args.into_iter().enumerate() {
        if let Ok(val) = arg.to_jsval_raw_throwing(scope) {
            let _ = arr.set_element(scope, i as u32, scope.root_value(val));
        }
    }
    arr
}

fn build_env_object<'s>(scope: &'s Scope<'_>) -> Object<'s> {
    let obj = js::Object::new_plain(scope).unwrap();
    for (key, value) in std::env::vars() {
        // POSIX env keys cannot contain NUL; skip any that somehow do.
        let Ok(ckey) = std::ffi::CString::new(key) else { continue };
        let _ = obj.set_property(scope, ckey.as_c_str(), value);
    }
    obj
}

// process.stdin: synchronous readable stream.
// read() returns the next line as a string, or null at EOF.
// TODO: size argument (read([size])), event API (on/resume/pipe), setEncoding.
fn build_stdin_object<'s>(scope: &'s Scope<'_>) -> Object<'s> {
    use std::io::IsTerminal;
    let obj = js::Object::new_plain(scope).unwrap();
    let _ = obj.set_property(scope, c"isTTY", std::io::stdin().is_terminal());
    let read_fn = js::Function::new_callback(
        scope,
        c"read",
        0,
        |scope, _args, _p| {
            
            let mut line = String::new();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(0) => Ok(js::value::null()),
                Ok(_) => line.to_jsval_raw_throwing(scope),
                Err(_) => Ok(js::value::null()),
            }
        },
        js::value::undefined(),
    )
    .unwrap();
    let _ = obj.set_property(scope, c"read", read_fn);
    obj
}

// Writable stream object for process.stdout / process.stderr.
// write(str) writes the string to the underlying OS stream and returns true.
// The private value passed to the callback is `true` for stderr, `false` for stdout.
fn build_stream_object<'s>(scope: &'s Scope<'_>, is_stderr: bool) -> Object<'s> {
    use std::io::IsTerminal;
    let (is_tty, payload) = if is_stderr {
        (std::io::stderr().is_terminal(), js::value::from_bool(true))
    } else {
        (std::io::stdout().is_terminal(), js::value::from_bool(false))
    };
    let obj = js::Object::new_plain(scope).unwrap();
    let write_fn = js::Function::new_callback(
        scope,
        c"write",
        1,
        |scope, args, p| {
            if args.len() > 0 {
                let s = String::from_jsval(scope, args.get(0), ()).unwrap_or_default();
                if (*p).to_boolean() {
                    let _ = std::io::stderr().write_all(s.as_bytes());
                } else {
                    let _ = std::io::stdout().write_all(s.as_bytes());
                }
            }
            Ok(js::value::from_bool(true))
        },
        payload,
    )
    .unwrap();
    let _ = obj.set_property(scope, c"write", write_fn);
    let _ = obj.set_property(scope, c"isTTY", is_tty);
    obj
}

struct NextTickTask {
    callback: RootedTraceableBox<Heap<js::object::Object>>,
    args: Vec<RootedTraceableBox<Heap<Value>>>,
}

impl Task for NextTickTask {
    fn kind(&self) -> &'static str {
        "next-tick"
    }

    fn run(self: Box<Self>, scope: &Scope<'_>, _id: TaskId) -> Result<(), ()> {
        let cb = self.callback.get(scope);
        let fval = scope.root_value(cb.as_value());
        let arg_handles: Vec<_> = self.args.iter().map(|a| a.get(scope)).collect();
        js::Function::call_value(scope, scope.global().handle(), fval, &arg_handles)
            .map(|_| ())
            .map_err(|_| ())
    }
}

#[jsnamespace(name = "process")]
pub mod process_ns {
    use super::*;

    /// `process.cwd()` — current working directory.
    pub fn cwd(_scope: &Scope<'_>, _args: &CallArgs) -> String {
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    }

    /// `process.exit(code)` — exit the process with the given code.
    pub fn exit(scope: &Scope<'_>, args: &CallArgs) -> () {
        let code = if args.argc_ > 0 {
            i32::from_jsval(scope, scope.root_value(*args.get(0)), js::conversion::ConversionBehavior::Default).unwrap_or(0)
        } else {
            0
        };
        std::process::exit(code);
    }

    /// `process.nextTick(callback[, ...args])` — queue a callback to run after
    /// the current operation and before the next event loop turn.
    pub fn next_tick(scope: &Scope<'_>, args: &CallArgs) -> Result<(), TypeError> {
        let Some(cb) = (args.argc_ > 0)
            .then(|| Object::from_value(scope, *args.get(0)).ok())
            .flatten()
            .filter(|o| o.is_callable())
        else {
            return Err(TypeError(
                "The 'callback' argument must be of type 'Function'".into(),
            ));
        };
        let extra_args: Vec<RootedTraceableBox<Heap<Value>>> = (1..args.argc_)
            .map(|i| RootedTraceableBox::new(Heap::from(*args.get(i))))
            .collect();
        with_active_event_loop(|el| el.queue_ready(Box::new(NextTickTask {
            callback: RootedTraceableBox::new(Heap::from(cb)),
            args: extra_args,
        })));
        Ok(())
    }
}

/// Install the `process` global on the provided global object.
pub fn add_to_global<'s>(scope: &'s Scope<'_>, global: Object<'s>) {
    process_ns::add_to_global(scope, global);

    let process_val = global.get_property(scope, c"process").expect("process not on global");
    let process_obj = Object::from_value(scope, process_val).expect("process is not an object");

    let _ = process_obj.set_property(scope, c"version",  NODE_VERSION.to_string());
    let _ = process_obj.set_property(scope, c"platform", platform().to_string());
    let _ = process_obj.set_property(scope, c"arch",     arch().to_string());
    let _ = process_obj.set_property(scope, c"title",    "starling".to_string());
    let _ = process_obj.set_property(scope, c"argv",     build_argv(scope));
    let _ = process_obj.set_property(scope, c"env",      build_env_object(scope));
    let _ = process_obj.set_property(scope, c"versions", build_versions_object(scope));
    let _ = process_obj.set_property(scope, c"stdin",    build_stdin_object(scope));
    let _ = process_obj.set_property(scope, c"stdout",   build_stream_object(scope, false));
    let _ = process_obj.set_property(scope, c"stderr",   build_stream_object(scope, true));
}

#[cfg(test)]
mod tests {
    use core_runtime::{runtime, test_util::eval_with_setup};

    fn eval(code: &str) -> String {
        eval_with_setup(
            || { runtime::register_global_initializer(super::add_to_global); },
            code,
        )
    }

    #[test]
    fn process_version() {
        assert_eq!(eval("process.version"), "v20.11.0");
        assert_eq!(eval("process.versions.node"), "20.11.0");
    }

    #[test]
    fn process_platform() {
        let p = eval("process.platform");
        assert!(!p.is_empty());
        assert!(
            p == "linux" || p == "darwin" || p == "win32" || p == "wasm32",
            "unexpected platform: {p}"
        );
    }

    #[test]
    fn process_arch() {
        let a = eval("process.arch");
        assert!(!a.is_empty());
        assert!(
            matches!(a.as_str(), "x64" | "arm64" | "ia32" | "arm" | "wasm32"),
            "unexpected arch: {a}"
        );
    }

    #[test]
    fn process_argv_is_array() {
        assert_eq!(eval("Array.isArray(process.argv)"), "true");
        assert_eq!(eval("typeof process.argv[0]"), "string");
    }

    #[test]
    fn process_env_is_object() {
        assert_eq!(eval("typeof process.env"), "object");
    }

    #[test]
    fn process_versions() {
        assert_eq!(eval("typeof process.versions"), "object");
        assert_eq!(eval("process.versions.starling"), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn process_cwd_returns_string() {
        assert_eq!(eval("typeof process.cwd()"), "string");
        assert!(!eval("process.cwd()").is_empty());
    }

    #[test]
    fn process_next_tick() {
        assert_eq!(eval("process.nextTick(function() {}); 'ok'"), "ok");
    }

    #[test]
    fn process_next_tick_invalid_callback() {
        use core_runtime::test_util::throws_with_setup;
        use core_runtime::runtime;
        let throws = throws_with_setup(
            || { runtime::register_global_initializer(super::add_to_global); },
            "process.nextTick('not a function')",
        );
        assert!(throws, "nextTick with non-function should throw");
    }

    #[test]
    fn process_stdin_exists() {
        assert_eq!(eval("typeof process.stdin"), "object");
        // In the test harness stdin is not a TTY.
        assert_eq!(eval("process.stdin.isTTY"), "false");
        assert_eq!(eval("typeof process.stdin.read"), "function");
    }

    #[test]
    fn process_stdout_stderr_exist() {
        assert_eq!(eval("typeof process.stdout"), "object");
        assert_eq!(eval("typeof process.stderr"), "object");
    }

    #[test]
    fn process_title() {
        assert_eq!(eval("process.title"), "starling");
    }
}
