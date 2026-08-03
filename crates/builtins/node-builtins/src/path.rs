// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Minimal `node:path` implementation.
//!
//! Implements the subset of path APIs needed by the enabled tests:
//!   resolve, join, dirname, basename, extname, parse, sep, delimiter,
//!   isAbsolute, normalize, relative, posix, win32

use core_runtime::jsmodule;
use js::conversion::{ConversionError, FromJSVal, ToJSVal};
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::native::{CallArgs, Value};
use js::Object;

#[cfg(not(windows))]
mod platform {
    pub const SEP: &str = "/";
    pub const DELIMITER: &str = ":";
}

#[cfg(windows)]
mod platform {
    pub const SEP: &str = "\\";
    pub const DELIMITER: &str = ";";
}

fn get_path_arg<'s>(scope: &'s Scope<'_>, args: &CallArgs, idx: u32) -> Result<String, ExnThrown> {
    if args.argc_ <= idx {
        return Ok(String::new());
    }
    let v = *args.get(idx);
    if !v.is_string() {
        return Err(js::error::TypeError("Path must be a string.".to_string()).throw(scope));
    }
    String::from_jsval(scope, scope.root_value(v), ())
        .map_err(|_| js::error::TypeError("Path must be a string.".to_string()).throw(scope))
}

// Extract a string from a Value, returning ExnThrown on type error.
fn val_to_str<'s>(scope: &'s Scope<'_>, v: Value) -> Result<String, ExnThrown> {
    if !v.is_string() {
        return Err(js::error::TypeError("Path must be a string.".to_string()).throw(scope));
    }
    String::from_jsval(scope, scope.root_value(v), ())
        .map_err(|_| js::error::TypeError("Path must be a string.".to_string()).throw(scope))
}

// ---------------------------------------------------------------------------
// POSIX path helpers
// ---------------------------------------------------------------------------

pub(crate) fn normalize_path(path: &str) -> String {
    let is_abs = path.starts_with('/');
    let is_trailing = path.ends_with('/') && path.len() > 1;
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                let can_pop = parts.last().map(|s| *s != "..").unwrap_or(false);
                if can_pop {
                    parts.pop();
                } else if !is_abs {
                    parts.push("..");
                }
            }
            _ => parts.push(seg),
        }
    }
    let mut result = parts.join("/");
    if is_abs {
        result = format!("/{}", result);
    }
    if result.is_empty() {
        result = if is_abs { "/".to_string() } else { ".".to_string() };
    }
    if is_trailing && !result.ends_with('/') {
        result.push('/');
    }
    result
}

fn posix_is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

fn posix_resolve_single(path: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/".to_string());
    if path.starts_with('/') {
        normalize_path(path)
    } else if path.is_empty() {
        cwd
    } else {
        normalize_path(&format!("{}/{}", cwd, path))
    }
}

fn posix_relative_impl(from: &str, to: &str) -> String {
    let from_abs = posix_resolve_single(from);
    let to_abs = posix_resolve_single(to);
    let from_parts: Vec<&str> = from_abs.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to_abs.split('/').filter(|s| !s.is_empty()).collect();
    let common_len = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut result: Vec<String> = Vec::new();
    for _ in common_len..from_parts.len() {
        result.push("..".to_string());
    }
    for s in &to_parts[common_len..] {
        result.push(s.to_string());
    }
    result.join("/")
}

// ---------------------------------------------------------------------------
// Win32 path helpers
// ---------------------------------------------------------------------------

fn is_sep(c: u8) -> bool {
    c == b'/' || c == b'\\'
}

fn win32_is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    if b.is_empty() { return false; }
    if is_sep(b[0]) { return true; }
    b.len() >= 3 && b[1] == b':' && is_sep(b[2])
}

