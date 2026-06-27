//! Minimal Lua package-file evaluation and Rust validation boundary.

use crate::repository::{
    PackageDirectory, PackageDirectoryMetadata, PackageLuaPermission, PackageTypeMetadata,
    PackageVersionScript, RepositoryLoadError, LUA_API_SHEBANG_V1, REPOSITORY_LUACLASS_DIR,
};
use crate::{
    InstalledTarget, PackageId, PackagePermissions, RepositoryId, ResolvedPackage, UpdateArtifact,
    UpdateCandidate,
};
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
        source: Box<mlua::Error>,
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
    #[error("repository layout failed for {path}: {source}")]
    Repository {
        path: PathBuf,
        #[source]
        source: Box<RepositoryLoadError>,
    },
}

pub fn evaluate_package_directory_script(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
) -> Result<ResolvedPackage, LuaPackageError> {
    let source = fs::read_to_string(&script.path).map_err(|source| LuaPackageError::ReadFile {
        path: script.path.clone(),
        source,
    })?;
    if source.lines().next() != Some(LUA_API_SHEBANG_V1) {
        return Err(LuaPackageError::Repository {
            path: script.path.clone(),
            source: Box::new(RepositoryLoadError::MissingLuaApiShebang {
                path: script.path.clone(),
                expected: LUA_API_SHEBANG_V1,
            }),
        });
    }
    let json = evaluate_package_source_to_json(
        &LuaRepositoryEnvironment::PackageDirectory { package },
        &script.path,
        &source,
    )?;
    validate_package_directory_version_json(repository_id, package, metadata, script, json)
}

fn evaluate_package_source_to_json(
    environment: &LuaRepositoryEnvironment<'_>,
    path: &Path,
    source: &str,
) -> Result<JsonValue, LuaPackageError> {
    let path = path.to_path_buf();
    let lua = Lua::new();
    configure_package_path(&lua, environment).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source: Box::new(source),
    })?;
    remove_unsafe_globals(&lua).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source: Box::new(source),
    })?;
    install_helpers(&lua, environment).map_err(|source| LuaPackageError::Runtime {
        path: path.clone(),
        source: Box::new(source),
    })?;

    let value = lua
        .load(source)
        .set_name(path.to_string_lossy().as_ref())
        .eval::<Value>()
        .map_err(|source| LuaPackageError::Runtime {
            path: path.clone(),
            source: Box::new(source),
        })?;
    let table = match value {
        Value::Table(table) => table,
        _ => return Err(LuaPackageError::NotATable { path }),
    };
    lua_table_to_json(&path, "$", table)
}

enum LuaRepositoryEnvironment<'a> {
    PackageDirectory { package: &'a PackageDirectory },
}

fn configure_package_path(
    lua: &Lua,
    environment: &LuaRepositoryEnvironment<'_>,
) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    package.set("cpath", "")?;
    package.set("loadlib", Value::Nil)?;
    match environment {
        LuaRepositoryEnvironment::PackageDirectory {
            package: package_dir,
        } => {
            package.set("path", "")?;
            install_luaclass_prefix_searcher(
                lua,
                &package,
                package_directory_repository_root(package_dir).join(REPOSITORY_LUACLASS_DIR),
            )?;
        }
    }
    disable_native_module_searchers(&package)
}

fn package_directory_repository_root(package: &PackageDirectory) -> PathBuf {
    let mut root = package.path.clone();
    for _ in package.id.to_string().split('/') {
        root.pop();
    }
    root
}

