local github_android = {}

local function require_string(table, field)
  local value = table[field]
  if type(value) ~= "string" or value == "" then
    error("github_android.package requires string field '" .. field .. "'")
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
  if type(getter_dev) ~= "table" or type(getter_dev.github_release_candidates) ~= "function" then
    error("luaclass.github_android_apk requires operation-installed getter_dev.github_release_candidates")
  end
  return getter_dev.github_release_candidates
end

function github_android.package(spec)
  if type(spec) ~= "table" then
    error("github_android.package expects a table")
  end
  local owner = require_string(spec, "owner")
  local repo = require_string(spec, "repo")
  local release_candidates = github_release_candidates()
  local result = {
    name = spec.name,
    source_priority = { "github" },
    updates = release_candidates {
      owner = owner,
      repo = repo,
      asset = optional_table(spec, "asset"),
      include_prereleases = optional_boolean(spec, "include_prereleases"),
    },
  }
  if spec.android_package ~= nil then
    result.installed = {
      { kind = "android_package", package_name = require_string(spec, "android_package") },
    }
  end
  return package_version(result)
end

return github_android
