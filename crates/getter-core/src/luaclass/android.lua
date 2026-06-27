local android = {}

function android.package_version(input)
  return package_version {
    name = input.name,
    installed = input.installed,
    source_priority = input.source_priority,
    updates = input.updates,
  }
end

return android
