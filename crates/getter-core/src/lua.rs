//! Minimal Lua package-file evaluation and Rust validation boundary.

use crate::repository::RepositoryLayout;
use crate::{InstalledTarget, PackageId, PackagePermissions, ResolvedPackage};
use mlua::{Lua, Table, Value};
use serde_json::{Map, Number, Value as JsonValue};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LuaPackageError {
    #[error("failed to read Lua package file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Lua runtime error in {path}: {source}")]
    Runtime {
        path: PathBuf,
        #[source]
        source: mlua::Error,
    },
    #[error("Lua package {path} did not return a table")]
    NotATable { path: PathBuf },
    #[error("Lua package {path} returned unsupported value at {location}: {value_type}")]
    UnsupportedValue {
        path: PathBuf,
        location: String,
        value_type: &'static str,
    },
    #[error("schema validation failed for {path}: {message}")]
    Schema { path: PathBuf, message: String },
    #[error("domain validation failed for {path}: {message}")]
    Domain { path: PathBuf, message: String },
}

/// Evaluate and validate a Lua package file from a repository layout.
pub fn evaluate_package_file(
    repository: &RepositoryLayout,
    path: impl AsRef<Path>,
) -> Result<ResolvedPackage, LuaPackageError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| LuaPackageError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    evaluate_package_source(repository, path, &source)
}

/// Evaluate package source text. This is public for focused tests and future CLI
/// plumbing; repository callers should normally use [`evaluate_package_file`].
pub fn evaluate_package_source(
    repository: &RepositoryLayout,
    path: impl AsRef<Path>,
    source: &str,
) -> Result<ResolvedPackage, LuaPackageError> {
    let path = path.as_ref().to_path_buf();
    let lua = Lua::new();
    configure_package_path(&lua, repository).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source,
    })?;
    remove_unsafe_globals(&lua).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source,
    })?;
    install_helpers(&lua).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source,
    })?;

    let value = lua
        .load(source)
        .set_name(path.to_string_lossy().as_ref())
        .eval::<Value>()
        .map_err(|source| LuaPackageError::Runtime {
            path: path.clone(),
            source,
        })?;
    let table = match value {
        Value::Table(table) => table,
        _ => return Err(LuaPackageError::NotATable { path }),
    };
    let json = lua_table_to_json(&path, "$", table)?;
    validate_package_json(repository, &path, json)
}

fn configure_package_path(lua: &Lua, repository: &RepositoryLayout) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let lib_pattern = repository.lib_dir.join("?.lua");
    let nested_lib_pattern = repository.lib_dir.join("?/init.lua");
    let new_path = format!(
        "{};{}",
        lib_pattern.to_string_lossy(),
        nested_lib_pattern.to_string_lossy()
    );
    package.set("path", new_path)?;
    package.set("cpath", "")?;
    package.set("loadlib", Value::Nil)?;
    install_lib_prefix_searcher(lua, &package, repository.lib_dir.clone())?;
    disable_native_module_searchers(&package)
}

fn disable_native_module_searchers(package: &Table) -> mlua::Result<()> {
    let searchers: Table = package.get("searchers")?;
    let len = searchers.raw_len();
    for index in 4..=len {
        searchers.raw_set(index, Value::Nil)?;
    }
    Ok(())
}

fn remove_unsafe_globals(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let package: Table = globals.get("package")?;
    let loaded: Table = package.get("loaded")?;
    for name in ["os", "io", "debug"] {
        loaded.set(name, Value::Nil)?;
    }
    for name in ["os", "io", "debug", "dofile", "loadfile", "package"] {
        globals.set(name, Value::Nil)?;
    }
    Ok(())
}

fn install_lib_prefix_searcher(lua: &Lua, package: &Table, lib_dir: PathBuf) -> mlua::Result<()> {
    let searchers: Table = package.get("searchers")?;
    let searcher = lua.create_function(move |lua, module: String| {
        let Some(module) = module.strip_prefix("lib.") else {
            return lua
                .create_string("\n\tconstrained repository lib searcher only handles lib.* modules")
                .map(Value::String);
        };

        let Some(relative_module) = module_to_relative_path(module) else {
            return lua
                .create_string(format!(
                    "\n\tinvalid repository lib module name 'lib.{module}'"
                ))
                .map(Value::String);
        };

        let module_path = lib_dir.join(&relative_module).with_extension("lua");
        let init_path = lib_dir.join(&relative_module).join("init.lua");
        for candidate in [&module_path, &init_path] {
            if candidate.is_file() {
                let source = fs::read_to_string(candidate).map_err(mlua::Error::external)?;
                let chunk = lua
                    .load(&source)
                    .set_name(candidate.to_string_lossy().as_ref());
                return chunk.into_function().map(Value::Function);
            }
        }

        lua.create_string(format!(
            "\n\tno repository lib module 'lib.{module}' in {}",
            lib_dir.display()
        ))
        .map(Value::String)
    })?;

    let len = searchers.raw_len();
    for index in (2..=len).rev() {
        let value: Value = searchers.raw_get(index)?;
        searchers.raw_set(index + 1, value)?;
    }
    searchers.raw_set(2, searcher)
}

