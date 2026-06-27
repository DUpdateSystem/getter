local fdroid = {}

local function require_string(table, field)
  local value = table[field]
  if type(value) ~= "string" or value == "" then
    error("fdroid.package requires string field '" .. field .. "'")
  end
  return value
end

local function fdroid_update_candidates()
  if type(getter_dev) ~= "table" or type(getter_dev.fdroid_update_candidates) ~= "function" then
    error("luaclass.fdroid_android requires operation-installed getter_dev.fdroid_update_candidates")
  end
  return getter_dev.fdroid_update_candidates
end

function fdroid.package(spec)
  if type(spec) ~= "table" then
    error("fdroid.package expects a table")
  end
  local package_name = require_string(spec, "package_name")
  local update_candidates = fdroid_update_candidates()
  return package_version {
    source_priority = { "fdroid" },
    updates = update_candidates {
      package_name = package_name,
    },
  }
end

return fdroid