// Parse out the "prefix" (UNC root / drive+sep / drive / rooted-sep / empty)
// Returns (prefix: String, rest_start: usize, is_absolute: bool)
fn win32_parse_prefix(path: &str) -> (String, usize, bool) {
    let b = path.as_bytes();
    if b.is_empty() {
        return (String::new(), 0, false);
    }
    // UNC: starts with \\ or //
    if b.len() >= 2 && is_sep(b[0]) && is_sep(b[1]) {
        let rest2 = &path[2..];
        // Device paths: \\?\ or \\.\  — return full path as a special prefix
        if rest2.starts_with("?\\") || rest2.starts_with("?/") || rest2.starts_with("./") || rest2.starts_with(".\\") {
            // Parse \\X\tail as prefix "\\X\" and the rest
            let marker = &rest2[..1];
            let after = &rest2[1..];
            let tail_end = after.find(|c| c == '/' || c == '\\').map_or(after.len(), |i| i + 1);
            let prefix = format!("\\\\{}{}", marker, &after[..tail_end].replace('/', "\\"));
            return (prefix, 2 + 1 + tail_end, true);
        }
        // Find server name
        let server_end = rest2.find(|c| c == '/' || c == '\\').unwrap_or(rest2.len());
        let server = &rest2[..server_end];
        if server.is_empty() {
            // \\\ or // with nothing → single rooted backslash
            return ("\\".to_string(), 1, true);
        }
        // Find share name
        let after_server = &rest2[server_end..];
        let share_str = after_server.trim_start_matches(|c| c == '/' || c == '\\');
        let skipped = after_server.len() - share_str.len();
        let share_end = share_str.find(|c| c == '/' || c == '\\').unwrap_or(share_str.len());
        let share = &share_str[..share_end];
        if share.is_empty() {
            // //server with no complete share → not a UNC path; treat as single-rooted.
            // rest_start=0 so the full path (with its leading double-sep) is re-parsed as
            // ordinary separators by win32_normalize (same as Node.js behavior).
            return ("\\".to_string(), 0, true);
        }
        let prefix = format!("\\\\{}\\{}\\", server, share);
        let after_share = 2 + server_end + skipped + share_end;
        // Skip one trailing separator
        let rest_start = if after_share < b.len() && is_sep(b[after_share]) {
            after_share + 1
        } else {
            after_share
        };
        return (prefix, rest_start, true);
    }
    // Rooted: single / or \
    if is_sep(b[0]) {
        return ("\\".to_string(), 1, true);
    }
    // Drive letter: X: ... (preserve original case)
    if b.len() >= 2 && b[1] == b':' {
        let drive_char = path.chars().next().unwrap();
        let drive = format!("{}:", drive_char);
        if b.len() >= 3 && is_sep(b[2]) {
            return (format!("{}\\", drive), 3, true);
        }
        return (drive, 2, false);
    }
    (String::new(), 0, false)
}

