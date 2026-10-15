#!/bin/sh
set -eu
cd "$(dirname "$0")"
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/screen-test-brightness.XXXXXX")
trap 'rm -rf "$test_dir"' EXIT
xcrun swiftc Sources/BrightnessHold.swift Tests/main.swift -o "$test_dir/test-brightness"
"$test_dir/test-brightness"
