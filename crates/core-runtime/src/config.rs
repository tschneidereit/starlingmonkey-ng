// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! CLI argument parsing for the StarlingMonkey runtime.

use std::{
    env,
    path::{Path, PathBuf},
};

use clap::Parser;

/// StarlingMonkey runtime configuration.
#[derive(Parser, Debug, Clone)]
#[command(name = "starling", about = "StarlingMonkey JS runtime")]
pub struct RuntimeConfig {
    /// Path to the content script to execute.
    #[arg(default_value = "./index.js")]
    pub script_path: String,

    /// Evaluate inline script instead of a file.
    #[arg(short = 'e', long = "eval")]
    pub eval_script: Option<String>,

    /// Path to an initialization script (runs in the default global, as a
    /// classic script, before the content script; must complete synchronously).
    #[arg(short = 'i', long = "initializer-script")]
    pub initializer_script_path: Option<String>,

    /// Enable verbose logging.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Enable script debugging via socket connection.
    #[arg(short = 'd', long = "debug")]
    pub debugging: bool,

    /// Use classic (non-module) script mode.
    #[arg(long = "legacy-script")]
    pub legacy_script: bool,

    /// Enable WPT (Web Platform Tests) mode.
    #[arg(long = "wpt-mode")]
    pub wpt_mode: bool,

    /// Override the location URL for initialization.
    #[arg(long = "init-location")]
    pub init_location: Option<String>,

    /// Strip this prefix from script paths.
    #[arg(long = "strip-path-prefix")]
    pub path_prefix: Option<String>,

    /// Serve incoming HTTP requests on this TCP port (native), dispatching each as a `fetch` event
    /// to the handler the script registered with `addEventListener("fetch", …)`. The script runs
    /// once to register handlers; the runtime then stays alive to handle requests.
    #[arg(long = "serve")]
    pub serve: Option<u16>,

    /// Pre-initialize the runtime (used during wizer snapshot).
    #[arg(skip)]
    pub pre_initialize: bool,
}

impl RuntimeConfig {
    /// Whether to use ES module mode (the default, unless --legacy-script).
    pub fn module_mode(&self) -> bool {
        !self.legacy_script
    }

    /// The effective content script source — either from --eval or from the file path.
    pub fn content_script(&self) -> Option<&str> {
        self.eval_script.as_deref()
    }

    /// The base directory for resolving module imports.
    ///
    /// For `--eval` scripts this is the current working directory. Otherwise,
    /// it is the directory containing the content script.
    pub fn base_path(&self) -> PathBuf {
        if self.eval_script.is_some() {
            return env::current_dir().unwrap_or_default();
        }
        let path = Path::new(&self.script_path);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            env::current_dir().unwrap_or_default().join(path)
        };
        match abs.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            // A filesystem root has no parent; resolve relative to it directly.
            _ => abs,
        }
    }

    /// Parse from an argument string (e.g., from STARLINGMONKEY_CONFIG env var).
    ///
    /// Splits the string on whitespace, respecting single/double quotes.
    pub fn from_arg_string(args: &str) -> Result<Self, clap::Error> {
        Self::try_parse_from(split_args(args)?)
    }

    /// Parse from WASI CLI arguments (as provided by the host).
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, clap::Error> {
        Self::try_parse_from(args)
    }

    /// Parse from the STARLINGMONKEY_CONFIG environment variable.
    pub fn from_env() -> Result<Self, clap::Error> {
        match std::env::var("STARLINGMONKEY_CONFIG") {
            Ok(config) => Self::from_arg_string(&config),
            Err(_) => Self::try_parse_from(["starling"]),
        }
    }

    /// Parse from stdin (for wizer pre-initialization).
    /// Reads a single line of arguments from stdin.
    pub fn from_stdin() -> Result<Self, clap::Error> {
        let mut input = String::new();
        if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err() {
            return Self::try_parse_from(["starling"]);
        }
        let mut config = Self::from_arg_string(input.trim())?;
        config.pre_initialize = true;
        Ok(config)
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::try_parse_from(["starling"]).unwrap()
    }
}

/// Split an argument string into individual arguments, respecting quotes.
fn split_args(s: &str) -> Result<Vec<String>, clap::Error> {
    let mut args = vec!["starling".into()];
    args.extend(shlex::split(s).ok_or(clap::Error::raw(
        clap::error::ErrorKind::InvalidValue,
        "Failed to parse arguments",
    ))?);
    Ok(args)
}

// Nothing platform-specific in these tests, so skip them on wasm32.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RuntimeConfig::default();
        assert_eq!(config.script_path, "./index.js");
        assert!(config.module_mode());
        assert!(!config.verbose);
        assert!(!config.debugging);
        assert!(!config.wpt_mode);
    }

    #[test]
    fn test_parse_args() {
        let config = RuntimeConfig::from_arg_string("-v --legacy-script app.js").unwrap();
        assert_eq!(config.script_path, "app.js");
        assert!(config.verbose);
        assert!(!config.module_mode());
    }

    #[test]
    fn base_path_for_eval_is_cwd() {
        let config = RuntimeConfig::from_arg_string("-e 42").unwrap();
        assert_eq!(config.base_path(), env::current_dir().unwrap());
    }

    #[test]
    fn base_path_for_bare_filename_is_cwd() {
        let config = RuntimeConfig::from_arg_string("app.js").unwrap();
        assert_eq!(config.base_path(), env::current_dir().unwrap());
    }

    #[test]
    fn base_path_for_absolute_script_is_its_directory() {
        let config = RuntimeConfig::from_arg_string("/srv/js/app.js").unwrap();
        assert_eq!(config.base_path(), Path::new("/srv/js"));
    }

    #[test]
    fn base_path_for_relative_script_is_absolute() {
        let config = RuntimeConfig::from_arg_string("sub/dir/app.js").unwrap();
        assert_eq!(
            config.base_path(),
            env::current_dir().unwrap().join("sub/dir")
        );
    }
}
