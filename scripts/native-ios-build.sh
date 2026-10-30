#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/native-ios-platform.py sdk "$(xcrun --sdk iphoneos --show-sdk-version)"
# Rust otherwise defaults to iOS 10 while native C dependencies use the SDK version.
# Set the same minimum as XcodeGen for Rust, cc and every device/simulator slice.
IPHONEOS_DEPLOYMENT_TARGET=$(python3 scripts/native-ios-platform.py deployment-target)
export IPHONEOS_DEPLOYMENT_TARGET
mkdir -p apps/ios/Generated apps/ios/Frameworks apps/ios/Headers
cargo build --locked -p zay-core
cargo run --locked -p zay-core --features bindgen --bin uniffi-bindgen -- generate --library target/debug/libzay_core.dylib --language swift --out-dir apps/ios/Generated
cp apps/ios/Generated/zay_coreFFI.h apps/ios/Headers/
cp apps/ios/Generated/zay_coreFFI.modulemap apps/ios/Headers/module.modulemap
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  [[ "$(rustc --print deployment-target --target "$target")" == "IPHONEOS_DEPLOYMENT_TARGET=$IPHONEOS_DEPLOYMENT_TARGET" ]] || { echo "Rust iOS deployment target mismatch" >&2; exit 1; }
  rustup target add "$target"
  cargo build --locked -p zay-core --release --target "$target"
done
mkdir -p target/ios-simulator
lipo -create target/aarch64-apple-ios-sim/release/libzay_core.a target/x86_64-apple-ios/release/libzay_core.a -output target/ios-simulator/libzay_core.a
rm -rf apps/ios/Frameworks/ZayCore.xcframework
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libzay_core.a -headers apps/ios/Headers \
  -library target/ios-simulator/libzay_core.a -headers apps/ios/Headers \
  -output apps/ios/Frameworks/ZayCore.xcframework
(cd apps/ios && xcodegen generate)
xcodebuild -project apps/ios/ZayMusic.xcodeproj -scheme ZayMusic -sdk iphoneos -configuration Release -derivedDataPath build/ios-device CODE_SIGNING_ALLOWED=NO build
python3 scripts/native-ios-platform.py app build/ios-device/Build/Products/Release-iphoneos/ZayMusic.app
mkdir -p build/ipa/Payload
cp -R build/ios-device/Build/Products/Release-iphoneos/ZayMusic.app build/ipa/Payload/
(cd build/ipa && zip -qry ../ZayMusic-unsigned.ipa Payload)
