#!/usr/bin/env bash
# Run on devbox with --mypc to install remotely, or locally on a Fedora desktop.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --release -p zaymusic-linux
if [[ "${1:-}" == --mypc ]]; then
  # Single binary; transfer without touching desktop audio or personal app data.
  ssh -o BatchMode=yes -o ConnectTimeout=10 isaiah@mypc 'mkdir -p ~/.local/bin ~/.local/share/applications ~/.local/share/icons/hicolor/scalable/apps'
  TASK_TRANSFER=$(mktemp -d)
  TASK_IP=$(tailscale ip -4)
  TASK_SHA=$(sha256sum target/release/zaymusic-linux | cut -d' ' -f1)
  split -b 512K -d target/release/zaymusic-linux "$TASK_TRANSFER/part"
  TASK_PARTS=$(find "$TASK_TRANSFER" -name 'part*' | wc -l)
  python3 -m http.server 18749 --bind "$TASK_IP" --directory "$TASK_TRANSFER" >/dev/null 2>&1 &
  TASK_SERVER_PID=$!
  trap 'kill "$TASK_SERVER_PID" 2>/dev/null || true; rm -rf "$TASK_TRANSFER"' EXIT
  sleep 1
  kill -0 "$TASK_SERVER_PID"
  ssh isaiah@mypc bash -s -- "$TASK_IP" "$TASK_SHA" "$TASK_PARTS" <<'REMOTE'
set -euo pipefail
TASK_REMOTE=$(mktemp -d)
trap 'rm -rf "$TASK_REMOTE"' EXIT
export TASK_REMOTE
TASK_SOURCE="$1"
export TASK_SOURCE
seq -f '%02g' 0 "$(($3 - 1))" | xargs -P16 -I{} sh -c 'curl --fail --silent --show-error "http://$TASK_SOURCE:18749/part{}" -o "$TASK_REMOTE/part{}"'
cat "$TASK_REMOTE"/part* > "$TASK_REMOTE/binary"
printf '%s  %s\n' "$2" "$TASK_REMOTE/binary" | sha256sum -c -
install -m755 "$TASK_REMOTE/binary" "$HOME/.local/bin/zaymusic"
REMOTE
  scp apps/linux/io.zaymusic.Native.desktop isaiah@mypc:.local/share/applications/
  scp apps/linux/io.zaymusic.Native.svg isaiah@mypc:.local/share/icons/hicolor/scalable/apps/
  ssh isaiah@mypc 'chmod +x ~/.local/bin/zaymusic; update-desktop-database ~/.local/share/applications'
else
  install -Dm755 target/release/zaymusic-linux "$HOME/.local/bin/zaymusic"
  install -Dm644 apps/linux/io.zaymusic.Native.desktop "$HOME/.local/share/applications/io.zaymusic.Native.desktop"
  install -Dm644 apps/linux/io.zaymusic.Native.svg "$HOME/.local/share/icons/hicolor/scalable/apps/io.zaymusic.Native.svg"
  update-desktop-database "$HOME/.local/share/applications"
fi
