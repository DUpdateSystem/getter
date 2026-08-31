//! Operation-owned Lua HTTP policy harness.
//!
//! This module installs getter-core's `http_get` seam for package evaluation
//! using fixture/in-memory responses. It proves the operation/runtime side owns
//! permission and package Manifest policy while `getter-core` remains transport
//! and storage agnostic. It intentionally does not perform live HTTP, cache
//! persistence/revalidation, auth, or public CLI/native exposure.

use crate::lua_runtime_hooks::load_runtime_hooks;
use getter_core::lua::{
    evaluate_package_directory_script_with_host_bindings, install_http_get_host, LuaHttpGetRequest,
    LuaPackageError,
};
use getter_core::repository::{
    PackageDirectory, PackageDirectoryMetadata, PackageLuaPermission, PackageVersionScript,
    PACKAGE_MANIFEST_FILE,
};
use getter_core::{RepositoryId, ResolvedPackage};
use sha2::{Digest, Sha512};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const HTTP_RESPONSE_NOT_IN_MANIFEST: &str = "package.http.response_not_in_manifest";
pub const HTTP_FIXTURE_NOT_FOUND: &str = "package.http.fixture_not_found";

#[derive(Debug, thiserror::Error)]
pub enum LuaHttpPolicyError {
    #[error("failed to read package Manifest at {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid package Manifest at {path}: {reason}")]
    InvalidManifest { path: PathBuf, reason: String },
    #[error("package evaluation failed: {0}")]
    PackageEval(#[from] LuaPackageError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LuaHttpPolicyEvaluation {
    pub package: ResolvedPackage,
    pub http_requests: Vec<LuaHttpGetRequest>,
    pub runtime_hooks: Vec<PathBuf>,
}

/// Evaluates a package version script with fixture-backed `http_get`.
///
/// This is an internal harness for operation/runtime policy tests. The caller
/// supplies URL-to-body fixtures instead of a live transport. For scripts that
/// do not declare `allow_free_network`, every returned response body must have
/// a SHA-512 digest listed in the package `Manifest`. Missing `Manifest` is an
/// empty allow-list. Scripts that declare `allow_free_network` bypass Manifest
/// membership, but requests are still captured for diagnostics/validation.
pub fn evaluate_package_directory_script_with_fixture_http(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
    responses: BTreeMap<String, Vec<u8>>,
) -> Result<LuaHttpPolicyEvaluation, LuaHttpPolicyError> {
    evaluate_package_directory_script_with_fixture_http_and_runtime_hooks(
        repository_id,
        package,
        metadata,
        script,
        responses,
        None,
    )
}

/// Evaluates a package version script with fixture-backed `http_get` and
/// optional runtime hooks from `<data-dir>/rc/hook/*.lua`.
pub fn evaluate_package_directory_script_with_fixture_http_and_runtime_hooks(
    repository_id: &RepositoryId,
    package: &PackageDirectory,
    metadata: &PackageDirectoryMetadata,
    script: &PackageVersionScript,
    responses: BTreeMap<String, Vec<u8>>,
    data_dir: Option<&Path>,
) -> Result<LuaHttpPolicyEvaluation, LuaHttpPolicyError> {
    let allow_free_network = metadata
        .permissions_for(&script.file_name)
        .contains(&PackageLuaPermission::AllowFreeNetwork);
    let manifest = if allow_free_network {
        None
    } else {
        Some(PackageManifest::load(
            package.path.join(PACKAGE_MANIFEST_FILE),
        )?)
    };
    let responses = Rc::new(responses);
    let http_requests = Rc::new(RefCell::new(Vec::new()));
    let runtime_hooks = Rc::new(RefCell::new(Vec::new()));
    let data_dir = data_dir.map(Path::to_path_buf);

    let resolved = evaluate_package_directory_script_with_host_bindings(
        repository_id,
        package,
        metadata,
        script,
        {
            let http_requests = Rc::clone(&http_requests);
            let runtime_hooks = Rc::clone(&runtime_hooks);
            move |lua| {
                install_http_get_host(lua, {
                    let responses = Rc::clone(&responses);
                    let http_requests = Rc::clone(&http_requests);
                    let manifest = manifest.clone();
                    move |request| {
                        let body = responses.get(&request.url).cloned().ok_or_else(|| {
                            mlua::Error::external(format!(
                                "{HTTP_FIXTURE_NOT_FOUND}: no fixture response for {}",
                                request.url
                            ))
                        })?;
                        if let Some(manifest) = manifest.as_ref() {
                            let digest = sha512_hex(&body);
                            if !manifest.contains(&digest) {
                                return Err(mlua::Error::external(format!(
                                    "{HTTP_RESPONSE_NOT_IN_MANIFEST}: response body SHA-512 {digest} for {} is not listed in package Manifest",
                                    request.url
                                )));
                            }
                        }
                        http_requests.borrow_mut().push(request);
                        Ok(body)
                    }
                })?;
                if let Some(data_dir) = data_dir.as_deref() {
                    *runtime_hooks.borrow_mut() = load_runtime_hooks(lua, data_dir)?;
                }
                Ok(())
            }
        },
    )?;

    let http_requests = http_requests.borrow().clone();
    let runtime_hooks = runtime_hooks.borrow().clone();
    Ok(LuaHttpPolicyEvaluation {
        package: resolved,
        http_requests,
        runtime_hooks,
    })
}

#[derive(Debug, Clone)]
struct PackageManifest(getter_core::manifest::PackageManifest);

impl PackageManifest {
    fn load(path: impl AsRef<Path>) -> Result<Self, LuaHttpPolicyError> {
        let path = path.as_ref();
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(LuaHttpPolicyError::ReadManifest {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        getter_core::manifest::PackageManifest::parse(&source)
            .map(Self)
            .map_err(|source| LuaHttpPolicyError::InvalidManifest {
                path: path.to_path_buf(),
                reason: source.to_string(),
            })
    }

    fn contains(&self, digest: &str) -> bool {
        self.0.contains_sha512(digest)
    }
}

fn sha512_hex(body: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(body);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use getter_core::repository::{RepositoryPackageDirectoryLayout, LUA_API_SHEBANG_V1};
    use serde_json::json;
    use std::fs;

    const URL: &str = "https://example.invalid/body";

    #[test]
    fn non_free_network_script_accepts_manifest_listed_response_body() {
        let fixture = package_fixture(PackageFixture {
            body: b"manifest body",
            manifest_body: Some(b"manifest body"),
            allow_free_network: false,
            cache: false,
        });

        let result = fixture.evaluate().unwrap();

        assert_eq!(result.package.name, "manifest body");
        assert_eq!(result.http_requests.len(), 1);
        assert!(!result.http_requests[0].cache);
    }

    #[test]
    fn non_free_network_script_rejects_missing_manifest_response_body() {
        let fixture = package_fixture(PackageFixture {
            body: b"unlisted body",
            manifest_body: None,
            allow_free_network: false,
            cache: false,
        });

        let err = fixture.evaluate().unwrap_err();

        let message = err.to_string();
        assert!(matches!(err, LuaHttpPolicyError::PackageEval(_)));
        assert!(message.contains(HTTP_RESPONSE_NOT_IN_MANIFEST));
    }

    #[test]
    fn non_free_network_script_rejects_empty_manifest_response_body() {
        let fixture = package_fixture(PackageFixture {
            body: b"unlisted body",
            manifest_body: Some(b"unused"),
            allow_free_network: false,
            cache: false,
        });
        fs::write(fixture.package_dir.join(PACKAGE_MANIFEST_FILE), "").unwrap();

        let err = fixture.evaluate().unwrap_err();

        let message = err.to_string();
        assert!(matches!(err, LuaHttpPolicyError::PackageEval(_)));
        assert!(message.contains(HTTP_RESPONSE_NOT_IN_MANIFEST));
    }

    #[test]
    fn non_free_network_script_rejects_mismatched_manifest_response_body() {
        let fixture = package_fixture(PackageFixture {
            body: b"actual body",
            manifest_body: Some(b"different body"),
            allow_free_network: false,
            cache: false,
        });

        let err = fixture.evaluate().unwrap_err();

        let message = err.to_string();
        assert!(message.contains(HTTP_RESPONSE_NOT_IN_MANIFEST));
        assert!(message.contains(URL));
    }

    #[test]
    fn allow_free_network_script_bypasses_manifest_membership() {
        let fixture = package_fixture(PackageFixture {
            body: b"free network body",
            manifest_body: None,
            allow_free_network: true,
            cache: false,
        });

        let result = fixture.evaluate().unwrap();

        assert_eq!(result.package.name, "free network body");
        assert!(result.package.permissions.free_network);
    }

    #[test]
    fn cache_true_is_forwarded_without_cache_persistence_behavior() {
        let fixture = package_fixture(PackageFixture {
            body: b"cache body",
            manifest_body: Some(b"cache body"),
            allow_free_network: false,
            cache: true,
        });

        let result = fixture.evaluate().unwrap();

        assert_eq!(result.package.name, "cache body");
        assert_eq!(result.http_requests.len(), 1);
        assert!(result.http_requests[0].cache);
        assert_eq!(
            result.http_requests[0]
                .headers
                .get("Accept")
                .map(String::as_str),
            Some("text/plain")
        );
    }

    #[test]
    fn invalid_manifest_fails_before_lua_evaluation() {
        let fixture = package_fixture(PackageFixture {
            body: b"unused",
            manifest_body: Some(b"unused"),
            allow_free_network: false,
            cache: false,
        });
        fs::write(
            fixture.package_dir.join(PACKAGE_MANIFEST_FILE),
            "not-a-sha512 body",
        )
        .unwrap();

        let err = fixture.evaluate().unwrap_err();

        assert!(matches!(err, LuaHttpPolicyError::InvalidManifest { .. }));
        assert!(err.to_string().contains("line 1"));
    }

    #[test]
    fn missing_runtime_hook_directory_is_noop() {
        let fixture = package_fixture(PackageFixture {
            body: b"manifest body",
            manifest_body: Some(b"manifest body"),
            allow_free_network: false,
            cache: false,
        });

        let result = fixture.evaluate_with_hooks().unwrap();

        assert_eq!(result.package.name, "manifest body");
        assert!(result.runtime_hooks.is_empty());
    }

    #[test]
    fn dot_prefixed_runtime_hook_is_ignored() {
        let fixture = package_fixture(PackageFixture {
            body: b"manifest body",
            manifest_body: Some(b"manifest body"),
            allow_free_network: false,
            cache: false,
        });
        fixture.write_hook(".10-fail.lua", r#"error("ignored hook should not run")"#);

        let result = fixture.evaluate_with_hooks().unwrap();

        assert_eq!(result.package.name, "manifest body");
        assert!(result.runtime_hooks.is_empty());
    }

    #[test]
    fn runtime_hooks_load_in_deterministic_filename_order() {
        let fixture = package_fixture(PackageFixture {
            body: b"manifest body",
            manifest_body: Some(b"manifest body"),
            allow_free_network: false,
            cache: false,
        });
        fixture.write_hook("20-second.lua", r#"hook_order = (hook_order or "") .. "b""#);
        fixture.write_hook("10-first.lua", r#"hook_order = (hook_order or "") .. "a""#);

        let result = fixture.evaluate_with_hooks().unwrap();

        assert_eq!(result.package.name, "manifest body");
        let loaded = result
            .runtime_hooks
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(loaded, vec!["10-first.lua", "20-second.lua"]);
        assert_eq!(
            result.http_requests[0]
                .headers
                .get("X-Hook-Order")
                .map(String::as_str),
            Some("ab")
        );
    }

    #[test]
    fn runtime_hook_can_wrap_http_get_and_rewrite_request() {
        let mut fixture = package_fixture(PackageFixture {
            body: b"unused original body",
            manifest_body: Some(b"mirror body"),
            allow_free_network: false,
            cache: false,
        });
        fixture.add_response("https://mirror.invalid/body", b"mirror body");
        fixture.write_hook(
            "10-http-rewrite.lua",
            r#"
local original_http_get = getter_builtin.http_get
function http_get(url, options)
  options = options or {}
  options.headers = options.headers or {}
  options.headers["X-Original-Url"] = url
  options.cache = true
  return original_http_get("https://mirror.invalid/body", options)
end
"#,
        );

        let result = fixture.evaluate_with_hooks().unwrap();

        assert_eq!(result.package.name, "mirror body");
        assert_eq!(result.http_requests.len(), 1);
        assert_eq!(result.http_requests[0].url, "https://mirror.invalid/body");
        assert!(result.http_requests[0].cache);
        assert_eq!(
            result.http_requests[0]
                .headers
                .get("X-Original-Url")
                .map(String::as_str),
            Some(URL)
        );
    }

    #[test]
    fn runtime_hook_rewrite_still_enforces_manifest_on_returned_body() {
        let mut fixture = package_fixture(PackageFixture {
            body: b"unused original body",
            manifest_body: Some(b"unused original body"),
            allow_free_network: false,
            cache: false,
        });
        fixture.add_response("https://mirror.invalid/body", b"unlisted mirror body");
        fixture.write_hook(
            "10-http-rewrite.lua",
            r#"
local original_http_get = getter_builtin.http_get
function http_get(url, options)
  return original_http_get("https://mirror.invalid/body", options)
end
"#,
        );

        let err = fixture.evaluate_with_hooks().unwrap_err();

        let message = err.to_string();
        assert!(message.contains(HTTP_RESPONSE_NOT_IN_MANIFEST));
        assert!(message.contains("https://mirror.invalid/body"));
    }

    #[test]
    fn runtime_hook_failure_fails_closed() {
        let fixture = package_fixture(PackageFixture {
            body: b"manifest body",
            manifest_body: Some(b"manifest body"),
            allow_free_network: false,
            cache: false,
        });
        fixture.write_hook("10-fail.lua", r#"error("hook failed closed")"#);

        let err = fixture.evaluate_with_hooks().unwrap_err();

        let message = err.to_string();
        assert!(matches!(err, LuaHttpPolicyError::PackageEval(_)));
        assert!(message.contains("failed to execute runtime hook"));
        assert!(message.contains("hook failed closed"));
    }

    struct PackageFixture {
        body: &'static [u8],
        manifest_body: Option<&'static [u8]>,
        allow_free_network: bool,
        cache: bool,
    }

    struct WrittenPackageFixture {
        temp: tempfile::TempDir,
        package_dir: PathBuf,
        responses: BTreeMap<String, Vec<u8>>,
    }

    impl WrittenPackageFixture {
        fn evaluate(&self) -> Result<LuaHttpPolicyEvaluation, LuaHttpPolicyError> {
            self.evaluate_with_data_dir(None)
        }

        fn evaluate_with_hooks(&self) -> Result<LuaHttpPolicyEvaluation, LuaHttpPolicyError> {
            self.evaluate_with_data_dir(Some(self.temp.path()))
        }

        fn evaluate_with_data_dir(
            &self,
            data_dir: Option<&Path>,
        ) -> Result<LuaHttpPolicyEvaluation, LuaHttpPolicyError> {
            let layout = RepositoryPackageDirectoryLayout::load(self.temp.path()).unwrap();
            let package = layout
                .package(&"android/app/org.example".parse().unwrap())
                .unwrap();
            let metadata = layout.package_metadata(package).unwrap();
            let script = layout.unambiguous_version_script(package).unwrap();
            evaluate_package_directory_script_with_fixture_http_and_runtime_hooks(
                &RepositoryId::new("official").unwrap(),
                package,
                &metadata,
                script,
                self.responses.clone(),
                data_dir,
            )
        }

        fn write_hook(&self, file_name: &str, source: &str) {
            let hook_dir = self.temp.path().join("rc/hook");
            fs::create_dir_all(&hook_dir).unwrap();
            fs::write(hook_dir.join(file_name), source).unwrap();
        }

        fn add_response(&mut self, url: &str, body: &'static [u8]) {
            self.responses.insert(url.to_owned(), body.to_vec());
        }
    }

    fn package_fixture(fixture: PackageFixture) -> WrittenPackageFixture {
        let temp = tempfile::tempdir().unwrap();
        let package_dir = temp.path().join("android/app/org.example");
        fs::create_dir_all(&package_dir).unwrap();
        let mut metadata = json!({
            "type": "android:app",
            "android": { "package_name": "org.example" }
        });
        if fixture.allow_free_network {
            metadata["lua"] = json!({
                "9999.lua": { "permission": ["allow_free_network"] }
            });
        }
        fs::write(
            package_dir.join("metadata.jsonc"),
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();
        if let Some(manifest_body) = fixture.manifest_body {
            fs::write(
                package_dir.join(PACKAGE_MANIFEST_FILE),
                format!("{} fixture-body\n", sha512_hex(manifest_body)),
            )
            .unwrap();
        }
        fs::write(package_dir.join("9999.lua"), version_script(fixture.cache)).unwrap();
        let mut responses = BTreeMap::new();
        responses.insert(URL.to_owned(), fixture.body.to_vec());
        WrittenPackageFixture {
            temp,
            package_dir,
            responses,
        }
    }

    fn version_script(cache: bool) -> String {
        format!(
            r#"{LUA_API_SHEBANG_V1}
local body = http_get("{URL}", {{
  headers = {{
    Accept = "text/plain",
    ["X-Hook-Order"] = hook_order or "",
  }},
  cache = {cache},
}})
return package_version {{
  name = body,
  updates = {{
    {{
      version = "1.0.0",
      artifacts = {{
        {{ name = "fixture", url = "https://example.invalid/artifact" }},
      }},
    }},
  }},
}}
"#
        )
    }
}
