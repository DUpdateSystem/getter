local installer = {}

function installer.artifact(name)
  return { artifact = name }
end

function installer.command(spec)
  return spec
end

return installer
