//! The on-disk cache for compiled plugins (`DECKWATCH_CACHE_DIR`).
//!
//! Every plugin call compiles the plugin's WebAssembly and extism's kernel
//! (`invoke` in `plugins.rs`). Left to itself, extism caches the result in
//! wasmtime's default directory, `$XDG_CACHE_HOME/wasmtime` or
//! `$HOME/.cache/wasmtime`, and when it can't create that directory (a
//! read-only root filesystem, a `$HOME` the user can't write), every plugin
//! call fails. So deckwatch picks the cache itself, once at startup:
//!
//! - `DECKWATCH_CACHE_DIR` unset or empty: no cache, and an info line saying
//!   how to turn it on.
//! - Set and usable: wasmtime's cache in `<dir>/wasmtime`, configured by
//!   `<dir>/wasmtime-cache.toml`, which deckwatch writes at startup and then
//!   loads through extism, as every plugin call will.
//! - Set but unusable: a warning, and no cache.
//!
//! The cache never makes a plugin call fail. wasmtime reads the config file
//! on every compile, so a compile that fails with the cache is retried
//! without it. If that works, the cache was the problem: deckwatch warns
//! once and stops using it until it restarts.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use extism::{CompiledPlugin, PluginBuilder};

/// The smallest valid WebAssembly module. Compiling it at startup loads the
/// cache config exactly as a plugin call does.
const EMPTY_MODULE: &[u8] = b"\0asm\x01\0\0\0";

/// How plugin calls cache compiled WebAssembly. Resolved once at startup and
/// passed to every call through `PluginTelemetry`.
#[derive(Debug)]
pub struct CompileCache {
    /// The wasmtime cache config file, when the cache is on.
    config: Option<PathBuf>,
    /// Set once a compile failed with the cache and worked without it.
    failed: AtomicBool,
}

impl CompileCache {
    /// No cache: every plugin call compiles its plugin.
    pub const fn off() -> Self {
        Self {
            config: None,
            failed: AtomicBool::new(false),
        }
    }

    /// The cache `dir` (`DECKWATCH_CACHE_DIR`) asks for, or none if it's
    /// unset, empty or unusable. Logs one line saying which. Never fails.
    pub fn resolve(dir: Option<&Path>) -> Self {
        let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) else {
            tracing::info!(
                "plugin compile cache off: every plugin call compiles its plugin; \
                 set DECKWATCH_CACHE_DIR to a writable directory to cache them"
            );
            return Self::off();
        };
        match prepare(dir) {
            Ok(config) => {
                tracing::info!(
                    directory = %dir.display(),
                    "caching compiled plugins in DECKWATCH_CACHE_DIR"
                );
                Self {
                    config: Some(config),
                    failed: AtomicBool::new(false),
                }
            }
            Err(e) => {
                tracing::warn!(
                    directory = %dir.display(),
                    error = %format!("{e:#}"),
                    "DECKWATCH_CACHE_DIR is not usable, so the plugin compile cache is off"
                );
                Self::off()
            }
        }
    }

    /// The wasmtime cache config file plugin calls compile with, or `None`
    /// while the cache is off.
    pub fn config_file(&self) -> Option<&Path> {
        if self.failed.load(Ordering::Relaxed) {
            return None;
        }
        self.config.as_deref()
    }

    /// Compile `builder` with the cache, or without one while it's off. A
    /// compile that fails with the cache is retried without it.
    pub fn compile(&self, builder: PluginBuilder<'_>) -> Result<CompiledPlugin, extism::Error> {
        let Some(config) = self.config_file() else {
            return builder.with_cache_disabled().compile();
        };
        let uncached = builder.clone().with_cache_disabled();
        let cache_error = match builder.with_cache_config(config).compile() {
            Ok(compiled) => return Ok(compiled),
            Err(e) => e,
        };
        // A module that doesn't compile fails here too, with its own error.
        let compiled = uncached.compile()?;
        if !self.failed.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                config = %config.display(),
                error = %format!("{cache_error:#}"),
                "the plugin compile cache stopped working, so it is off until deckwatch restarts"
            );
        }
        Ok(compiled)
    }
}

/// Create `<dir>/wasmtime`, check deckwatch can write to it, write the config
/// file next to it, and load that the way plugin calls will. Returns the
/// config file's path.
///
/// The config file isn't inside `<dir>/wasmtime` because wasmtime's cleanup
/// deletes files there that it doesn't recognize.
fn prepare(dir: &Path) -> anyhow::Result<PathBuf> {
    // wasmtime only takes an absolute directory.
    let dir = std::path::absolute(dir).context("can't resolve the directory")?;
    let cache_dir = dir.join("wasmtime");
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("can't create {}", cache_dir.display()))?;
    let probe = cache_dir.join(format!(".deckwatch-write-check-{}", std::process::id()));
    fs::write(&probe, b"").with_context(|| format!("can't write to {}", cache_dir.display()))?;
    let _ = fs::remove_file(&probe);

    let config = dir.join("wasmtime-cache.toml");
    let directory = cache_dir
        .to_str()
        .context("the path isn't UTF-8, which wasmtime's config file needs")?;
    fs::write(&config, config_toml(directory))
        .with_context(|| format!("can't write {}", config.display()))?;
    load(&config)?;
    Ok(config)
}

/// Load `config` the way plugin calls do: extism hands it to wasmtime.
fn load(config: &Path) -> anyhow::Result<()> {
    PluginBuilder::new(EMPTY_MODULE)
        .with_cache_config(config)
        .compile()
        .map(drop)
        .with_context(|| format!("wasmtime can't use {}", config.display()))
}

/// wasmtime's cache config for the cache in `directory`. wasmtime 49 accepts
/// only `directory` and its tuning keys under `[cache]`; an unknown key, such
/// as older wasmtime's `enabled`, fails the whole file.
fn config_toml(directory: &str) -> String {
    format!(
        "# wasmtime's compile cache for deckwatch's plugins. deckwatch writes\n\
         # this file from DECKWATCH_CACHE_DIR on every start.\n\
         [cache]\n\
         directory = {}\n",
        toml_string(directory)
    )
}

/// `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "plugin_cache_tests.rs"]
mod tests;
