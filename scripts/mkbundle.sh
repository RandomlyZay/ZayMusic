#!/usr/bin/env bash
# Builds ~/Projects/ZayMusic/deploy-bundle/ — the artifact the host pulls.
# Idempotent. Safe to re-run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/deploy-bundle"
STAMP="$(date +%Y.%m.%d-%H%M)"
SHA="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo "nogit")"

rm -rf "$OUT"
mkdir -p "$OUT"

# source + Dockerfiles (NO dev-only files — see §11.4)
# The gateway builds from the bundle root: workspace manifests, lockfile, the
# context allowlist and the gateway crate. Native client sources never ship.
(cd "$ROOT" && rsync -aR --exclude 'target/' \
  ./Cargo.toml ./Cargo.lock ./.dockerignore \
  ./crates/zay-core/Cargo.toml ./apps/linux/Cargo.toml ./crates/zay-gateway \
  "$OUT/")
rsync -a \
  --exclude '__pycache__/' --exclude '*.pyc' --exclude '.pytest_cache/' \
  --exclude 'deploy/' --exclude 'browser/data/' \
  "$ROOT/services" "$OUT/"

# prod compose only. NEVER docker-compose.dev.yml (§4.3).
cp "$ROOT/docker-compose.yml" "$OUT/docker-compose.yml"
cp "$ROOT/.env.example" "$OUT/.env.example"
cp "$ROOT/scripts/gen-secrets.sh" "$OUT/gen-secrets.sh"
chmod +x "$OUT/gen-secrets.sh"
# the runbook has exactly one source of truth (§11.5)
cp "$ROOT/services/deploy/README-DEPLOY.md.tmpl" "$OUT/README-DEPLOY.md"

# hard guard: the dev override must never ship
rm -f "$OUT/docker-compose.dev.yml" "$OUT/dev.env" "$OUT/dev.env.example"

# record what this bundle is
python3 - <<PY
import json, os, datetime, pathlib
root = pathlib.Path("$OUT")
pin = ""
p = root / "services/zayfetch/pyproject.toml"
if p.exists():
    for line in p.read_text().splitlines():
        s = line.strip()
        if s.startswith('"yt-dlp') or s.startswith("'yt-dlp") or s.startswith("yt-dlp"):
            pin = s
            break
(root / "app-manifest.json").write_text(json.dumps({
    "sha": "$SHA",
    "slug": "$STAMP",
    "builtAt": datetime.datetime.now(datetime.UTC).isoformat(),
    "zayfetchYtdlpPin": pin,
}, indent=2) + "\n")
PY

# safety: refuse to ship a real .env
if [ -f "$OUT/.env" ]; then
  echo "FATAL: deploy-bundle/.env present — real secrets must never ship" >&2
  rm -f "$OUT/.env"; exit 1
fi

# safety: no audio files
if find "$OUT" -type f \( -name '*.opus' -o -name '*.m4a' -o -name '*.mp3' -o -name '*.flac' -o -name '*.wav' -o -name '*.ogg' \) | grep -q .; then
  echo "FATAL: audio files present in deploy-bundle" >&2
  exit 1
fi

echo "bundle ready: $OUT   (sha=$SHA slug=$STAMP)"
echo "next: tell Isaiah  \"deploy ZayMusic from ~/Projects/ZayMusic/deploy-bundle/\""
