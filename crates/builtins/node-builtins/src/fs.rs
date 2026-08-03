// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Minimal `node:fs` implementation.
//!
//! Implements the subset of fs APIs needed by the enabled tests:
//!   existsSync, readFileSync, readdirSync, stat/statSync, lstat/lstatSync,
//!   access/accessSync, constants, promises (stub), mkdir/mkdirSync,
//!   rm/rmSync, rmdir/rmdirSync, writeFile/writeFileSync, appendFileSync,
//!   mkdtempSync, renameSync, unlinkSync, copyFileSync/copyFile,
//!   symlinkSync, realpathSync, truncateSync, chmodSync, readdir, readFile,
//!   rename, unlink, chmod, realpath, symlink

use core_runtime::jsmodule;
use js::conversion::{ConversionError, FromJSVal, ToJSVal};
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::native::{CallArgs, Value};
use js::Object;
use std::path::Path;

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn get_path<'s>(scope: &'s Scope<'_>, args: &CallArgs, idx: u32) -> Result<String, ExnThrown> {
    if args.argc_ <= idx {
        return Err(js::error::TypeError(
            "The \"path\" argument must be of type string.".to_string(),
        )
        .throw(scope));
    }
    let v = *args.get(idx);
    if !v.is_string() {
        return Err(js::error::TypeError(
            "The \"path\" argument must be of type string.".to_string(),
        )
        .throw(scope));
    }
    String::from_jsval(scope, scope.root_value(v), ()).map_err(|_| {
        js::error::TypeError("The \"path\" argument must be of type string.".to_string())
            .throw(scope)
    })
}

// ---------------------------------------------------------------------------
// Error object creation
// ---------------------------------------------------------------------------

fn io_error_code(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::NotADirectory => "ENOTDIR",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        std::io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        _ => "ERR_FS_SYSTEM_ERROR",
    }
}

fn io_error_description(code: &str) -> &'static str {
    match code {
        "ENOENT" => "no such file or directory",
        "EEXIST" => "file already exists",
        "EACCES" => "permission denied",
        "EPERM" => "operation not permitted",
        "ENOTDIR" => "not a directory",
        "EISDIR" => "illegal operation on a directory",
        "ENOTEMPTY" => "directory not empty",
        _ => "system error",
    }
}

fn make_fs_error<'s>(
    scope: &'s Scope<'s>,
    code: &str,
    message: &str,
    path: &str,
    syscall: &str,
) -> Object<'s> {
    let error_ctor =
        js::class::get_class_object(scope, js::class_spec::JSProtoKey::JSProto_Error).unwrap();
    let ctor_val = scope.root_value(unsafe { js::value::from_object(error_ctor.get()) });
    let msg_val = scope.root_value(message.to_jsval_raw(scope).unwrap());
    let err_obj = js::Function::construct(scope, ctor_val, &[msg_val]).unwrap();
    let _ = err_obj.set_property(scope, c"code", code.to_string());
    let _ = err_obj.set_property(scope, c"path", path.to_string());
    let _ = err_obj.set_property(scope, c"syscall", syscall.to_string());
    err_obj
}

/// Create a Node.js-style fs error with properly formatted message.
fn make_io_error<'s>(
    scope: &'s Scope<'s>,
    e: &std::io::Error,
    path: &str,
    syscall: &str,
) -> Object<'s> {
    let code = io_error_code(e);
    let desc = io_error_description(code);
    let message = format!("{}: {}, {} '{}'", code, desc, syscall, path);
    make_fs_error(scope, code, &message, path, syscall)
}

/// Call a callback with (null, value) on success or (error) on failure.
fn call_cb_result<'s, T, F>(
    scope: &'s Scope<'_>,
    cb: Value,
    result: std::io::Result<T>,
    path: &str,
    syscall: &str,
    make_value: F,
) -> Result<(), js::error::ExnThrown>
where
    F: FnOnce(&'s Scope<'_>, T) -> Value,
{
    let cb_rooted = scope.root_value(cb);
    match result {
        Ok(v) => {
            let null = scope.root_value(js::value::null());
            let val = scope.root_value(make_value(scope, v));
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[null, val])?;
        }
        Err(e) => {
            let err = scope.root_value(make_io_error(scope, &e, path, syscall).as_value());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[err])?;
        }
    }
    Ok(())
}

