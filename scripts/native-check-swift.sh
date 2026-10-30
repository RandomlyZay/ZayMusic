#!/usr/bin/env bash
# Parse platform code and run the generated Swift/Rust bridge on Linux before using macOS CI.
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in swiftc cargo python3 cc; do
  command -v "$tool" >/dev/null || { echo "Missing Swift bridge prerequisite: $tool" >&2; exit 1; }
done
python3 scripts/native-ios-platform.py self-test
IOS_MINIMUM=$(python3 scripts/native-ios-platform.py deployment-target)
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  [[ "$(IPHONEOS_DEPLOYMENT_TARGET="$IOS_MINIMUM" rustc --print deployment-target --target "$target")" == "IPHONEOS_DEPLOYMENT_TARGET=$IOS_MINIMUM" ]] || { echo "Rust iOS deployment target mismatch" >&2; exit 1; }
done
swiftc -frontend -parse apps/ios/ZayMusic/*.swift apps/ios/ZayMusicTests/*.swift apps/ios/ZayMusicUITests/*.swift
cargo build --locked -p zay-core --features bindgen
mkdir -p build/swift-linux
cargo run --locked -p zay-core --features bindgen --bin uniffi-bindgen -- generate --library target/debug/libzay_core.so --language swift --out-dir build/swift-linux
# The generated module map declares Darwin-specific module dependencies. The C
# header itself is portable; omit only those declarations for this Linux check.
python3 - <<'PY'
from pathlib import Path
root=Path('build/swift-linux')
text=(root/'zay_coreFFI.modulemap').read_text()
(root/'module.modulemap').write_text('\n'.join(line for line in text.splitlines() if 'use ' not in line)+'\n')
PY
swiftc -I build/swift-linux -L target/debug -lzay_core build/swift-linux/zay_core.swift apps/ios/ZayMusic/Models.swift scripts/native-swift-check.swift -o build/native-swift-check
FIXTURE_READY=$(mktemp)
trap 'kill "${FIXTURE_PID:-}" 2>/dev/null || true; rm -f "$FIXTURE_READY"' EXIT
python3 scripts/native-fixture-server.py --port 0 --ready-file "$FIXTURE_READY" > build/swift-linux/fixtures.log 2>&1 &
FIXTURE_PID=$!
for _ in {1..50}; do [[ -s "$FIXTURE_READY" ]] && break; sleep 0.1; done
[[ -s "$FIXTURE_READY" ]] || { echo "Swift fixture server failed to start" >&2; exit 1; }
NATIVE_FIXTURE_URL="$(cat "$FIXTURE_READY")" LD_LIBRARY_PATH="$PWD/target/debug${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" build/native-swift-check
