#!/bin/sh
set -eu
cd "$(dirname "$0")"
: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
export ANDROID_HOME
if [ -z "${JAVA_HOME:-}" ] && [ -d '/Applications/Android Studio.app/Contents/jbr/Contents/Home' ]; then
    JAVA_HOME='/Applications/Android Studio.app/Contents/jbr/Contents/Home'; export JAVA_HOME
fi
exec ./gradlew --no-daemon :app:assembleDebug "$@"