fn module_to_relative_path(module: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in module.split('.') {
        if part.is_empty()
            || part == ".."
            || part.contains('/')
            || part.contains('\\')
            || part.contains(std::path::MAIN_SEPARATOR)
        {
            return None;
        }
        path.push(part);
    }
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

fn install_helpers(lua: &Lua) -> mlua::Result<()> {
    let package_fn = lua.create_function(|_, table: Table| Ok(table))?;
    lua.globals().set("package_def", package_fn.clone())?;
    lua.globals().set("android_app", package_fn.clone())?;
    lua.globals().set("magisk_module", package_fn.clone())?;
    lua.globals().set("generic_package", package_fn)?;
    Ok(())
}

fn lua_table_to_json(
    path: &Path,
    location: &str,
    table: Table,
) -> Result<JsonValue, LuaPackageError> {
    if is_array_table(&table).map_err(|source| LuaPackageError::Runtime {
        path: path.to_path_buf(),
        source,
    })? {
        let mut array = Vec::new();
        for pair in table.sequence_values::<Value>() {
            let value = pair.map_err(|source| LuaPackageError::Runtime {
                path: path.to_path_buf(),
                source,
            })?;
            array.push(lua_value_to_json(path, &format!("{location}[]"), value)?);
        }
        Ok(JsonValue::Array(array))
    } else {
        let mut object = Map::new();
        for pair in table.pairs::<Value, Value>() {
            let (key, value) = pair.map_err(|source| LuaPackageError::Runtime {
                path: path.to_path_buf(),
                source,
            })?;
            let key = match key {
                Value::String(value) => value
                    .to_str()
                    .map_err(|source| LuaPackageError::Runtime {
                        path: path.to_path_buf(),
                        source,
                    })?
                    .to_owned(),
                Value::Integer(value) => value.to_string(),
                _ => {
                    return Err(LuaPackageError::UnsupportedValue {
                        path: path.to_path_buf(),
                        location: format!("{location}.<key>"),
                        value_type: key.type_name(),
                    })
                }
            };
            let child_location = format!("{location}.{key}");
            object.insert(key, lua_value_to_json(path, &child_location, value)?);
        }
        Ok(JsonValue::Object(object))
    }
}

fn lua_value_to_json(
    path: &Path,
    location: &str,
    value: Value,
) -> Result<JsonValue, LuaPackageError> {
    match value {
        Value::Nil => Ok(JsonValue::Null),
        Value::Boolean(value) => Ok(JsonValue::Bool(value)),
        Value::Integer(value) => Ok(JsonValue::Number(Number::from(value))),
        Value::Number(value) => Number::from_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| LuaPackageError::UnsupportedValue {
                path: path.to_path_buf(),
                location: location.to_owned(),
                value_type: "non-finite number",
            }),
        Value::String(value) => Ok(JsonValue::String(
            value
                .to_str()
                .map_err(|source| LuaPackageError::Runtime {
                    path: path.to_path_buf(),
                    source,
                })?
                .to_owned(),
        )),
        Value::Table(table) => lua_table_to_json(path, location, table),
        _ => Err(LuaPackageError::UnsupportedValue {
            path: path.to_path_buf(),
            location: location.to_owned(),
            value_type: value.type_name(),
        }),
    }
}

fn is_array_table(table: &Table) -> mlua::Result<bool> {
    let len = table.raw_len();
    if len == 0 {
        // Empty tables are accepted as objects by default. Package schemas avoid
        // ambiguous empty array/object requirements at this boundary.
        return Ok(false);
    }
    let mut count = 0usize;
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        match key {
            Value::Integer(index) if index >= 1 && (index as usize) <= len => count += 1,
            _ => return Ok(false),
        }
    }
    Ok(count == len)
}

