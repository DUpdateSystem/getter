//! Operation-owned runtime/local Lua hook loading.
//!
//! This module is an internal harness for ADR-0012 `rc/hook/*.lua` runtime
//! policy. It only loads local hook source into an already prepared Lua
//! environment; callers still decide which host functions exist and which
//! operation policy applies. It intentionally does not expose hooks through
//! CLI/native/Flutter surfaces or make hook policy part of `getter-core`.

use mlua::Lua;
use std::fs;
use std::path::{Path, PathBuf};

const RUNTIME_HOOK_DIR: &str = "rc/hook";

/// Loads enabled runtime hooks from `<data-dir>/rc/hook/*.lua`.
///
/// Hooks are discovered from the filesystem only, dot-prefixed Lua basenames
/// are ignored, and the remaining hooks are loaded in deterministic path order.
/// Any read or runtime error fails closed by returning an error to the caller.
pub fn load_runtime_hooks(lua: &Lua, data_dir: &Path) -> mlua::Result<Vec<PathBuf>> {
    let hook_dir = data_dir.join(RUNTIME_HOOK_DIR);
    let entries = match fs::read_dir(&hook_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(mlua::Error::external(format!(
                "failed to read runtime hook directory {}: {source}",
                hook_dir.display()
            )))
        }
    };

    let mut hooks = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            mlua::Error::external(format!(
                "failed to read runtime hook directory entry in {}: {source}",
                hook_dir.display()
            ))
        })?;
        let path = entry.path();
        if !is_enabled_lua_hook(&path) {
            continue;
        }
        hooks.push(path);
    }
    hooks.sort();

    for hook in &hooks {
        let source = fs::read_to_string(hook).map_err(|source| {
            mlua::Error::external(format!(
                "failed to read runtime hook {}: {source}",
                hook.display()
            ))
        })?;
        lua.load(&source)
            .set_name(hook.to_string_lossy().as_ref())
            .exec()
            .map_err(|source| {
                mlua::Error::external(format!(
                    "failed to execute runtime hook {}: {source}",
                    hook.display()
                ))
            })?;
    }

    Ok(hooks)
}

fn is_enabled_lua_hook(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("lua") {
        return false;
    }
    let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
        return false;
    };
    !file_name.starts_with('.')
}