/// Call a callback with (null) on success or (error) on failure (no result value).
fn call_cb_void<'s>(
    scope: &'s Scope<'_>,
    cb: Value,
    result: std::io::Result<()>,
    path: &str,
    syscall: &str,
) -> Result<(), js::error::ExnThrown> {
    let cb_rooted = scope.root_value(cb);
    match result {
        Ok(()) => {
            let null = scope.root_value(js::value::null());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[null])?;
        }
        Err(e) => {
            let err = scope.root_value(make_io_error(scope, &e, path, syscall).as_value());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[err])?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// File content encoding
// ---------------------------------------------------------------------------

fn encode_file_content<'s>(scope: &'s Scope<'s>, bytes: &[u8], encoding: Option<&str>) -> Value {
    let enc = encoding.map(|s| s.to_lowercase());
    match enc.as_deref() {
        Some("utf8") | Some("utf-8") => {
            let s = String::from_utf8_lossy(bytes);
            String::to_jsval_raw(&s.to_string(), scope).unwrap()
        }
        Some("hex") => hex::encode(bytes).to_jsval_raw(scope).unwrap(),
        Some("base64") => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .encode(bytes)
                .to_jsval_raw(scope)
                .unwrap()
        }
        Some("latin1") | Some("binary") | Some("ascii") => {
            let s: String = bytes.iter().map(|b| *b as char).collect();
            s.to_jsval_raw(scope).unwrap()
        }
        Some("utf16le") | Some("utf-16le") => {
            let s = String::from_utf16_lossy(
                &bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect::<Vec<u16>>(),
            );
            s.to_jsval_raw(scope).unwrap()
        }
        _ => {
            let s: String = bytes.iter().map(|b| *b as char).collect();
            s.to_jsval_raw(scope).unwrap()
        }
    }
}