fn win32_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let (prefix, rest_start, is_abs) = win32_parse_prefix(path);
    let rest = &path[rest_start..];
    let trailing_sep = !rest.is_empty()
        && rest.as_bytes().last().map_or(false, |&c| is_sep(c));

    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split(|c| c == '/' || c == '\\') {
        match seg {
            "" | "." => {}
            ".." => {
                // Can we pop?
                let can_pop = parts
                    .last()
                    .map(|s| *s != "..")
                    .unwrap_or(false);
                if can_pop {
                    parts.pop();
                } else if !is_abs {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }

    let body = parts.join("\\");
    let result = if prefix.is_empty() {
        if body.is_empty() {
            if is_abs { "\\".to_string() } else { ".".to_string() }
        } else if is_abs {
            format!("\\{}", body)
        } else {
            body
        }
    } else if prefix.ends_with('\\') {
        // Absolute prefix (C:\, \\server\share\, \\.\, \\?\, \)
        if body.is_empty() { prefix.clone() } else { format!("{}{}", prefix, body) }
    } else {
        // Drive-relative prefix (C:) — join WITHOUT adding a backslash separator.
        // C: + "foo" = "C:foo", C: + "" = "C:."
        if body.is_empty() {
            format!("{}.", prefix)
        } else {
            format!("{}{}", prefix, body)
        }
    };
    // CVE-2024-36139: For relative paths (no prefix, not absolute), if the
    // normalized result starts with something that could be misinterpreted as
    // an absolute path on Windows (drive letter or ? namespace), prepend ".\".
    let result = if prefix.is_empty() && !is_abs && !result.is_empty() {
        let b0 = result.as_bytes()[0];
        let needs_dot_prefix = if result.len() >= 2 && result.as_bytes()[1] == b':' {
            b0.is_ascii_alphabetic()
        } else {
            b0 == b'?'
        };
        if needs_dot_prefix { format!(".\\{}", result) } else { result }
    } else {
        result
    };
    if trailing_sep && !result.ends_with('\\') && result != "\\" {
        format!("{}\\", result)
    } else {
        result
    }
}

fn win32_join_impl(parts: &[String]) -> String {
    if parts.is_empty() { return ".".to_string(); }
    let non_empty: Vec<&str> = parts.iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_str())
        .collect();
    if non_empty.is_empty() { return ".".to_string(); }
    // TODO: Node.js has a security guard for join: when any non-first component
    // contains a ':' in a non-drive-letter position (e.g. "CON:.."), the result
    // is returned as the literal concatenation without resolving traversals.
    // See test-path-join.js "\\fileserver\\public\\uploads" + "CON:..\..\.." case.
    let joined = non_empty.join("\\");

    // Node.js win32 join collapses leading separators to prevent normalize from
    // mistakenly treating the result as a UNC path.  If the first part itself
    // doesn't begin a valid UNC prefix (\\<non-sep>), then any run of 2+
    // leading separators in the joined string is reduced to a single backslash.
    // This mirrors the Node.js path.js logic exactly.
    let first_b = non_empty[0].as_bytes();
    let mut needs_replace = true;
    let mut slash_count: usize = 0;
    if !first_b.is_empty() && is_sep(first_b[0]) {
        slash_count += 1;
        if first_b.len() > 1 && is_sep(first_b[1]) {
            slash_count += 1;
            if first_b.len() > 2 {
                if is_sep(first_b[2]) { slash_count += 1; }
                else { needs_replace = false; } // valid UNC first part (\\server\...)
            }
        }
    }
    let joined = if needs_replace {
        let jb = joined.as_bytes();
        let mut sc = slash_count;
        while sc < jb.len() && is_sep(jb[sc]) { sc += 1; }
        if sc >= 2 { format!("\\{}", &joined[sc..]) } else { joined }
    } else {
        joined
    };

    win32_normalize(&joined)
}

fn win32_dirname(path: &str) -> String {
    if path.is_empty() { return ".".to_string(); }
    let (prefix, rest_start, _) = win32_parse_prefix(path);
    let rest = &path[rest_start..];
    let trimmed = rest.trim_end_matches(|c| c == '/' || c == '\\');
    if let Some(pos) = trimmed.rfind(|c| c == '/' || c == '\\') {
        let dir_rest = &trimmed[..pos];
        if dir_rest.is_empty() {
            let base = prefix.trim_end_matches('\\');
            if base.is_empty() { "\\".to_string() } else { format!("{}\\", base) }
        } else if prefix.is_empty() {
            dir_rest.replace('/', "\\")
        } else {
            format!("{}{}", prefix, dir_rest.replace('/', "\\"))
        }
    } else {
        if prefix.is_empty() {
            ".".to_string()
        } else {
            prefix.trim_end_matches('\\').to_string()
        }
    }
}

fn win32_basename(path: &str, suffix: &str) -> String {
    if path.is_empty() { return String::new(); }
    let (_, rest_start, _) = win32_parse_prefix(path);
    let rest = &path[rest_start..];
    let trimmed = rest.trim_end_matches(|c| c == '/' || c == '\\');
    let base = if let Some(pos) = trimmed.rfind(|c| c == '/' || c == '\\') {
        &trimmed[pos + 1..]
    } else {
        trimmed
    };
    if !suffix.is_empty() {
        if let Some(s) = base.strip_suffix(suffix) { return s.to_string(); }
    }
    base.to_string()
}

fn win32_extname(path: &str) -> String {
    let (_, rest_start, _) = win32_parse_prefix(path);
    let rest = &path[rest_start..];
    let base = if let Some(pos) = rest.rfind(|c| c == '/' || c == '\\') {
        &rest[pos + 1..]
    } else {
        rest
    };
    if let Some(dp) = base.rfind('.') {
        if dp > 0 { return base[dp..].to_string(); }
    }
    String::new()
}

fn win32_resolve_single(path: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "C:\\".to_string());
    if win32_is_absolute(path) {
        win32_normalize(path)
    } else if path.is_empty() {
        win32_normalize(&cwd)
    } else {
        win32_normalize(&format!("{}\\{}", cwd, path))
    }
}

fn win32_relative_impl(from: &str, to: &str) -> String {
    // TODO: cross-drive relative: when from and to are on different drives,
    // Node.js returns the absolute `to` path, not a relative traversal.
    let from_abs = win32_resolve_single(from);
    let to_abs = win32_resolve_single(to);
    let from_parts: Vec<&str> = from_abs
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to_abs
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();
    let common = from_parts.iter().zip(to_parts.iter())
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();
    let mut result: Vec<String> = Vec::new();
    for _ in common..from_parts.len() { result.push("..".to_string()); }
    for s in &to_parts[common..] { result.push(s.to_string()); }
    result.join("\\")
}

