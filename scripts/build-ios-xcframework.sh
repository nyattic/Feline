#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRATE="feline-core"
LIB_NAME="libfeline_core.a"
OUT="$ROOT/target/ios"
SWIFT_OUT="$OUT/swift"

cd "$ROOT"

# 1. Build static libs for each iOS target
cargo build --release --target aarch64-apple-ios          -p $CRATE
cargo build --release --target aarch64-apple-ios-sim      -p $CRATE
cargo build --release --target x86_64-apple-ios-sim       -p $CRATE 2>/dev/null \
  || cargo build --release --target x86_64-apple-ios      -p $CRATE 2>/dev/null \
  || echo "skipping x86_64 simulator slice (host is arm64-only)"

# 2. lipo simulator slices into one fat lib (or just copy if only one slice)
mkdir -p "$OUT/sim"
if [ -f "target/x86_64-apple-ios-sim/release/$LIB_NAME" ]; then
    lipo -create \
        "target/aarch64-apple-ios-sim/release/$LIB_NAME" \
        "target/x86_64-apple-ios-sim/release/$LIB_NAME" \
        -output "$OUT/sim/$LIB_NAME"
elif [ -f "target/x86_64-apple-ios/release/$LIB_NAME" ]; then
    lipo -create \
        "target/aarch64-apple-ios-sim/release/$LIB_NAME" \
        "target/x86_64-apple-ios/release/$LIB_NAME" \
        -output "$OUT/sim/$LIB_NAME"
else
    cp "target/aarch64-apple-ios-sim/release/$LIB_NAME" "$OUT/sim/$LIB_NAME"
fi

# 3. Generate Swift bindings + module map
mkdir -p "$SWIFT_OUT"
cargo run --release --bin uniffi-bindgen-feline -- \
    generate --library "target/aarch64-apple-ios/release/$LIB_NAME" \
    --language swift --out-dir "$SWIFT_OUT"

# 4. Assemble xcframework
mkdir -p "$OUT/headers"
cp "$SWIFT_OUT"/*.h "$OUT/headers/" 2>/dev/null || true
# uniffi may emit module.modulemap or feline_coreFFI.modulemap -- pick whichever exists
if [ -f "$SWIFT_OUT/module.modulemap" ]; then
    cp "$SWIFT_OUT/module.modulemap" "$OUT/headers/module.modulemap"
elif compgen -G "$SWIFT_OUT/*.modulemap" > /dev/null; then
    cp "$SWIFT_OUT"/*.modulemap "$OUT/headers/module.modulemap"
fi

rm -rf "$OUT/FelineCore.xcframework"
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/$LIB_NAME" -headers "$OUT/headers" \
    -library "$OUT/sim/$LIB_NAME"                          -headers "$OUT/headers" \
    -output "$OUT/FelineCore.xcframework"

echo "built: $OUT/FelineCore.xcframework"
echo "swift sources: $SWIFT_OUT"