fn validate_package_json(
    repository: &RepositoryLayout,
    path: &Path,
    value: JsonValue,
) -> Result<ResolvedPackage, LuaPackageError> {
    let object = value.as_object().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: "package value must be an object".to_owned(),
    })?;

    let id = required_string(path, object, "id")?;
    let id: PackageId = id.parse().map_err(|source| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: format!("invalid package id: {source}"),
    })?;
    let path_id = crate::repository::package_id_from_path(&repository.packages_dir, path).map_err(
        |source| LuaPackageError::Domain {
            path: path.to_path_buf(),
            message: format!("failed to derive package id from path: {source}"),
        },
    )?;
    if id != path_id {
        return Err(LuaPackageError::Domain {
            path: path.to_path_buf(),
            message: format!("package id '{id}' does not match path-derived id '{path_id}'"),
        });
    }

    let name = required_string(path, object, "name")?.to_owned();
    let installed = parse_installed_targets(path, object.get("installed"))?;
    let permissions = parse_permissions(path, object.get("permissions"))?;
    let source_priority =
        parse_string_array(path, "source_priority", object.get("source_priority"))?;

    Ok(ResolvedPackage {
        id,
        repository: repository.metadata.id.clone(),
        name,
        installed,
        permissions,
        source_priority,
    })
}

fn required_string<'a>(
    path: &Path,
    object: &'a Map<String, JsonValue>,
    field: &str,
) -> Result<&'a str, LuaPackageError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| LuaPackageError::Schema {
            path: path.to_path_buf(),
            message: format!("required string field '{field}' is missing"),
        })
}

fn parse_installed_targets(
    path: &Path,
    value: Option<&JsonValue>,
) -> Result<Vec<InstalledTarget>, LuaPackageError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: "field 'installed' must be an array".to_owned(),
    })?;
    array
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| LuaPackageError::Schema {
                path: path.to_path_buf(),
                message: "installed entries must be objects".to_owned(),
            })?;
            let kind = required_string(path, object, "kind")?;
            match kind {
                "android_package" => Ok(InstalledTarget::AndroidPackage {
                    package_name: required_string(path, object, "package_name")?.to_owned(),
                }),
                "magisk_module" => Ok(InstalledTarget::MagiskModule {
                    module_id: required_string(path, object, "module_id")?.to_owned(),
                }),
                "generic" => Ok(InstalledTarget::Generic {
                    id: required_string(path, object, "id")?.to_owned(),
                }),
                other => Err(LuaPackageError::Schema {
                    path: path.to_path_buf(),
                    message: format!("unknown installed target kind '{other}'"),
                }),
            }
        })
        .collect()
}

fn parse_permissions(
    path: &Path,
    value: Option<&JsonValue>,
) -> Result<PackagePermissions, LuaPackageError> {
    let Some(value) = value else {
        return Ok(PackagePermissions::default());
    };
    let object = value.as_object().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: "field 'permissions' must be an object".to_owned(),
    })?;
    Ok(PackagePermissions {
        free_network: object
            .get("free_network")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
    })
}

fn parse_string_array(
    path: &Path,
    field: &str,
    value: Option<&JsonValue>,
) -> Result<Vec<String>, LuaPackageError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: format!("field '{field}' must be an array"),
    })?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| LuaPackageError::Schema {
                    path: path.to_path_buf(),
                    message: format!("field '{field}' must contain only strings"),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::RepositoryLayout;
    use std::fs;

    fn fixture_repo() -> (tempfile::TempDir, RepositoryLayout, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("repo.toml"),
            r#"id = "official"
name = "UpgradeAll Official"
priority = 0
api_version = "getter.repo.v1"
"#,
        )
        .unwrap();
        fs::create_dir(root.join("packages")).unwrap();
        fs::create_dir(root.join("packages/android")).unwrap();
        fs::create_dir(root.join("lib")).unwrap();
        fs::create_dir(root.join("templates")).unwrap();
        let package_path = root.join("packages/android/org.fdroid.fdroid.lua");
        fs::write(&package_path, "return {}").unwrap();
        let layout = RepositoryLayout::load(root).unwrap();
        (temp, layout, package_path)
    }

    #[test]
    fn evaluates_json_like_lua_package_table() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            &package_path,
            r#"