// ---------------------------------------------------------------------------
// Build path variant object (posix or win32)
// ---------------------------------------------------------------------------

fn build_path_variant_object<'s>(scope: &'s Scope<'_>, win32: bool) -> Result<Object<'s>, ExnThrown> {
    let obj = Object::new_plain(scope)?;
    let flag = js::value::from_bool(win32);
    let (sep_str, delim_str) = if win32 { ("\\", ";") } else { ("/", ":") };
    let _ = obj.set_property(scope, c"sep", sep_str.to_string());
    let _ = obj.set_property(scope, c"delimiter", delim_str.to_string());

    // isAbsolute
    let f = js::Function::new_callback(scope, c"isAbsolute", 1,
        |scope, args, p| {
            let path = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            Ok(js::value::from_bool(if (*p).to_boolean() { win32_is_absolute(&path) } else { posix_is_absolute(&path) }))
        }, flag)?;
    let _ = obj.set_property(scope, c"isAbsolute", f);

    // normalize
    let f = js::Function::new_callback(scope, c"normalize", 1,
        |scope, args, p| {
            let path = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let r = if (*p).to_boolean() { win32_normalize(&path) } else { normalize_path(&path) };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"normalize", f);

    // join
    let f = js::Function::new_callback(scope, c"join", 0,
        |scope, args, p| {
            let win32 = (*p).to_boolean();
            let mut parts: Vec<String> = Vec::new();
            for i in 0..args.len() {
                let s = val_to_str(scope, *args.get(i))?;
                if !s.is_empty() { parts.push(s); }
            }
            if parts.is_empty() { return ".".to_jsval_raw_throwing(scope); }
            let r = if win32 { win32_join_impl(&parts) } else { normalize_path(&parts.join("/")) };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"join", f);

    // dirname
    let f = js::Function::new_callback(scope, c"dirname", 1,
        |scope, args, p| {
            let path = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let r = if (*p).to_boolean() {
                win32_dirname(&path)
            } else {
                let trimmed = path.trim_end_matches('/');
                if trimmed == "/" || trimmed.is_empty() { "/".to_string() }
                else if let Some(pos) = trimmed.rfind('/') {
                    if pos == 0 { "/".to_string() } else { trimmed[..pos].to_string() }
                } else { ".".to_string() }
            };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"dirname", f);

    // basename
    let f = js::Function::new_callback(scope, c"basename", 1,
        |scope, args, p| {
            let path = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let suffix = if args.len() > 1 { val_to_str(scope, *args.get(1))? } else { String::new() };
            let r = if (*p).to_boolean() {
                win32_basename(&path, &suffix)
            } else {
                let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                let base = segs.last().copied().unwrap_or("");
                if !suffix.is_empty() {
                    if let Some(s) = base.strip_suffix(suffix.as_str()) { s.to_string() } else { base.to_string() }
                } else { base.to_string() }
            };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"basename", f);

    // extname
    let f = js::Function::new_callback(scope, c"extname", 1,
        |scope, args, p| {
            let path = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let r = if (*p).to_boolean() {
                win32_extname(&path)
            } else {
                let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                let base = segs.last().copied().unwrap_or("");
                if let Some(dp) = base.rfind('.') {
                    if dp > 0 { base[dp..].to_string() } else { String::new() }
                } else { String::new() }
            };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"extname", f);

    // resolve
    let f = js::Function::new_callback(scope, c"resolve", 0,
        |scope, args, p| {
            let win32 = (*p).to_boolean();
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| if win32 { "C:\\".to_string() } else { "/".to_string() });
            let mut resolved = if win32 { win32_normalize(&cwd) } else { cwd };
            for i in 0..args.len() {
                let part = val_to_str(scope, *args.get(i))?;
                if part.is_empty() { continue; }
                resolved = if win32 {
                    if win32_is_absolute(&part) { win32_normalize(&part) }
                    else { win32_normalize(&format!("{}\\{}", resolved, part)) }
                } else {
                    if part.starts_with('/') { normalize_path(&part) }
                    else { normalize_path(&format!("{}/{}", resolved, part)) }
                };
            }
            resolved.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"resolve", f);

    // relative
    let f = js::Function::new_callback(scope, c"relative", 2,
        |scope, args, p| {
            let from = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let to = if args.len() > 1 { val_to_str(scope, *args.get(1))? } else { String::new() };
            let r = if (*p).to_boolean() {
                win32_relative_impl(&from, &to)
            } else {
                posix_relative_impl(&from, &to)
            };
            r.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"relative", f);

    // parse
    let f = js::Function::new_callback(scope, c"parse", 1,
        |scope, args, p| {
            let path_str = if args.len() > 0 { val_to_str(scope, *args.get(0))? } else { String::new() };
            let win32 = (*p).to_boolean();
            let result = Object::new_plain(scope)?;
            if win32 {
                let (prefix, rest_start, _) = win32_parse_prefix(&path_str);
                let root = prefix.clone();
                let rest = &path_str[rest_start..];
                let (dir, base) = if let Some(pos) = rest.rfind(|c| c == '/' || c == '\\') {
                    let dr = &rest[..pos];
                    let dir_str = if dr.is_empty() {
                        prefix.trim_end_matches('\\').to_string()
                    } else if prefix.is_empty() {
                        dr.replace('/', "\\")
                    } else {
                        format!("{}{}", prefix, dr.replace('/', "\\"))
                    };
                    (dir_str, rest[pos + 1..].to_string())
                } else {
                    (prefix.trim_end_matches('\\').to_string(), rest.to_string())
                };
                let ext = win32_extname(&path_str);
                let name = if ext.is_empty() { base.clone() }
                    else { base.strip_suffix(ext.as_str()).unwrap_or(&base).to_string() };
                let _ = result.set_property(scope, c"root", root);
                let _ = result.set_property(scope, c"dir", dir);
                let _ = result.set_property(scope, c"base", base);
                let _ = result.set_property(scope, c"name", name);
                let _ = result.set_property(scope, c"ext", ext);
            } else {
                let root = if path_str.starts_with('/') { "/" } else { "" };
                let (dir, base) = if let Some(pos) = path_str.rfind('/') {
                    let dp = if pos == 0 { "/" } else { &path_str[..pos] };
                    (dp.to_string(), path_str[pos + 1..].to_string())
                } else {
                    (String::new(), path_str.clone())
                };
                let (name, ext) = if let Some(dp) = base.rfind('.') {
                    if dp > 0 { (base[..dp].to_string(), base[dp..].to_string()) }
                    else { (base.clone(), String::new()) }
                } else { (base.clone(), String::new()) };
                let _ = result.set_property(scope, c"root", root.to_string());
                let _ = result.set_property(scope, c"dir", dir);
                let _ = result.set_property(scope, c"base", base);
                let _ = result.set_property(scope, c"name", name);
                let _ = result.set_property(scope, c"ext", ext);
            }
            Ok(result.as_value())
        }, flag)?;
    let _ = obj.set_property(scope, c"parse", f);

    // format
    let f = js::Function::new_callback(scope, c"format", 1,
        |scope, args, p| {
            let win32 = (*p).to_boolean();
            let sep = if win32 { '\\' } else { '/' };
            if args.len() == 0 { return "".to_jsval_raw_throwing(scope); }
            let v = *args.get(0);
            if !v.is_object() { return "".to_jsval_raw_throwing(scope); }
            let arg_obj = Object::from_value(scope, scope.root_value(v))
                .map_err(|_| ExnThrown)?;
            let get_str = |prop: &std::ffi::CStr| -> String {
                arg_obj.get_property(scope, prop)
                    .ok()
                    .and_then(|v| String::from_jsval(scope, v, ()).ok())
                    .unwrap_or_default()
            };
            let dir = get_str(c"dir");
            let root = get_str(c"root");
            let base = get_str(c"base");
            let name = get_str(c"name");
            let ext = get_str(c"ext");
            let filename = if !base.is_empty() { base } else { format!("{}{}", name, ext) };
            let result = if !dir.is_empty() {
                let last = dir.as_bytes().last().copied().unwrap_or(0);
                if is_sep(last) { format!("{}{}", dir, filename) }
                else { format!("{}{}{}", dir, sep, filename) }
            } else if !root.is_empty() {
                let last = root.as_bytes().last().copied().unwrap_or(0);
                if is_sep(last) { format!("{}{}", root, filename) }
                else { format!("{}{}{}", root, sep, filename) }
            } else {
                filename
            };
            result.to_jsval_raw_throwing(scope)
        }, flag)?;
    let _ = obj.set_property(scope, c"format", f);

    Ok(obj)
}

// Marker types for posix/win32 exported from the module
pub struct PathPosixValue;
impl<'s> ToJSVal<'s> for PathPosixValue {
    fn to_jsval_raw(&self, scope: &'s Scope<'_>) -> Result<js::native::Value, ConversionError> {
        build_path_variant_object(scope, false)
            .map(|o| o.as_value())
            .map_err(|_| ConversionError::ExnPending)
    }
}

pub struct PathWin32Value;
impl<'s> ToJSVal<'s> for PathWin32Value {
    fn to_jsval_raw(&self, scope: &'s Scope<'_>) -> Result<js::native::Value, ConversionError> {
        build_path_variant_object(scope, true)
            .map(|o| o.as_value())
            .map_err(|_| ConversionError::ExnPending)
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[jsmodule(name = "node:path")]
pub mod node_path {
    use super::*;

    pub fn resolve(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| String::from("/"));
        let mut parts: Vec<String> = Vec::new();
        for i in 0..args.argc_ {
            let p = get_path_arg(scope, args, i)?;
            if p.is_empty() { continue; }
            if p.starts_with('/') { parts.clear(); }
            for seg in p.split('/') {
                if !seg.is_empty() { parts.push(seg.to_string()); }
            }
        }
        if parts.is_empty() { return Ok(cwd); }
        Ok(format!("/{}", parts.join("/")))
    }

    pub fn join(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        if args.argc_ == 0 { return Ok(".".to_string()); }
        let mut segments: Vec<String> = Vec::new();
        for i in 0..args.argc_ {
            let p = get_path_arg(scope, args, i)?;
            if !p.is_empty() { segments.push(p); }
        }
        if segments.is_empty() { return Ok(".".to_string()); }
        Ok(normalize_path(&segments.join("/")))
    }

    pub fn dirname(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        let trimmed = path.trim_end_matches('/');
        if trimmed == "/" || trimmed.is_empty() { return Ok("/".to_string()); }
        if let Some(pos) = trimmed.rfind('/') {
            if pos == 0 { Ok("/".to_string()) } else { Ok(trimmed[..pos].to_string()) }
        } else { Ok(".".to_string()) }
    }

    pub fn basename(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        let suffix = if args.argc_ > 1 { get_path_arg(scope, args, 1)? } else { String::new() };
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let base = segments.last().copied().unwrap_or("");
        if !suffix.is_empty() {
            if let Some(stripped) = base.strip_suffix(&suffix) { return Ok(stripped.to_string()); }
        }
        Ok(base.to_string())
    }

    pub fn extname(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let base = segments.last().copied().unwrap_or("");
        if let Some(dot_pos) = base.rfind('.') {
            if dot_pos > 0 { return Ok(base[dot_pos..].to_string()); }
        }
        Ok(String::new())
    }

    pub fn is_absolute(scope: &Scope<'_>, args: &CallArgs) -> Result<bool, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        Ok(path.starts_with('/'))
    }

    pub fn normalize(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        Ok(normalize_path(&path))
    }

    pub fn relative(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let from_path = get_path_arg(scope, args, 0)?;
        let to_path = get_path_arg(scope, args, 1)?;
        Ok(posix_relative_impl(&from_path, &to_path))
    }

    pub fn parse(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path = get_path_arg(scope, args, 0)?;
        let obj = js::Object::new_plain(scope).unwrap();
        let root = if path.starts_with('/') { "/" } else { "" };
        let (dir, base) = if let Some(pos) = path.rfind('/') {
            let dir_part = if pos == 0 { "/" } else { &path[..pos] };
            (dir_part.to_string(), path[pos + 1..].to_string())
        } else {
            (String::new(), path.clone())
        };
        let (name, ext) = if let Some(dot_pos) = base.rfind('.') {
            if dot_pos > 0 { (base[..dot_pos].to_string(), base[dot_pos..].to_string()) }
            else { (base.clone(), String::new()) }
        } else { (base.clone(), String::new()) };
        let _ = obj.set_property(scope, c"root", root.to_string());
        let _ = obj.set_property(scope, c"dir", dir);
        let _ = obj.set_property(scope, c"base", base);
        let _ = obj.set_property(scope, c"name", name);
        let _ = obj.set_property(scope, c"ext", ext);
        Ok(obj.as_value())
    }

    #[allow(non_upper_case_globals)]
    pub const sep: &str = platform::SEP;
    #[allow(non_upper_case_globals)]
    pub const delimiter: &str = platform::DELIMITER;
    #[allow(non_upper_case_globals)]
    pub const posix: PathPosixValue = PathPosixValue;
    #[allow(non_upper_case_globals)]
    pub const win32: PathWin32Value = PathWin32Value;
}

pub fn register(scope: &js::gc::scope::Scope<'_>) {
    unsafe {
        node_path::register(scope);
    }
}
