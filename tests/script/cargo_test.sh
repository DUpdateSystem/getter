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

echo "Building with default features"
run cargo build --verbose
echo "Testing with default features"
run cargo test --verbose

echo "Building with each feature individually"
for feature in "rustls-platform-verifier" "webpki-roots" "native-tokio"; do
  echo "Building with feature: $feature"
  run cargo build --verbose --no-default-features --features "$feature"
done
echo "Testing with each feature individually"
for feature in "rustls-platform-verifier" "webpki-roots" "native-tokio"; do
  echo "Testing with feature: $feature"
  run cargo test --verbose --no-default-features --features "$feature"
done

echo "Building with all features"
run cargo build --verbose --all-features
echo "Testing with all features"
run cargo test --verbose --all-features
