//! Minimal Lua package-file evaluation and Rust validation boundary.

use crate::repository::{
    PackageDirectory, PackageDirectoryMetadata, PackageLuaPermission, PackageTypeMetadata,
    PackageVersionScript, RepositoryLoadError, LUA_API_SHEBANG_V1, REPOSITORY_LUACLASS_DIR,
};
use crate::{
    InstalledTarget, PackageId, PackagePermissions, RepositoryId, ResolvedPackage, UpdateArtifact,
    UpdateCandidate,
};
use mlua::{Function, Lua, Table, Value};
use serde_json::{Map, Number, Value as JsonValue};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const BUILTIN_LUACLASS_MODULES: &[(&str, &str)] = &[
    ("android", include_str!("luaclass/android.lua")),
    #[cfg(feature = "provider-luaclass-dev")]
    (
        "fdroid_android",
        include_str!("luaclass/fdroid_android.lua"),
    ),
    #[cfg(feature = "provider-luaclass-dev")]
    (
        "github_android_apk",
        include_str!("luaclass/github_android_apk.lua"),
    ),
];

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
    evaluate_package_directory_script_with_host_bindings(
        repository_id,
        package,
        metadata,
        script,
        |_| Ok(()),
    )
}

/// Evaluates a package version script with caller-installed Lua host bindings.
///
/// This is an internal seam for higher-level getter operations to provide
/// getter-owned host functions while keeping `getter-core` independent from
/// provider/cache crates. It does not define a stable public Lua provider API.
pub fn evaluate_package_directory_script_with_host_bindings<F>(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
    install_host_bindings: F,
) -> Result<ResolvedPackage, LuaPackageError>
where
    F: FnOnce(&Lua) -> mlua::Result<()>,
{
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
        install_host_bindings,
    )?;
    validate_package_directory_version_json(repository_id, package, metadata, script, json)
}