fn disable_native_module_searchers(package: &Table) -> mlua::Result<()> {
    let searchers = package_searchers(package)?;
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

fn install_luaclass_prefix_searcher(
    lua: &Lua,
    package: &Table,
    luaclass_dir: PathBuf,
) -> mlua::Result<()> {
    install_prefixed_file_searcher(
        lua,
        package,
        "luaclass.",
        luaclass_dir,
        "constrained repository luaclass searcher only handles luaclass.* modules",
    )
}

fn install_prefixed_file_searcher(
    lua: &Lua,
    package: &Table,
    prefix: &'static str,
    module_dir: PathBuf,
    wrong_prefix_message: &'static str,
) -> mlua::Result<()> {
    let searchers = package_searchers(package)?;
    let searcher = lua.create_function(move |lua, module: String| {
        let Some(module) = module.strip_prefix(prefix) else {
            return lua
                .create_string(format!("\n\t{wrong_prefix_message}"))
                .map(Value::String);
        };

        let Some(relative_module) = module_to_relative_path(module) else {
            return lua
                .create_string(format!(
                    "\n\tinvalid repository module name '{prefix}{module}'"
                ))
                .map(Value::String);
        };

        let module_path = module_dir.join(&relative_module).with_extension("lua");
        let init_path = module_dir.join(&relative_module).join("init.lua");
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
            "\n\tno repository module '{prefix}{module}' in {}",
            module_dir.display()
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

fn package_searchers(package: &Table) -> mlua::Result<Table> {
    match package.get::<Value>("searchers")? {
        Value::Table(searchers) => Ok(searchers),
        _ => package.get("loaders"),
    }
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

fn install_helpers(lua: &Lua, environment: &LuaRepositoryEnvironment<'_>) -> mlua::Result<()> {
    let package_fn = lua.create_function(|_, table: Table| Ok(table))?;
    lua.globals().set("package_def", package_fn.clone())?;
    lua.globals().set("package_version", package_fn.clone())?;
    lua.globals().set("android_app", package_fn.clone())?;
    lua.globals().set("magisk_module", package_fn.clone())?;
    lua.globals().set("generic_package", package_fn)?;
    let LuaRepositoryEnvironment::PackageDirectory { package } = environment;
    install_package_file_helpers(lua, package)?;
    Ok(())
}

fn install_package_file_helpers(lua: &Lua, package: &PackageDirectory) -> mlua::Result<()> {
    let files_root = package.path.join("files");
    let read_package_file = lua.create_function(move |lua, requested_path: String| {
        let relative_path = package_file_relative_path(&requested_path)?;
        let path = files_root.join(relative_path);
        let metadata = fs::metadata(&path).map_err(mlua::Error::external)?;
        if !metadata.is_file() {
            return Err(mlua::Error::external(format!(
                "read_package_file target {} is not a file",
                path.display()
            )));
        }
        let bytes = fs::read(&path).map_err(mlua::Error::external)?;
        lua.create_string(&bytes)
    })?;

    let globals = lua.globals();
    let getter_builtin = match globals.get::<Value>("getter_builtin")? {
        Value::Table(table) => table,
        Value::Nil => {
            let table = lua.create_table()?;
            globals.set("getter_builtin", table.clone())?;
            table
        }
        _ => return Err(mlua::Error::external("getter_builtin must be a table")),
    };
    getter_builtin.set("read_package_file", read_package_file.clone())?;
    globals.set("read_package_file", read_package_file)
}

fn package_file_relative_path(path: &str) -> mlua::Result<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            std::path::Component::Normal(part) => relative.push(part),
            _ => {
                return Err(mlua::Error::external(
                    "read_package_file path must be relative to files/ and must not contain special path components",
                ))
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(mlua::Error::external(
            "read_package_file path must not be empty",
        ));
    }
    Ok(relative)
}

fn lua_table_to_json(
    path: &Path,
    location: &str,
    table: Table,
) -> Result<JsonValue, LuaPackageError> {
    if is_array_table(&table).map_err(|source| LuaPackageError::Runtime {
        path: path.to_path_buf(),
        source: Box::new(source),
    })? {
        let mut array = Vec::new();
        for pair in table.sequence_values::<Value>() {
            let value = pair.map_err(|source| LuaPackageError::Runtime {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
            array.push(lua_value_to_json(path, &format!("{location}[]"), value)?);
        }
        Ok(JsonValue::Array(array))
    } else {
        let mut object = Map::new();
        for pair in table.pairs::<Value, Value>() {
            let (key, value) = pair.map_err(|source| LuaPackageError::Runtime {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
            let key = match key {
                Value::String(value) => value
                    .to_str()
                    .map_err(|source| LuaPackageError::Runtime {
                        path: path.to_path_buf(),
                        source: Box::new(source),
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
                    source: Box::new(source),
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

fn validate_package_directory_version_json(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
    value: JsonValue,
) -> Result<ResolvedPackage, LuaPackageError> {
    let object = value.as_object().ok_or_else(|| LuaPackageError::Schema {
        path: script.path.clone(),
        message: "package value must be an object".to_owned(),
    })?;
    if object.contains_key("id") {
        return Err(LuaPackageError::Schema {
            path: script.path.clone(),
            message: "field 'id' must not be declared by package version scripts; package id is derived from path".to_owned(),
        });
    }
    let name = object
        .get("name")
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| metadata.display_name_for(&package.id));
    let mut package = package_from_version_json(
        repository_id.clone(),
        package.id.clone(),
        name,
        metadata_permissions_for_script(metadata, &script.file_name),
        &script.path,
        object,
    )?;
    apply_metadata_installed_target(&mut package, metadata, &script.path)?;
    Ok(package)
}

fn apply_metadata_installed_target(
    package: &mut ResolvedPackage,
    metadata: &PackageDirectoryMetadata,
    path: &Path,
) -> Result<(), LuaPackageError> {
    let metadata_target = metadata_installed_target(metadata);
    if package.installed.is_empty() {
        package.installed.push(metadata_target);
        return Ok(());
    }
    if package.installed == [metadata_target] {
        return Ok(());
    }
    Err(LuaPackageError::Domain {
        path: path.to_path_buf(),
        message: "field 'installed' must match package metadata identity".to_owned(),
    })
}

fn metadata_installed_target(metadata: &PackageDirectoryMetadata) -> InstalledTarget {
    match &metadata.package {
        PackageTypeMetadata::AndroidApp { android } => InstalledTarget::AndroidPackage {
            package_name: android.package_name.clone(),
        },
        PackageTypeMetadata::MagiskModule { magisk } => InstalledTarget::MagiskModule {
            module_id: magisk.module_id.clone(),
        },
        PackageTypeMetadata::Generic { generic } => InstalledTarget::Generic {
            id: generic.id.clone(),
        },
    }
}

fn metadata_permissions_for_script(
    metadata: &PackageDirectoryMetadata,
    file_name: &str,
) -> PackagePermissions {
    PackagePermissions {
        free_network: metadata
            .permissions_for(file_name)
            .contains(&PackageLuaPermission::AllowFreeNetwork),
    }
}

fn package_from_version_json(
    repository: RepositoryId,
    id: PackageId,
    name: String,
    default_permissions: PackagePermissions,
    path: &Path,
    object: &Map<String, JsonValue>,
) -> Result<ResolvedPackage, LuaPackageError> {
    let installed = parse_installed_targets(path, object.get("installed"))?;
    let permissions = match object.get("permissions") {
        Some(value) => parse_permissions(path, Some(value))?,
        None => default_permissions,
    };
    let source_priority =
        parse_string_array(path, "source_priority", object.get("source_priority"))?;
    let updates = parse_update_candidates(path, object.get("updates"))?;

    Ok(ResolvedPackage {
        id,
        repository,
        name,
        installed,
        permissions,
        source_priority,
        updates,
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

fn parse_update_candidates(
    path: &Path,
    value: Option<&JsonValue>,
) -> Result<Vec<UpdateCandidate>, LuaPackageError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: "field 'updates' must be an array".to_owned(),
    })?;
    array
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| LuaPackageError::Schema {
                path: path.to_path_buf(),
                message: "updates entries must be objects".to_owned(),
            })?;
            let artifacts = parse_update_artifacts(path, object.get("artifacts"))?;
            Ok(UpdateCandidate {
                version: required_string(path, object, "version")?.to_owned(),
                version_code: optional_i64(path, object, "version_code")?,
                channel: optional_string(object, "channel"),
                source: optional_string(object, "source"),
                artifacts,
            })
        })
        .collect()
}

fn parse_update_artifacts(
    path: &Path,
    value: Option<&JsonValue>,
) -> Result<Vec<UpdateArtifact>, LuaPackageError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| LuaPackageError::Schema {
        path: path.to_path_buf(),
        message: "field 'artifacts' must be an array".to_owned(),
    })?;
    array
        .iter()
        .map(|item| {
            let object = item.as_object().ok_or_else(|| LuaPackageError::Schema {
                path: path.to_path_buf(),
                message: "artifact entries must be objects".to_owned(),
            })?;
            Ok(UpdateArtifact {
                name: required_string(path, object, "name")?.to_owned(),
                url: required_string(path, object, "url")?.to_owned(),
                file_name: optional_string(object, "file_name"),
                sha256: optional_string(object, "sha256"),
                size: optional_u64(path, object, "size")?,
            })
        })
        .collect()
}

fn optional_string(object: &Map<String, JsonValue>, field: &str) -> Option<String> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

fn optional_i64(
    path: &Path,
    object: &Map<String, JsonValue>,
    field: &str,
) -> Result<Option<i64>, LuaPackageError> {
    object
        .get(field)
        .map(|value| {
            value.as_i64().ok_or_else(|| LuaPackageError::Schema {
                path: path.to_path_buf(),
                message: format!("field '{field}' must be an integer"),
            })
        })
        .transpose()
}

fn optional_u64(
    path: &Path,
    object: &Map<String, JsonValue>,
    field: &str,
) -> Result<Option<u64>, LuaPackageError> {
    object
        .get(field)
        .map(|value| {
            value.as_u64().ok_or_else(|| LuaPackageError::Schema {
                path: path.to_path_buf(),
                message: format!("field '{field}' must be an unsigned integer"),
            })
        })
        .transpose()
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
    use crate::repository::RepositoryPackageDirectoryLayout;
    use crate::RepositoryId;
    use std::fs;

    fn evaluate_single_package_directory(
        root: &Path,
        repository_id: &str,
    ) -> Result<ResolvedPackage, LuaPackageError> {
        let layout = RepositoryPackageDirectoryLayout::load(root).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();
        evaluate_package_directory_script(
            &RepositoryId::new(repository_id).unwrap(),
            package,
            &metadata,
            script,
        )
    }

    #[test]
    fn evaluates_json_like_lua_package_table() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "F-Droid",
  "android": { "package_name": "org.fdroid.fdroid" },
  "lua": { "9999.lua": { "permission": ["allow_free_network"] } }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version {
  installed = {
    { kind = "android_package", package_name = "org.fdroid.fdroid" },
  },
  source_priority = { "github", "fdroid" },
  updates = {
    {
      version = "1.2.0",
      version_code = 120,
      channel = "stable",
      source = "fixture",
      artifacts = {
        {
          name = "app.apk",
          url = "https://example.invalid/app.apk",
          file_name = "fdroid.apk",
          sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          size = 12345,
        },
      },
    },
  },
}
"#,
        )
        .unwrap();

        let package = evaluate_single_package_directory(temp.path(), "official").unwrap();
        assert_eq!(package.id.to_string(), "android/app/org.fdroid.fdroid");
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
        assert_eq!(package.updates.len(), 1);
        assert_eq!(package.updates[0].version, "1.2.0");
        assert_eq!(package.updates[0].version_code, Some(120));
        assert_eq!(package.updates[0].channel.as_deref(), Some("stable"));
        assert_eq!(package.updates[0].source.as_deref(), Some("fixture"));
        assert_eq!(
            package.updates[0].artifacts[0].file_name.as_deref(),
            Some("fdroid.apk")
        );
        assert_eq!(
            package.updates[0].artifacts[0].sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(package.updates[0].artifacts[0].size, Some(12345));
    }

    #[test]
    fn evaluates_package_directory_version_script_from_metadata_identity() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" },
  "lua": { "9999.lua": { "permission": ["allow_free_network"] } }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version {
  installed = {
    { kind = "android_package", package_name = "com.example.autogen" },
  },
  updates = {
    {
      version = "1.2.0",
      artifacts = {
        { name = "app.apk", url = "https://example.invalid/app.apk", file_name = "app.apk" },
      },
    },
  },
}
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let package = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap();

        assert_eq!(package.id.to_string(), "android/app/com.example.autogen");
        assert_eq!(package.repository.as_str(), "autogen");
        assert_eq!(package.name, "Example Autogen");
        assert!(package.permissions.free_network);
        assert_eq!(package.installed.len(), 1);
        assert_eq!(package.updates[0].version, "1.2.0");
    }

    #[test]
    fn rejects_package_directory_installed_target_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version {
  installed = {
    { kind = "android_package", package_name = "com.other.app" },
  },
}
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let err = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap_err();

        assert!(matches!(err, LuaPackageError::Domain { .. }));
        assert!(err.to_string().contains("metadata identity"));
    }

    #[test]
    fn rejects_package_directory_version_script_id_field() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version { id = "android/app/com.example.autogen" }
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let err = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap_err();

        assert!(matches!(err, LuaPackageError::Schema { .. }));
        assert!(err.to_string().contains("must not be declared"));
    }

    #[test]
    fn package_directory_can_load_luaclass_modules() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(&package_dir).unwrap();
        fs::create_dir_all(temp.path().join("luaclass")).unwrap();
        fs::write(
            temp.path().join("luaclass/android.lua"),
            r#"
return {
  package_version = function(input)
    return {
      installed = input.installed,
      updates = input.updates,
    }
  end
}
"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local android = require("luaclass.android")
return android.package_version {
  installed = {
    { kind = "android_package", package_name = "com.example.autogen" },
  },
}
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let package = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap();

        assert_eq!(package.name, "Example Autogen");
        assert_eq!(package.installed.len(), 1);
    }

    #[test]
    fn package_directory_can_use_repository_local_fdroid_luaclass_shape() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/f-droid/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::create_dir_all(temp.path().join("luaclass")).unwrap();
        fs::write(
            temp.path().join("luaclass/fdroid_android.lua"),
            r#"
local fdroid = {}

function fdroid.package(spec)
  if spec.package_name ~= "org.fdroid.fdroid" then
    error("F-Droid package_name fixture mismatch")
  end
  return package_version {
    updates = {
      {
        version = "1.20.0",
        version_code = 1020000,
        source = "fdroid",
        artifacts = {
          {
            name = "org.fdroid.fdroid_1020000.apk",
            url = "https://f-droid.org/repo/org.fdroid.fdroid_1020000.apk",
            file_name = "org.fdroid.fdroid_1020000.apk",
          },
        },
      },
    },
  }
end

return fdroid
"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local fdroid = require("luaclass.fdroid_android")
return fdroid.package {
  package_name = "org.fdroid.fdroid",
}
"#,
        )
        .unwrap();

        let package = evaluate_single_package_directory(temp.path(), "official").unwrap();

        assert_eq!(
            package.id.to_string(),
            "android/f-droid/app/org.fdroid.fdroid"
        );
        assert_eq!(package.repository.as_str(), "official");
        assert_eq!(package.name, "android/f-droid/app/org.fdroid.fdroid");
        assert_eq!(
            package.installed,
            vec![InstalledTarget::AndroidPackage {
                package_name: "org.fdroid.fdroid".to_owned()
            }]
        );
        assert_eq!(package.updates.len(), 1);
        assert_eq!(package.updates[0].version, "1.20.0");
        assert_eq!(package.updates[0].version_code, Some(1020000));
        assert_eq!(package.updates[0].source.as_deref(), Some("fdroid"));
        assert_eq!(
            package.updates[0].artifacts[0].file_name.as_deref(),
            Some("org.fdroid.fdroid_1020000.apk")
        );
    }

    #[test]
    fn package_directory_can_read_package_local_files() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(package_dir.join("files/nested")).unwrap();
        fs::write(package_dir.join("files/nested/data.txt"), b"hello\xff").unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local body = read_package_file("nested/data.txt")
return package_version { name = "bytes:" .. tostring(#body) }
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let package = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap();

        assert_eq!(package.name, "bytes:6");
    }

    #[test]
    fn read_package_file_rejects_paths_outside_package_files() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(package_dir.join("files")).unwrap();
        fs::write(package_dir.join("outside.txt"), "outside").unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local ok = pcall(read_package_file, "../outside.txt")
return package_version { name = ok and "leaked" or "blocked" }
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let package = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap();

        assert_eq!(package.name, "blocked");
    }

    #[test]
    fn getter_builtin_exposes_package_file_reader_to_package_directory_lua() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/com.example.autogen");
        fs::create_dir_all(package_dir.join("files")).unwrap();
        fs::write(package_dir.join("files/name.txt"), "from builtin").unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "Example Autogen",
  "android": { "package_name": "com.example.autogen" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
return package_version { name = getter_builtin.read_package_file("name.txt") }
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package = &layout.packages[0];
        let metadata = layout.package_metadata(package).unwrap();
        let script = layout.unambiguous_version_script(package).unwrap();

        let package = evaluate_package_directory_script(
            &RepositoryId::new("autogen").unwrap(),
            package,
            &metadata,
            script,
        )
        .unwrap();

        assert_eq!(package.name, "from builtin");
    }

    #[test]
    fn lua_environment_does_not_expose_process_or_file_system_globals() {
        let temp = tempfile::tempdir().unwrap();
        let side_effect = temp.path().join("side-effect");
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "F-Droid",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            format!(
                r#"#!/bin/upa-lua v1
return package_version {{
  name = (os == nil and io == nil and debug == nil and dofile == nil and loadfile == nil and package == nil) and "F-Droid" or "leaked",
  side_effect = rawget(_G, "os") and os.execute("touch {}"),
}}
"#,
                side_effect.display()
            ),
        )
        .unwrap();

        let package = evaluate_single_package_directory(temp.path(), "official").unwrap();
        assert_eq!(package.name, "F-Droid");
        assert!(!side_effect.exists());
    }

    #[test]
    fn repository_root_is_not_exposed_even_when_cwd_is_repository_root() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("rootleak.lua"),
            r#"return { leaked = true }"#,
        )
        .unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("metadata.jsonc"),
            r#"{
  "type": "android:app",
  "display_name": "F-Droid",
  "android": { "package_name": "org.fdroid.fdroid" }
}"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("9999.lua"),
            r#"#!/bin/upa-lua v1
local root_ok = pcall(require, "rootleak")
local template_ok = pcall(require, "templates.android")
return package_version { name = (root_ok or template_ok) and "leaked" or "F-Droid" }
"#,
        )
        .unwrap();

        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        let result = evaluate_single_package_directory(temp.path(), "official");
        std::env::set_current_dir(original_cwd).unwrap();

        let package = result.unwrap();
        assert_eq!(package.name, "F-Droid");
    }
}