return package_def {
  id = "android/org.fdroid.fdroid",
  name = "F-Droid",
  installed = {
    { kind = "android_package", package_name = "org.fdroid.fdroid" },
  },
  permissions = { free_network = true },
  source_priority = { "github", "fdroid" },
}
"#,
        )
        .unwrap();

        let package = evaluate_package_file(&layout, &package_path).unwrap();
        assert_eq!(package.id.to_string(), "android/org.fdroid.fdroid");
        assert_eq!(package.repository.as_str(), "official");
        assert_eq!(package.name, "F-Droid");
        assert_eq!(
            package.installed,
            vec![InstalledTarget::AndroidPackage {
                package_name: "org.fdroid.fdroid".to_owned()
            }]
        );
        assert!(package.permissions.free_network);
        assert_eq!(package.source_priority, vec!["github", "fdroid"]);
    }

    #[test]
    fn rejects_package_id_that_does_not_match_path() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            &package_path,
            r#"return { id = "android/com.termux", name = "Termux" }"#,
        )
        .unwrap();

        let err = evaluate_package_file(&layout, &package_path).unwrap_err();
        assert!(matches!(err, LuaPackageError::Domain { .. }));
    }

    #[test]
    fn require_can_load_repository_lib_modules() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            layout.lib_dir.join("android.lua"),
            r#"
return {
  local_app = function(input)
    return {
      id = input.id,
      name = input.name,
      installed = {
        { kind = "android_package", package_name = input.package_name },
      },
    }
  end
}
"#,
        )
        .unwrap();
        fs::write(
            &package_path,
            r#"
local android = require("android")
return android.local_app {
  id = "android/org.fdroid.fdroid",
  name = "F-Droid",
  package_name = "org.fdroid.fdroid",
}
"#,
        )
        .unwrap();

        let package = evaluate_package_file(&layout, &package_path).unwrap();
        assert_eq!(package.name, "F-Droid");
        assert_eq!(package.installed.len(), 1);
    }

    #[test]
    fn require_can_load_repository_lib_modules_with_lib_prefix() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            layout.lib_dir.join("android.lua"),
            r#"
return {
  local_app = function(input)
    return {
      id = input.id,
      name = input.name,
      installed = {
        { kind = "android_package", package_name = input.package_name },
      },
    }
  end
}
"#,
        )
        .unwrap();
        fs::write(
            &package_path,
            r#"
local android = require("lib.android")
return android.local_app {
  id = "android/org.fdroid.fdroid",
  name = "F-Droid",
  package_name = "org.fdroid.fdroid",
}
"#,
        )
        .unwrap();

        let package = evaluate_package_file(&layout, &package_path).unwrap();
        assert_eq!(package.name, "F-Droid");
        assert_eq!(package.installed.len(), 1);
    }

    #[test]
    fn lib_prefixed_searcher_does_not_expose_repository_templates() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            layout.templates_dir.join("android.lua"),
            r#"return { leaked = true }"#,
        )
        .unwrap();
        fs::write(
            &package_path,
            r#"
local ok = pcall(require, "templates.android")
return {
  id = "android/org.fdroid.fdroid",
  name = ok and "leaked" or "F-Droid",
}
"#,
        )
        .unwrap();

        let package = evaluate_package_file(&layout, &package_path).unwrap();
        assert_eq!(package.name, "F-Droid");
    }

    #[test]
    fn lua_environment_does_not_expose_process_or_file_system_globals() {
        let temp = tempfile::tempdir().unwrap();
        let side_effect = temp.path().join("side-effect");
        let (_repo_temp, layout, package_path) = fixture_repo();
        fs::write(
            &package_path,
            format!(
                r#"
return {{
  id = "android/org.fdroid.fdroid",
  name = (os == nil and io == nil and debug == nil and dofile == nil and loadfile == nil and package == nil) and "F-Droid" or "leaked",
  side_effect = rawget(_G, "os") and os.execute("touch {}"),
}}
"#,
                side_effect.display()
            ),
        )
        .unwrap();

        let package = evaluate_package_file(&layout, &package_path).unwrap();
        assert_eq!(package.name, "F-Droid");
        assert!(!side_effect.exists());
    }

    #[test]
    fn repository_root_is_not_exposed_even_when_cwd_is_repository_root() {
        let (_temp, layout, package_path) = fixture_repo();
        fs::write(
            layout.root.join("rootleak.lua"),
            r#"return { leaked = true }"#,
        )
        .unwrap();
        fs::write(
            layout.templates_dir.join("android.lua"),
            r#"return { leaked = true }"#,
        )
        .unwrap();
        fs::write(
            &package_path,
            r#"
local root_ok = pcall(require, "rootleak")
local template_ok = pcall(require, "templates.android")
return {
  id = "android/org.fdroid.fdroid",
  name = (root_ok or template_ok) and "leaked" or "F-Droid",
}
"#,
        )
        .unwrap();

        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&layout.root).unwrap();
        let result = evaluate_package_file(&layout, &package_path);
        std::env::set_current_dir(original_cwd).unwrap();

        let package = result.unwrap();
        assert_eq!(package.name, "F-Droid");
    }
}
