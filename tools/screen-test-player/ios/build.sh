#!/bin/sh
set -eu
cd "$(dirname "$0")"
xcodegen generate
if [ "${1:-}" = 'device' ]; then
    : "${DEVELOPMENT_TEAM:?Set DEVELOPMENT_TEAM to your Apple signing team}"
    xcodebuild -project NodeForgeScreenTest.xcodeproj -scheme ScreenTest -configuration Debug -sdk iphoneos -derivedDataPath build DEVELOPMENT_TEAM="$DEVELOPMENT_TEAM" -allowProvisioningUpdates build
else
    xcodebuild -project NodeForgeScreenTest.xcodeproj -scheme ScreenTest -configuration Debug -sdk iphonesimulator -derivedDataPath build CODE_SIGNING_ALLOWED=NO build
fi
