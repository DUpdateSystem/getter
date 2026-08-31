local github_android = {}

local function require_string(table, field)
  local value = table[field]
  if type(value) ~= "string" or value == "" then
    error("github_android.package requires string field '" .. field .. "'")
  end
  return value
end

local function optional_string(table, field)
  local value = table[field]
  if value == nil then
    return nil
  end
  if type(value) ~= "string" then
    error("github_android.package field '" .. field .. "' must be a string")
  end
  if value == "" then
    return nil
  end
  return value
end

local function optional_table(table, field)
  local value = table[field]
  if value == nil then
    return nil
  end
  if type(value) ~= "table" then
    error("github_android.package field '" .. field .. "' must be a table")
  end
  return value
end

local function optional_boolean(table, field)
  local value = table[field]
  if value == nil then
    return nil
  end
  if type(value) ~= "boolean" then
    error("github_android.package field '" .. field .. "' must be a boolean")
  end
  return value
end

local function github_release_candidates()
  if type(getter) ~= "table"
      or type(getter.provider) ~= "table"
      or type(getter.provider.github) ~= "table"
      or type(getter.provider.github.release_candidates) ~= "function" then
    error("luaclass.github_android_apk requires operation-installed getter.provider.github.release_candidates")
  end
  return getter.provider.github.release_candidates
end

function github_android.package(spec)
  if type(spec) ~= "table" then
    error("github_android.package expects a table")
  end
  local host_spec = {
    owner = require_string(spec, "owner"),
    repo = require_string(spec, "repo"),
    asset = optional_table(spec, "asset"),
    include_prereleases = optional_boolean(spec, "include_prereleases"),
    endpoint_id = optional_string(spec, "endpoint_id"),
  }
  local result = github_release_candidates()(host_spec)
  if spec.install ~= nil then
    for _, candidate in ipairs(result.candidates) do
      candidate.install = spec.install
    end
  end
  local package = {
    name = spec.name,
    source_priority = { "github" },
    updates = result.candidates,
  }
  if spec.android_package ~= nil then
    package.installed = {
      { kind = "android_package", package_name = require_string(spec, "android_package") },
    }
  end
  return package_version(package)
end

return github_android
