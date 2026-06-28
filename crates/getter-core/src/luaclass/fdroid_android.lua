local fdroid = {}

local function require_string(table, field)
  local value = table[field]
  if type(value) ~= "string" or value == "" then
    error("fdroid.package requires string field '" .. field .. "'")
  end
  return value
end

local function optional_string(table, field)
  local value = table[field]
  if value == nil then
    return nil
  end
  if type(value) ~= "string" then
    error("fdroid.package field '" .. field .. "' must be a string")
  end
  if value == "" then
    return nil
  end
  return value
end

local function fdroid_update_candidates()
  if type(getter) ~= "table"
      or type(getter.provider) ~= "table"
      or type(getter.provider.fdroid) ~= "table"
      or type(getter.provider.fdroid.update_candidates) ~= "function" then
    error("luaclass.fdroid_android requires operation-installed getter.provider.fdroid.update_candidates")
  end
  return getter.provider.fdroid.update_candidates
end

function fdroid.package(spec)
  if type(spec) ~= "table" then
    error("fdroid.package expects a table")
  end
  local host_spec = {
    package_name = require_string(spec, "package_name"),
    endpoint_id = optional_string(spec, "endpoint_id"),
  }
  local result = fdroid_update_candidates()(host_spec)
  return package_version {
    source_priority = { "fdroid" },
    updates = result.candidates,
  }
end

return fdroid
