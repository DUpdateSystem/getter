#!/bin/bash

# function for running a command and printing it
# if command failed, print notification and exit
function run {
  echo "Running: $@"
  "$@"
  local status=$?
  if [ $status -ne 0 ]; then
    echo "Command failed: $@"
    exit $status
  fi
}

echo "Building workspace with default features"
run cargo build --workspace --verbose
echo "Testing workspace with default features"
run cargo test --workspace --verbose

echo "Building individual packages"
packages=("getter-utils" "getter-cache" "getter-provider" "getter-config" "getter-appmanager" "getter-rpc" "getter-core" "getter-cli")
for package in "${packages[@]}"; do
  echo "Building package: $package"
  run cargo build --package "$package" --verbose
done

echo "Testing individual packages"
for package in "${packages[@]}"; do
  echo "Testing package: $package"
  run cargo test --package "$package" --verbose
done

echo "Building with feature flags (where applicable)"
echo "Building getter-cache with concurrent feature"
run cargo build --package getter-cache --features concurrent --verbose

echo "Building with all features"
run cargo build --workspace --all-features --verbose
echo "Testing with all features"
run cargo test --workspace --all-features --verbose

echo "All tests completed successfully!"