fn data_to_bytes(data_val: Value, scope: &Scope<'_>) -> Vec<u8> {
    if data_val.is_string() {
        String::from_jsval(scope, scope.root_value(data_val), ())
            .unwrap_or_default()
            .into_bytes()
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Option parsing
// ---------------------------------------------------------------------------

/// Parse (encoding, flag) from args[opt_idx] which can be a string or object.
fn parse_opts_at(scope: &Scope<'_>, args: &CallArgs, opt_idx: u32) -> (Option<String>, Option<String>) {
    if args.argc_ <= opt_idx {
        return (None, None);
    }
    let opt = *args.get(opt_idx);
    if opt.is_string() {
        let enc = String::from_jsval(scope, scope.root_value(opt), ())
            .ok()
            .map(|s| s.to_lowercase());
        return (enc, None);
    }
    if opt.is_object() {
        if let Ok(obj) = js::Object::from_value(scope, opt) {
            let enc = match obj.get_property(scope, c"encoding") {
                Ok(v) if v.is_string() => {
                    String::from_jsval(scope, scope.root_value(*v), ()).ok().map(|s| s.to_lowercase())
                }
                Ok(_) => None,
                Err(_) => {
                    let _ = js::exception::take_pending(scope);
                    None
                }
            };
            let flag = match obj.get_property(scope, c"flag") {
                Ok(v) if v.is_string() => {
                    String::from_jsval(scope, scope.root_value(*v), ()).ok()
                }
                Ok(_) => None,
                Err(_) => {
                    let _ = js::exception::take_pending(scope);
                    None
                }
            };
            return (enc, flag);
        }
    }
    (None, None)
}

/// Validate that encoding is a known value; throw ERR_INVALID_ARG_VALUE if not.
fn validate_encoding(scope: &Scope<'_>, enc: &str) -> Result<(), js::error::ExnThrown> {
    match enc {
        "utf8" | "utf-8" | "hex" | "base64" | "latin1" | "binary" | "ascii"
        | "utf16le" | "utf-16le" => Ok(()),
        _ => Err(js::error::TypeError(
            format!("The argument 'encoding' is invalid. Received '{}'", enc),
        ).throw(scope)),
    }
}

/// Get a callback argument, throwing ERR_INVALID_ARG_TYPE if it is not callable.
fn get_callback(scope: &Scope<'_>, args: &CallArgs, idx: u32) -> Result<Value, js::error::ExnThrown> {
    if args.argc_ <= idx {
        return Err(js::error::TypeError(
            "The \"callback\" argument must be of type function. Received undefined".to_string(),
        ).throw(scope));
    }
    let v = *args.get(idx);
    let is_fn = js::Object::from_value(scope, v)
        .map(|o| o.is_callable())
        .unwrap_or(false);
    if !is_fn {
        return Err(js::error::TypeError(
            "The \"callback\" argument must be of type function.".to_string(),
        ).throw(scope));
    }
    Ok(v)
}

fn parse_read_opts(scope: &Scope<'_>, args: &CallArgs) -> (Option<String>, Option<String>) {
    parse_opts_at(scope, args, 1)
}

fn read_with_flag(path: &str, flag: &str) -> std::io::Result<Vec<u8>> {
    match flag {
        "a+" => {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .open(path)?;
            file.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        }
        _ => std::fs::read(path),
    }
}

// ---------------------------------------------------------------------------
// Stats object
// ---------------------------------------------------------------------------

/// Build a Node.js Stats-like object with isFile(), isDirectory(), etc. methods.
fn build_stats_object<'s>(scope: &'s Scope<'_>, meta: &std::fs::Metadata) -> Result<Object<'s>, ExnThrown> {
    #[cfg(unix)]
    let (mode, nlink, uid, gid, ino, dev, rdev, blksize, blocks) = {
        use std::os::unix::fs::MetadataExt;
        (
            meta.mode() as i64, meta.nlink() as i64, meta.uid() as i64,
            meta.gid() as i64, meta.ino() as i64, meta.dev() as i64,
            meta.rdev() as i64, meta.blksize() as i64, meta.blocks() as i64,
        )
    };
    #[cfg(not(unix))]
    let (mode, nlink, uid, gid, ino, dev, rdev, blksize, blocks): (i64, i64, i64, i64, i64, i64, i64, i64, i64) = {
        let m = if meta.is_dir() { 0o040755 } else { 0o100644 };
        (m, 1, 0, 0, 0, 0, 0, 4096, 0)
    };

    let to_ms = |t: std::io::Result<std::time::SystemTime>| -> i64 {
        t.ok()
            .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    };

    let size = meta.len() as i64;
    let mtime_ms = to_ms(meta.modified());
    let atime_ms = to_ms(meta.accessed());
    let ctime_ms = to_ms(meta.created());
    let is_file = meta.is_file();
    let is_dir = meta.is_dir();
    let is_sym = meta.file_type().is_symlink();

    let obj = Object::new_plain(scope)?;
    let _ = obj.set_property(scope, c"dev", dev);
    let _ = obj.set_property(scope, c"ino", ino);
    let _ = obj.set_property(scope, c"mode", mode);
    let _ = obj.set_property(scope, c"nlink", nlink);
    let _ = obj.set_property(scope, c"uid", uid);
    let _ = obj.set_property(scope, c"gid", gid);
    let _ = obj.set_property(scope, c"rdev", rdev);
    let _ = obj.set_property(scope, c"size", size);
    let _ = obj.set_property(scope, c"blksize", blksize);
    let _ = obj.set_property(scope, c"blocks", blocks);
    let _ = obj.set_property(scope, c"atimeMs", atime_ms);
    let _ = obj.set_property(scope, c"mtimeMs", mtime_ms);
    let _ = obj.set_property(scope, c"ctimeMs", ctime_ms);
    let _ = obj.set_property(scope, c"birthtimeMs", ctime_ms);

    // Add method functions using bool payloads
    macro_rules! bool_method {
        ($name:expr, $val:expr) => {{
            let p = js::value::from_bool($val);
            let f = js::Function::new_callback(scope, $name, 0, |_, _, p| Ok(*p), p)?;
            let _ = obj.set_property(scope, $name, f);
        }};
    }
    bool_method!(c"isFile", is_file);
    bool_method!(c"isDirectory", is_dir);
    bool_method!(c"isSymbolicLink", is_sym);
    bool_method!(c"isBlockDevice", false);
    bool_method!(c"isCharacterDevice", false);
    bool_method!(c"isFIFO", false);
    bool_method!(c"isSocket", false);

    Ok(obj)
}

// ---------------------------------------------------------------------------
// null-prototype object (for fs.constants)
// ---------------------------------------------------------------------------

fn build_null_proto_object<'s>(scope: &'s Scope<'_>) -> Result<Object<'s>, ExnThrown> {
    let object_prop = scope.global().get_property(scope, c"Object")?;
    let object_obj = js::Object::from_value(scope, object_prop).map_err(|_| ExnThrown)?;
    let null_val = scope.root_value(js::value::null());
    let result =
        js::Function::call_by_name(scope, object_obj.handle(), c"create", &[null_val])?;
    js::Object::from_value(scope, result).map_err(|_| ExnThrown)
}

fn build_fs_constants<'s>(scope: &'s Scope<'_>) -> Result<Object<'s>, ExnThrown> {
    let obj = build_null_proto_object(scope)?;
    let _ = obj.set_property(scope, c"UV_FS_SYMLINK_DIR", 1i64);
    let _ = obj.set_property(scope, c"UV_FS_SYMLINK_JUNCTION", 2i64);
    let _ = obj.set_property(scope, c"O_RDONLY", 0i64);
    let _ = obj.set_property(scope, c"O_WRONLY", 1i64);
    let _ = obj.set_property(scope, c"O_RDWR", 2i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_UNKNOWN", 0i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_FILE", 1i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_DIR", 2i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_LINK", 3i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_FIFO", 4i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_SOCKET", 5i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_CHAR", 6i64);
    let _ = obj.set_property(scope, c"UV_DIRENT_BLOCK", 7i64);
    let _ = obj.set_property(scope, c"S_IFMT", 0o170000i64);
    let _ = obj.set_property(scope, c"S_IFREG", 0o100000i64);
    let _ = obj.set_property(scope, c"S_IFDIR", 0o040000i64);
    let _ = obj.set_property(scope, c"S_IFCHR", 0o020000i64);
    let _ = obj.set_property(scope, c"S_IFBLK", 0o060000i64);
    let _ = obj.set_property(scope, c"S_IFIFO", 0o010000i64);
    let _ = obj.set_property(scope, c"S_IFLNK", 0o120000i64);
    let _ = obj.set_property(scope, c"S_IFSOCK", 0o140000i64);
    let _ = obj.set_property(scope, c"O_CREAT", 64i64);
    let _ = obj.set_property(scope, c"O_EXCL", 128i64);
    let _ = obj.set_property(scope, c"UV_FS_O_FILEMAP", 0i64);
    let _ = obj.set_property(scope, c"O_NOCTTY", 256i64);
    let _ = obj.set_property(scope, c"O_TRUNC", 512i64);
    let _ = obj.set_property(scope, c"O_APPEND", 1024i64);
    let _ = obj.set_property(scope, c"O_DIRECTORY", 65536i64);
    let _ = obj.set_property(scope, c"O_NOATIME", 262144i64);
    let _ = obj.set_property(scope, c"O_NOFOLLOW", 131072i64);
    let _ = obj.set_property(scope, c"O_SYNC", 1052672i64);
    let _ = obj.set_property(scope, c"O_DSYNC", 4096i64);
    let _ = obj.set_property(scope, c"O_SYMLINK", 0i64);
    let _ = obj.set_property(scope, c"O_DIRECT", 16384i64);
    let _ = obj.set_property(scope, c"O_NONBLOCK", 2048i64);
    let _ = obj.set_property(scope, c"S_IRWXU", 0o700i64);
    let _ = obj.set_property(scope, c"S_IRUSR", 0o400i64);
    let _ = obj.set_property(scope, c"S_IWUSR", 0o200i64);
    let _ = obj.set_property(scope, c"S_IXUSR", 0o100i64);
    let _ = obj.set_property(scope, c"S_IRWXG", 0o070i64);
    let _ = obj.set_property(scope, c"S_IRGRP", 0o040i64);
    let _ = obj.set_property(scope, c"S_IWGRP", 0o020i64);
    let _ = obj.set_property(scope, c"S_IXGRP", 0o010i64);
    let _ = obj.set_property(scope, c"S_IRWXO", 0o007i64);
    let _ = obj.set_property(scope, c"S_IROTH", 0o004i64);
    let _ = obj.set_property(scope, c"S_IWOTH", 0o002i64);
    let _ = obj.set_property(scope, c"S_IXOTH", 0o001i64);
    let _ = obj.set_property(scope, c"F_OK", 0i64);
    let _ = obj.set_property(scope, c"R_OK", 4i64);
    let _ = obj.set_property(scope, c"W_OK", 2i64);
    let _ = obj.set_property(scope, c"X_OK", 1i64);
    let _ = obj.set_property(scope, c"UV_FS_COPYFILE_EXCL", 1i64);
    let _ = obj.set_property(scope, c"COPYFILE_EXCL", 1i64);
    let _ = obj.set_property(scope, c"UV_FS_COPYFILE_FICLONE", 2i64);
    let _ = obj.set_property(scope, c"COPYFILE_FICLONE", 2i64);
    let _ = obj.set_property(scope, c"UV_FS_COPYFILE_FICLONE_FORCE", 4i64);
    let _ = obj.set_property(scope, c"COPYFILE_FICLONE_FORCE", 4i64);
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Module export value types
// ---------------------------------------------------------------------------

pub struct FsConstantsValue;

impl<'s> ToJSVal<'s> for FsConstantsValue {
    fn to_jsval_raw(
        &self,
        scope: &'s Scope<'_>,
    ) -> Result<js::native::Value, ConversionError> {
        build_fs_constants(scope)
            .map(|obj| obj.as_value())
            .map_err(|_| ConversionError::ExnPending)
    }
}

pub struct FsPromisesValue;

impl<'s> ToJSVal<'s> for FsPromisesValue {
    fn to_jsval_raw(
        &self,
        scope: &'s Scope<'_>,
    ) -> Result<js::native::Value, ConversionError> {
        js::Object::new_plain(scope)
            .map(|obj| obj.as_value())
            .map_err(|_| ConversionError::ExnPending)
    }
}

// ---------------------------------------------------------------------------
// node:fs module
// ---------------------------------------------------------------------------

#[jsmodule(name = "node:fs")]
pub mod node_fs {
    use super::*;

    // ---- existsSync -------------------------------------------------------

    pub fn exists_sync(scope: &Scope<'_>, args: &CallArgs) -> bool {
        match get_path(scope, args, 0) {
            Ok(p) => Path::new(&p).exists(),
            Err(_) => false,
        }
    }

    // ---- readFileSync / readFile ------------------------------------------

    pub fn read_file_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let (encoding, flag) = parse_read_opts(scope, args);
        let bytes = read_with_flag(&path_str, flag.as_deref().unwrap_or("r")).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "read");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(encode_file_content(scope, &bytes, encoding.as_deref()))
    }

    pub fn read_file(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        // Options at [1], callback at [2]; or callback at [1] if no options.
        let (enc_idx, cb_idx) = if args.argc_ >= 3 { (1u32, 2u32) } else { (1u32, 1u32) };
        let (encoding, flag) = parse_opts_at(scope, args, enc_idx);
        if let Some(ref enc) = encoding {
            validate_encoding(scope, enc)?;
        }
        let cb = get_callback(scope, args, cb_idx)?;
        let result = read_with_flag(&path_str, flag.as_deref().unwrap_or("r"));
        call_cb_result(scope, cb, result, &path_str, "read", |scope, bytes| {
            encode_file_content(scope, &bytes, encoding.as_deref())
        })?;
        Ok(())
    }

    // ---- writeFileSync / writeFile ----------------------------------------

    pub fn write_file_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let data_val = if args.argc_ > 1 { *args.get(1) } else { js::value::undefined() };
        let bytes = data_to_bytes(data_val, scope);
        // Parse mode and flag from options at [2]
        let (_, flag) = parse_opts_at(scope, args, 2);
        let flag_str = flag.as_deref().unwrap_or("w");
        let open_res = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(!flag_str.contains('a'))
            .append(flag_str.contains('a'))
            .open(&path_str);
        let mut file = open_res.map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "open");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "write");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    pub fn write_file(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let data_val = if args.argc_ > 1 { *args.get(1) } else { js::value::undefined() };
        let (opts_idx, cb_idx) = if args.argc_ >= 4 { (2u32, 3u32) } else { (2u32, 2u32) };
        let (_, flag) = parse_opts_at(scope, args, opts_idx);
        let flag_str = flag.unwrap_or_else(|| "w".to_string());
        let bytes = data_to_bytes(data_val, scope);
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(!flag_str.contains('a'))
            .append(flag_str.contains('a'))
            .open(&path_str)
            .and_then(|mut f| { use std::io::Write; f.write_all(&bytes) });
        call_cb_void(scope, cb, result, &path_str, "write")?;
        Ok(())
    }

    // ---- appendFileSync ---------------------------------------------------

    pub fn append_file_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let data_val = if args.argc_ > 1 { *args.get(1) } else { js::value::undefined() };
        let bytes = data_to_bytes(data_val, scope);
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(&path_str)
            .map_err(|e| {
                let _ = make_io_error(scope, &e, &path_str, "open");
                js::error::TypeError(e.to_string()).throw(scope)
            })?;
        file.write_all(&bytes).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "write");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    // ---- readdirSync / readdir -------------------------------------------

    pub fn readdir_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let p = Path::new(&path_str);
        if !p.is_dir() {
            let e = std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory");
            let _ = make_io_error(scope, &e, &path_str, "getdents");
            return Err(js::error::TypeError(format!(
                "ENOTDIR: not a directory, '{}'",
                path_str
            ))
            .throw(scope));
        }
        let entries: Vec<String> = std::fs::read_dir(&path_str)
            .map_err(|e| {
                let _ = make_io_error(scope, &e, &path_str, "getdents");
                js::error::TypeError(e.to_string()).throw(scope)
            })?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let arr = js::Array::new(scope, entries.len()).unwrap();
        for (i, entry) in entries.into_iter().enumerate() {
            let val = entry.to_jsval_raw(scope).unwrap();
            let _ = arr.set_element(scope, i as u32, scope.root_value(val));
        }
        Ok(arr.as_value())
    }

    pub fn readdir(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::read_dir(&path_str).map(|iter| {
            iter.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>()
        });
        call_cb_result(scope, cb, result, &path_str, "getdents", |scope, entries| {
            let arr = js::Array::new(scope, entries.len()).unwrap();
            for (i, entry) in entries.into_iter().enumerate() {
                let val = entry.to_jsval_raw(scope).unwrap();
                let _ = arr.set_element(scope, i as u32, scope.root_value(val));
            }
            arr.as_value()
        })?;
        Ok(())
    }

    // ---- statSync / stat / lstatSync / lstat -----------------------------

    pub fn stat_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let metadata = std::fs::metadata(&path_str).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "stat");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(build_stats_object(scope, &metadata)?.as_value())
    }

    pub fn stat(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::metadata(&path_str);
        call_cb_result(scope, cb, result, &path_str, "stat", |scope, meta| {
            build_stats_object(scope, &meta).map(|o| o.as_value()).unwrap_or(js::value::undefined())
        })?;
        Ok(())
    }

    pub fn lstat_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let metadata = std::fs::symlink_metadata(&path_str).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "lstat");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(build_stats_object(scope, &metadata)?.as_value())
    }

    pub fn lstat(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::symlink_metadata(&path_str);
        call_cb_result(scope, cb, result, &path_str, "lstat", |scope, meta| {
            build_stats_object(scope, &meta).map(|o| o.as_value()).unwrap_or(js::value::undefined())
        })?;
        Ok(())
    }

    // ---- accessSync / access ---------------------------------------------

    pub fn access_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        if !Path::new(&path_str).exists() {
            let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
            let _ = make_io_error(scope, &e, &path_str, "access");
            return Err(js::error::TypeError(format!(
                "ENOENT: no such file or directory, access '{}'",
                path_str
            ))
            .throw(scope));
        }
        Ok(())
    }

    pub fn access(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb_val = *args.get(cb_idx);
        let cb = scope.root_value(cb_val);
        let exists = Path::new(&path_str).exists();
        let err_arg = if exists {
            scope.root_value(js::value::null())
        } else {
            let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
            let err = make_io_error(scope, &e, &path_str, "access");
            scope.root_value(err.as_value())
        };
        js::Function::call_value(scope, scope.global().handle(), cb, &[err_arg])?;
        Ok(())
    }

    // ---- mkdirSync / mkdir -----------------------------------------------

    pub fn mkdir_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<Value, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let recursive = if args.argc_ > 1 {
            if let Ok(obj) = js::Object::from_value(scope, *args.get(1)) {
                obj.get_property(scope, c"recursive")
                    .map(|v| v.to_boolean())
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };
        let p = Path::new(&path_str);
        let result = if recursive {
            std::fs::create_dir_all(p)
        } else {
            std::fs::create_dir(p)
        };
        result.map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "mkdir");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(js::value::undefined())
    }

    pub fn mkdir(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        // mkdir(path, callback) or mkdir(path, mode_or_options, callback)
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let recursive = if args.argc_ >= 3 {
            if let Ok(obj) = js::Object::from_value(scope, *args.get(1)) {
                obj.get_property(scope, c"recursive")
                    .map(|v| v.to_boolean())
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };
        let result = if recursive {
            std::fs::create_dir_all(&path_str)
        } else {
            std::fs::create_dir(&path_str)
        };
        call_cb_void(scope, cb, result, &path_str, "mkdir")?;
        Ok(())
    }

    // ---- rmdirSync / rmdir -----------------------------------------------

    pub fn rmdir_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let p = Path::new(&path_str);
        if !p.exists() {
            let e = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
            let _ = make_io_error(scope, &e, &path_str, "rmdir");
            return Err(js::error::TypeError(format!(
                "ENOENT: no such file or directory, rmdir '{}'",
                path_str
            ))
            .throw(scope));
        }
        std::fs::remove_dir(p).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "rmdir");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    pub fn rmdir(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        // If dir doesn't exist, report ENOENT
        let result = if !Path::new(&path_str).exists() {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"))
        } else {
            std::fs::remove_dir(&path_str)
        };
        call_cb_void(scope, cb, result, &path_str, "rmdir")?;
        Ok(())
    }

    // ---- rmSync / rm -----------------------------------------------------

    pub fn rm_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let p = Path::new(&path_str);
        let (recursive, force) = if args.argc_ > 1 {
            if let Ok(obj) = js::Object::from_value(scope, *args.get(1)) {
                let rec = obj
                    .get_property(scope, c"recursive")
                    .map(|v| v.to_boolean())
                    .unwrap_or(false);
                let frc = obj
                    .get_property(scope, c"force")
                    .map(|v| v.to_boolean())
                    .unwrap_or(false);
                (rec, frc)
            } else {
                (false, false)
            }
        } else {
            (false, false)
        };
        let result = if let Ok(m) = std::fs::symlink_metadata(p) {
            if m.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_dir(p)
                }
            } else {
                std::fs::remove_file(p)
            }
        } else if force {
            Ok(())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
        };
        if let Err(e) = result {
            if !force {
                let _ = make_io_error(scope, &e, &path_str, "unlink");
                return Err(js::error::TypeError(e.to_string()).throw(scope));
            }
        }
        Ok(())
    }

    pub fn rm(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let (recursive, force) = if args.argc_ >= 3 {
            if let Ok(obj) = js::Object::from_value(scope, *args.get(1)) {
                let rec = obj.get_property(scope, c"recursive").map(|v| v.to_boolean()).unwrap_or(false);
                let frc = obj.get_property(scope, c"force").map(|v| v.to_boolean()).unwrap_or(false);
                (rec, frc)
            } else { (false, false) }
        } else { (false, false) };
        let p = Path::new(&path_str);
        let result = if let Ok(m) = std::fs::symlink_metadata(p) {
            if m.is_dir() {
                if recursive { std::fs::remove_dir_all(p) } else { std::fs::remove_dir(p) }
            } else { std::fs::remove_file(p) }
        } else if force { Ok(()) }
        else { Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found")) };
        if force {
            // In force mode, call callback with null regardless
            let cb_rooted = scope.root_value(cb);
            let null = scope.root_value(js::value::null());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[null])?;
        } else {
            call_cb_void(scope, cb, result, &path_str, "unlink")?;
        }
        Ok(())
    }

    // ---- renameSync / rename ----------------------------------------------

    pub fn rename_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let old_path = get_path(scope, args, 0)?;
        let new_path = get_path(scope, args, 1)?;
        std::fs::rename(&old_path, &new_path).map_err(|e| {
            let _ = make_io_error(scope, &e, &old_path, "rename");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    pub fn rename(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let old_path = get_path(scope, args, 0)?;
        let new_path = get_path(scope, args, 1)?;
        if args.argc_ < 3 { return Ok(()); }
        let cb = *args.get(2);
        let result = std::fs::rename(&old_path, &new_path);
        call_cb_void(scope, cb, result, &old_path, "rename")?;
        Ok(())
    }

    // ---- unlinkSync / unlink ---------------------------------------------

    pub fn unlink_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        std::fs::remove_file(&path_str).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "unlink");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    pub fn unlink(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        if args.argc_ < 2 { return Ok(()); }
        let cb = *args.get(1);
        let result = std::fs::remove_file(&path_str);
        call_cb_void(scope, cb, result, &path_str, "unlink")?;
        Ok(())
    }

    // ---- copyFileSync / copyFile -----------------------------------------

    pub fn copy_file_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let src = get_path(scope, args, 0)?;
        let dest = get_path(scope, args, 1)?;
        std::fs::copy(&src, &dest).map_err(|e| {
            let _ = make_io_error(scope, &e, &src, "copyfile");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    pub fn copy_file(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let src = get_path(scope, args, 0)?;
        let dest = get_path(scope, args, 1)?;
        let cb_idx = if args.argc_ >= 4 { 3u32 } else { 2u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::copy(&src, &dest).map(|_| ());
        call_cb_void(scope, cb, result, &src, "copyfile")?;
        Ok(())
    }

    // ---- symlinkSync / symlink ------------------------------------------

    pub fn symlink_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let target = get_path(scope, args, 0)?;
        let path_str = get_path(scope, args, 1)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(&target, &path_str).map_err(|e| {
                let _ = make_io_error(scope, &e, &path_str, "symlink");
                js::error::TypeError(e.to_string()).throw(scope)
            })?;
        }
        #[cfg(not(unix))]
        {
            return Err(js::error::TypeError(
                "symlinkSync is not supported on this platform".to_string(),
            )
            .throw(scope));
        }
        Ok(())
    }

    pub fn symlink(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let target = get_path(scope, args, 0)?;
        let path_str = get_path(scope, args, 1)?;
        let cb_idx = if args.argc_ >= 4 { 3u32 } else { 2u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let result = symlink(&target, &path_str);
            call_cb_void(scope, cb, result, &path_str, "symlink")?;
        }
        #[cfg(not(unix))]
        {
            let e = std::io::Error::new(std::io::ErrorKind::Other, "not supported");
            call_cb_void(scope, cb, Err(e), &path_str, "symlink")?;
        }
        Ok(())
    }

    // ---- realpathSync / realpath -----------------------------------------

    pub fn realpath_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        std::fs::canonicalize(&path_str)
            .map(|p| p.display().to_string())
            .map_err(|e| {
                let _ = make_io_error(scope, &e, &path_str, "realpath");
                js::error::TypeError(e.to_string()).throw(scope)
            })
    }

    pub fn realpath(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let result = std::fs::canonicalize(&path_str).map(|p| p.display().to_string());
        call_cb_result(scope, cb, result, &path_str, "realpath", |scope, s| {
            s.to_jsval_raw(scope).unwrap()
        })?;
        Ok(())
    }

    // ---- mkdtempSync / mkdtemp ------------------------------------------

    pub fn mkdtemp_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<String, ExnThrown> {
        let prefix = get_path(scope, args, 0)?;
        // Use nanosecond time as a pseudo-random component for uniqueness.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        for i in 0u32..1000 {
            let suffix = format!("{:06x}", (seed ^ (i * 0x9e37_79b9)) & 0xffffff);
            let dir_path = format!("{}{}", prefix, suffix);
            match std::fs::create_dir(&dir_path) {
                Ok(()) => return Ok(dir_path),
                Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    let _ = make_io_error(scope, &e, &prefix, "mkdtemp");
                    return Err(js::error::TypeError(e.to_string()).throw(scope));
                }
            }
        }
        Err(js::error::TypeError("mkdtempSync: could not create unique directory".to_string())
            .throw(scope))
    }

    pub fn mkdtemp(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let prefix = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        let cb = get_callback(scope, args, cb_idx)?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut found: Option<String> = None;
        for i in 0u32..1000 {
            let suffix = format!("{:06x}", (seed ^ (i * 0x9e37_79b9)) & 0xffffff);
            let dir_path = format!("{}{}", prefix, suffix);
            match std::fs::create_dir(&dir_path) {
                Ok(()) => { found = Some(dir_path); break; }
                Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    call_cb_void(scope, cb, Err(e), &prefix, "mkdtemp")?;
                    return Ok(());
                }
            }
        }
        if let Some(dir) = found {
            let cb_rooted = scope.root_value(cb);
            let null = scope.root_value(js::value::null());
            let dir_val = scope.root_value(dir.to_jsval_raw(scope).unwrap());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[null, dir_val])?;
        } else {
            let e = std::io::Error::new(std::io::ErrorKind::Other, "could not create unique directory");
            call_cb_void(scope, cb, Err(e), &prefix, "mkdtemp")?;
        }
        Ok(())
    }

    // ---- truncateSync ---------------------------------------------------

    pub fn truncate_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let len = if args.argc_ > 1 {
            let v = *args.get(1);
            i64::from_jsval(scope, scope.root_value(v), js::conversion::ConversionBehavior::Default)
                .unwrap_or(0)
        } else {
            0
        };
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path_str)
            .map_err(|e| {
                let _ = make_io_error(scope, &e, &path_str, "open");
                js::error::TypeError(e.to_string()).throw(scope)
            })?;
        file.set_len(len as u64).map_err(|e| {
            let _ = make_io_error(scope, &e, &path_str, "truncate");
            js::error::TypeError(e.to_string()).throw(scope)
        })?;
        Ok(())
    }

    // ---- chmodSync / chmod -----------------------------------------------

    pub fn chmod_sync(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let mode = if args.argc_ > 1 {
            let v = *args.get(1);
            u32::from_jsval(scope, scope.root_value(v), js::conversion::ConversionBehavior::Default)
                .unwrap_or(0o644)
        } else {
            0o644
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path_str, std::fs::Permissions::from_mode(mode))
                .map_err(|e| {
                    let _ = make_io_error(scope, &e, &path_str, "chmod");
                    js::error::TypeError(e.to_string()).throw(scope)
                })?;
        }
        Ok(())
    }

    pub fn chmod(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let path_str = get_path(scope, args, 0)?;
        let cb_idx = if args.argc_ >= 3 { 2u32 } else { 1u32 };
        if args.argc_ <= cb_idx { return Ok(()); }
        let cb = *args.get(cb_idx);
        let mode = if args.argc_ >= 3 {
            let v = *args.get(1);
            u32::from_jsval(scope, scope.root_value(v), js::conversion::ConversionBehavior::Default)
                .unwrap_or(0o644)
        } else {
            0o644
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let result = std::fs::set_permissions(&path_str, std::fs::Permissions::from_mode(mode));
            call_cb_void(scope, cb, result, &path_str, "chmod")?;
        }
        #[cfg(not(unix))]
        {
            let cb_rooted = scope.root_value(cb);
            let null = scope.root_value(js::value::null());
            js::Function::call_value(scope, scope.global().handle(), cb_rooted, &[null])?;
        }
        Ok(())
    }

    /// `fs.constants` — null-prototype object with filesystem constants.
    #[allow(non_upper_case_globals)]
    pub const constants: FsConstantsValue = FsConstantsValue;

    /// `fs.promises` — stub promises namespace (empty object).
    #[allow(non_upper_case_globals)]
    pub const promises: FsPromisesValue = FsPromisesValue;
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(scope: &js::gc::scope::Scope<'_>) {
    unsafe {
        node_fs::register(scope);
    }
}
