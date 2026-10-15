#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
mkdir -p target/screen-test
opencv_pkg=opencv4
if ! pkg-config --exists "$opencv_pkg"; then opencv_pkg=opencv5; fi
pkg-config --exists libraw "$opencv_pkg"
omp_flags=""
if command -v brew >/dev/null 2>&1; then omp_prefix=$(brew --prefix libomp); omp_flags="-I$omp_prefix/include -L$omp_prefix/lib -lomp"; fi
c++ $omp_flags -std=c++17 -O2 tools/screen-test-native/main.cpp $(pkg-config --cflags libraw "$opencv_pkg") $(pkg-config --libs libraw) -L$(pkg-config --variable=libdir "$opencv_pkg") -lopencv_objdetect -lopencv_imgcodecs -lopencv_imgproc -lopencv_core -o target/screen-test/screen-test-native