fn evaluate_package_source_to_json<F>(
    environment: &LuaRepositoryEnvironment<'_>,
    path: &Path,
    source: &str,
    install_host_bindings: F,
) -> Result<JsonValue, LuaPackageError>
where
    F: FnOnce(&Lua) -> mlua::Result<()>,
{
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
    install_host_bindings(&lua).map_err(|source| LuaPackageError::Runtime {
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
            install_builtin_luaclass_searcher(lua, &package)?;
            install_repository_luaclass_searcher(
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

fn install_repository_luaclass_searcher(
    lua: &Lua,
    package: &Table,
    luaclass_dir: PathBuf,
) -> mlua::Result<()> {
    install_searcher(
        lua,
        package,
        lua.create_function(move |lua, module: String| {
            let Some(module) = module.strip_prefix("luaclass.") else {
                return lua
                    .create_string("\n\tconstrained repository luaclass searcher only handles luaclass.* modules")
                    .map(Value::String);
            };

            let Some(relative_module) = module_to_relative_path(module) else {
                return lua
                    .create_string(format!("\n\tinvalid repository module name 'luaclass.{module}'"))
                    .map(Value::String);
            };

            let module_path = luaclass_dir.join(&relative_module).with_extension("lua");
            let init_path = luaclass_dir.join(&relative_module).join("init.lua");
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
                "\n\tno repository module 'luaclass.{module}' in {}",
                luaclass_dir.display()
            ))
            .map(Value::String)
        })?,
    )
}

fn install_builtin_luaclass_searcher(lua: &Lua, package: &Table) -> mlua::Result<()> {
    install_searcher(
        lua,
        package,
        lua.create_function(move |lua, module: String| {
            let Some(module) = module.strip_prefix("luaclass.") else {
                return lua
                    .create_string("\n\tbuiltin luaclass searcher only handles luaclass.* modules")
                    .map(Value::String);
            };
            if module_to_relative_path(module).is_none() {
                return lua
                    .create_string(format!(
                        "\n\tinvalid builtin luaclass module name 'luaclass.{module}'"
                    ))
                    .map(Value::String);
            }
            let Some((_, source)) = BUILTIN_LUACLASS_MODULES
                .iter()
                .find(|(name, _)| *name == module)
            else {
                return lua
                    .create_string(format!(
                        "\n\tno builtin luaclass module 'luaclass.{module}'"
                    ))
                    .map(Value::String);
            };
            lua.load(*source)
                .set_name(format!("@builtin/luaclass/{module}.lua"))
                .into_function()
                .map(Value::Function)
        })?,
    )
}

fn install_searcher(_lua: &Lua, package: &Table, searcher: Function) -> mlua::Result<()> {
    let searchers = package_searchers(package)?;
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
    let getter_builtin = getter_builtin_table(lua)?;
    getter_builtin.set("read_package_file", read_package_file.clone())?;
    globals.set("read_package_file", read_package_file)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaHttpGetRequest {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub cache: bool,
}

/// Installs the Lua `http_get` host seam for a caller-provided transport.
///
/// Plain package evaluation does not install this function. Higher-level
/// getter operations/runtime code must deliberately provide a handler and own
/// provider policy, permission checks, Manifest validation, and cache behavior.
pub fn install_http_get_host<F>(lua: &Lua, handler: F) -> mlua::Result<()>
where
    F: Fn(LuaHttpGetRequest) -> mlua::Result<Vec<u8>> + 'static,
{
    let http_get = lua.create_function(move |lua, args: mlua::Variadic<Value>| {
        let request = lua_http_get_request_from_args(args)?;
        let bytes = handler(request)?;
        lua.create_string(&bytes)
    })?;
    let globals = lua.globals();
    let getter_builtin = getter_builtin_table(lua)?;
    getter_builtin.set("http_get", http_get.clone())?;
    globals.set("http_get", http_get)
}

fn getter_builtin_table(lua: &Lua) -> mlua::Result<Table> {
    let globals = lua.globals();
    match globals.get::<Value>("getter_builtin")? {
        Value::Table(table) => Ok(table),
        Value::Nil => {
            let table = lua.create_table()?;
            globals.set("getter_builtin", table.clone())?;
            Ok(table)
        }
        _ => Err(mlua::Error::external("getter_builtin must be a table")),
    }
}

fn lua_http_get_request_from_args(args: mlua::Variadic<Value>) -> mlua::Result<LuaHttpGetRequest> {
    if args.is_empty() || args.len() > 2 {
        return Err(mlua::Error::external(
            "http_get expects url and optional options table",
        ));
    }
    let url = match &args[0] {
        Value::String(value) => value.to_str()?.to_owned(),
        _ => return Err(mlua::Error::external("http_get url must be a string")),
    };
    if url.trim().is_empty() {
        return Err(mlua::Error::external("http_get url must not be empty"));
    }
    let mut request = LuaHttpGetRequest {
        url,
        headers: BTreeMap::new(),
        cache: false,
    };
    match args.get(1) {
        None | Some(Value::Nil) => {}
        Some(Value::Table(options)) => apply_http_get_options(&mut request, options)?,
        Some(_) => return Err(mlua::Error::external("http_get options must be a table")),
    }
    Ok(request)
}

fn apply_http_get_options(request: &mut LuaHttpGetRequest, options: &Table) -> mlua::Result<()> {
    for pair in options.clone().pairs::<Value, Value>() {
        let (key, _) = pair?;
        match key {
            Value::String(value) if matches!(value.to_str()?.as_ref(), "cache" | "headers") => {}
            Value::String(value) => {
                return Err(mlua::Error::external(format!(
                    "http_get unsupported option '{}'",
                    value.to_str()?
                )))
            }
            _ => {
                return Err(mlua::Error::external(
                    "http_get option keys must be strings",
                ))
            }
        }
    }

    match options.get::<Value>("cache")? {
        Value::Nil => {}
        Value::Boolean(value) => request.cache = value,
        _ => {
            return Err(mlua::Error::external(
                "http_get options.cache must be a boolean",
            ))
        }
    }

    match options.get::<Value>("headers")? {
        Value::Nil => {}
        Value::Table(headers) => {
            for pair in headers.pairs::<Value, Value>() {
                let (key, value) = pair?;
                let key = match key {
                    Value::String(value) => value.to_str()?.to_owned(),
                    _ => {
                        return Err(mlua::Error::external(
                            "http_get headers keys must be strings",
                        ))
                    }
                };
                let value = match value {
                    Value::String(value) => value.to_str()?.to_owned(),
                    _ => {
                        return Err(mlua::Error::external(
                            "http_get headers values must be strings",
                        ))
                    }
                };
                request.headers.insert(key, value);
            }
        }
        _ => {
            return Err(mlua::Error::external(
                "http_get options.headers must be a table",
            ))
        }
    }
    Ok(())
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

    fn write_simple_android_package(root: &Path, script_source: &str) {
        let package_dir = root.join("android/app/com.example.autogen");
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
        fs::write(package_dir.join("9999.lua"), script_source).unwrap();
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
    fn package_directory_can_load_repository_luaclass_modules() {
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
      name = "repository module",
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

        assert_eq!(package.name, "repository module");
        assert_eq!(package.installed.len(), 1);
    }

    #[test]
    fn package_directory_can_load_builtin_luaclass_modules() {
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
local android = require("luaclass.android")
return android.package_version {
  name = "builtin module",
  installed = {
    { kind = "android_package", package_name = "com.example.autogen" },
  },
}
"#,
        )
        .unwrap();

        let package = evaluate_single_package_directory(temp.path(), "official").unwrap();

        assert_eq!(package.name, "builtin module");
        assert_eq!(package.installed.len(), 1);
    }

    #[cfg(not(feature = "provider-luaclass-dev"))]
    #[test]
    fn provider_luaclass_dev_modules_are_not_default_builtins() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/f-droid/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
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
return fdroid.package { package_name = "org.fdroid.fdroid" }
"#,
        )
        .unwrap();

        let err = evaluate_single_package_directory(temp.path(), "official").unwrap_err();

        let message = err.to_string();
        assert!(matches!(err, LuaPackageError::Runtime { .. }));
        assert!(message.contains("no builtin luaclass module 'luaclass.fdroid_android'"));
    }

    #[cfg(feature = "provider-luaclass-dev")]
    #[test]
    fn provider_luaclass_dev_builtin_can_call_injected_host() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/f-droid/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
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
return fdroid.package { package_name = "org.fdroid.fdroid" }
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package_dir = &layout.packages[0];
        let metadata = layout.package_metadata(package_dir).unwrap();
        let script = layout.unambiguous_version_script(package_dir).unwrap();

        let package = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("official").unwrap(),
            package_dir,
            &metadata,
            script,
            |lua| {
                let getter_dev = lua.create_table()?;
                getter_dev.set(
                    "fdroid_update_candidates",
                    lua.create_function(|lua, spec: Table| {
                        let package_name: String = spec.get("package_name")?;
                        if package_name != "org.fdroid.fdroid" {
                            return Err(mlua::Error::external("F-Droid package mismatch"));
                        }
                        let artifact = lua.create_table()?;
                        artifact.set("name", "org.fdroid.fdroid.apk")?;
                        artifact.set("url", "https://f-droid.org/repo/org.fdroid.fdroid.apk")?;
                        let artifacts = lua.create_table()?;
                        artifacts.raw_set(1, artifact)?;
                        let candidate = lua.create_table()?;
                        candidate.set("version", "1.20.0")?;
                        candidate.set("source", "fdroid")?;
                        candidate.set("artifacts", artifacts)?;
                        let candidates = lua.create_table()?;
                        candidates.raw_set(1, candidate)?;
                        Ok(candidates)
                    })?,
                )?;
                lua.globals().set("getter_dev", getter_dev)
            },
        )
        .unwrap();

        assert_eq!(package.name, "android/f-droid/app/org.fdroid.fdroid");
        assert_eq!(package.source_priority, vec!["fdroid"]);
        assert_eq!(package.updates[0].version, "1.20.0");
        assert_eq!(package.updates[0].source.as_deref(), Some("fdroid"));
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
    fn package_directory_can_use_repository_local_github_android_luaclass_shape() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::create_dir_all(temp.path().join("luaclass")).unwrap();
        fs::write(
            temp.path().join("luaclass/github_android_apk.lua"),
            r#"
local github_android = {}

function github_android.package(spec)
  if spec.owner ~= "f-droid" or spec.repo ~= "fdroidclient" then
    error("GitHub project fixture mismatch")
  end
  local asset_name = "F-Droid.apk"
  if spec.asset.include and not string.match(asset_name, spec.asset.include) then
    error("GitHub asset include fixture mismatch")
  end
  if spec.asset.exclude and string.match(asset_name, spec.asset.exclude) then
    error("GitHub asset exclude fixture mismatch")
  end
  return package_version {
    name = spec.name,
    installed = {
      { kind = "android_package", package_name = spec.android_package },
    },
    source_priority = { "github" },
    updates = {
      {
        version = "v1.20.0",
        source = "github",
        artifacts = {
          {
            name = asset_name,
            url = "https://github.com/f-droid/fdroidclient/releases/download/v1.20.0/F-Droid.apk",
            file_name = asset_name,
          },
        },
      },
    },
  }
end

return github_android
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
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name = "F-Droid",
  android_package = "org.fdroid.fdroid",
  owner = "f-droid",
  repo = "fdroidclient",
  asset = {
    include = "%.apk$",
    exclude = "debug",
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
        assert_eq!(package.source_priority, vec!["github"]);
        assert_eq!(package.updates.len(), 1);
        assert_eq!(package.updates[0].version, "v1.20.0");
        assert_eq!(package.updates[0].source.as_deref(), Some("github"));
        assert_eq!(
            package.updates[0].artifacts[0].url,
            "https://github.com/f-droid/fdroidclient/releases/download/v1.20.0/F-Droid.apk"
        );
        assert_eq!(
            package.updates[0].artifacts[0].file_name.as_deref(),
            Some("F-Droid.apk")
        );
    }

    #[test]
    fn package_directory_luaclass_can_call_injected_provider_host() {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.fdroid.fdroid");
        fs::create_dir_all(&package_dir).unwrap();
        fs::create_dir_all(temp.path().join("luaclass")).unwrap();
        fs::write(
            temp.path().join("luaclass/github_android_apk.lua"),
            r#"
local github_android = {}

function github_android.package(spec)
  local releases = getter_test_provider.github_releases(spec.owner, spec.repo, spec.asset)
  return package_version {
    name = spec.name,
    source_priority = { "github" },
    updates = releases,
  }
end

return github_android
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
local github_android = require("luaclass.github_android_apk")
return github_android.package {
  name = "F-Droid",
  owner = "f-droid",
  repo = "fdroidclient",
  asset = { include = "%.apk$" },
}
"#,
        )
        .unwrap();
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package_dir = &layout.packages[0];
        let metadata = layout.package_metadata(package_dir).unwrap();
        let script = layout.unambiguous_version_script(package_dir).unwrap();

        let package = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("official").unwrap(),
            package_dir,
            &metadata,
            script,
            |lua| {
                let provider = lua.create_table()?;
                provider.set(
                    "github_releases",
                    lua.create_function(
                        |lua, (owner, repo, asset): (String, String, Table)| {
                            if owner != "f-droid" || repo != "fdroidclient" {
                                return Err(mlua::Error::external("GitHub fixture mismatch"));
                            }
                            let include: String = asset.get("include")?;
                            if include != "%.apk$" {
                                return Err(mlua::Error::external("GitHub asset filter mismatch"));
                            }
                            let artifact = lua.create_table()?;
                            artifact.set("name", "F-Droid.apk")?;
                            artifact.set(
                                "url",
                                "https://github.com/f-droid/fdroidclient/releases/download/v1.20.0/F-Droid.apk",
                            )?;
                            artifact.set("file_name", "F-Droid.apk")?;
                            let artifacts = lua.create_table()?;
                            artifacts.raw_set(1, artifact)?;
                            let candidate = lua.create_table()?;
                            candidate.set("version", "v1.20.0")?;
                            candidate.set("source", "github")?;
                            candidate.set("artifacts", artifacts)?;
                            let candidates = lua.create_table()?;
                            candidates.raw_set(1, candidate)?;
                            Ok(candidates)
                        },
                    )?,
                )?;
                lua.globals().set("getter_test_provider", provider)
            },
        )
        .unwrap();

        assert_eq!(package.id.to_string(), "android/app/org.fdroid.fdroid");
        assert_eq!(package.repository.as_str(), "official");
        assert_eq!(package.name, "F-Droid");
        assert_eq!(
            package.installed,
            vec![InstalledTarget::AndroidPackage {
                package_name: "org.fdroid.fdroid".to_owned()
            }]
        );
        assert_eq!(package.source_priority, vec!["github"]);
        assert_eq!(package.updates.len(), 1);
        assert_eq!(package.updates[0].version, "v1.20.0");
        assert_eq!(package.updates[0].source.as_deref(), Some("github"));
        assert_eq!(
            package.updates[0].artifacts[0].file_name.as_deref(),
            Some("F-Droid.apk")
        );
    }

    #[test]
    fn package_directory_http_get_is_not_installed_by_plain_eval() {
        let temp = tempfile::tempdir().unwrap();
        write_simple_android_package(
            temp.path(),
            r#"#!/bin/upa-lua v1
local builtin_http_get = getter_builtin and getter_builtin.http_get
return package_version { name = (http_get == nil and builtin_http_get == nil) and "not installed" or "installed" }
"#,
        );

        let package = evaluate_single_package_directory(temp.path(), "official").unwrap();

        assert_eq!(package.name, "not installed");
    }

    #[test]
    fn package_directory_http_get_host_parses_request_and_defaults_cache_false() {
        let temp = tempfile::tempdir().unwrap();
        write_simple_android_package(
            temp.path(),
            r#"#!/bin/upa-lua v1
local first = http_get("https://example.invalid/a")
local second = http_get("https://example.invalid/b", {
  headers = { Accept = "application/json", ["X-Test"] = "yes" },
  cache = true,
})
return package_version { name = first .. "|" .. second }
"#,
        );
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package_dir = &layout.packages[0];
        let metadata = layout.package_metadata(package_dir).unwrap();
        let script = layout.unambiguous_version_script(package_dir).unwrap();
        let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        let package = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("autogen").unwrap(),
            package_dir,
            &metadata,
            script,
            {
                let requests = std::rc::Rc::clone(&requests);
                move |lua| {
                    install_http_get_host(lua, move |request| {
                        let body = if request.url.ends_with("/a") {
                            b"first".to_vec()
                        } else {
                            b"second".to_vec()
                        };
                        requests.borrow_mut().push(request);
                        Ok(body)
                    })
                }
            },
        )
        .unwrap();

        assert_eq!(package.name, "first|second");
        let requests = requests.borrow();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url, "https://example.invalid/a");
        assert!(!requests[0].cache);
        assert!(requests[0].headers.is_empty());
        assert_eq!(requests[1].url, "https://example.invalid/b");
        assert!(requests[1].cache);
        assert_eq!(
            requests[1].headers.get("Accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            requests[1].headers.get("X-Test").map(String::as_str),
            Some("yes")
        );
    }

    #[test]
    fn package_directory_http_get_rejects_unsupported_options() {
        let temp = tempfile::tempdir().unwrap();
        write_simple_android_package(
            temp.path(),
            r#"#!/bin/upa-lua v1
local extra_ok = pcall(http_get, "https://example.invalid/a", { method = "POST" })
local header_ok = pcall(http_get, "https://example.invalid/b", { headers = { Accept = 1 } })
return package_version { name = (not extra_ok and not header_ok) and "rejected" or "accepted" }
"#,
        );
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package_dir = &layout.packages[0];
        let metadata = layout.package_metadata(package_dir).unwrap();
        let script = layout.unambiguous_version_script(package_dir).unwrap();

        let package = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("autogen").unwrap(),
            package_dir,
            &metadata,
            script,
            |lua| install_http_get_host(lua, |_| Ok(Vec::new())),
        )
        .unwrap();

        assert_eq!(package.name, "rejected");
    }

    #[test]
    fn getter_builtin_exposes_http_get_for_lua_wrappers() {
        let temp = tempfile::tempdir().unwrap();
        write_simple_android_package(
            temp.path(),
            r#"#!/bin/upa-lua v1
local upstream_http_get = getter_builtin.http_get
function http_get(url, opts)
  opts = opts or {}
  opts.headers = opts.headers or {}
  opts.headers["X-Rewritten"] = "yes"
  return upstream_http_get(url .. "?mirror=1", opts)
end
return package_version { name = http_get("https://example.invalid/file", { cache = true }) }
"#,
        );
        let layout = RepositoryPackageDirectoryLayout::load(temp.path()).unwrap();
        let package_dir = &layout.packages[0];
        let metadata = layout.package_metadata(package_dir).unwrap();
        let script = layout.unambiguous_version_script(package_dir).unwrap();
        let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        let package = evaluate_package_directory_script_with_host_bindings(
            &RepositoryId::new("autogen").unwrap(),
            package_dir,
            &metadata,
            script,
            {
                let requests = std::rc::Rc::clone(&requests);
                move |lua| {
                    install_http_get_host(lua, move |request| {
                        requests.borrow_mut().push(request);
                        Ok(b"wrapped".to_vec())
                    })
                }
            },
        )
        .unwrap();

        assert_eq!(package.name, "wrapped");
        let requests = requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, "https://example.invalid/file?mirror=1");
        assert!(requests[0].cache);
        assert_eq!(
            requests[0].headers.get("X-Rewritten").map(String::as_str),
            Some("yes")
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
