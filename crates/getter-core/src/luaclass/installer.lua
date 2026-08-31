local installer = {}

function installer.artifact(name)
  return { artifact = name }
end

function installer.command(spec)
  return spec
end

function installer.android_apk(spec)
  spec.kind = "android_apk"
  return spec
end

return installer
