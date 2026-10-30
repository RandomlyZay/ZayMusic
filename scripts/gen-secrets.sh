#!/usr/bin/env bash
# Prints a .env to stdout with real random secrets. Redirect to .env, chmod 600.
set -euo pipefail
hex() { openssl rand -hex "${1:-16}"; }
cat <<EOF
# ZayMusic — real secrets. chmod 600. NEVER commit.
NAVIDROME_USER=admin
NAVIDROME_PASS=$(hex 16)
ZAYFETCH_TOKEN=$(hex 32)

# Paths on the host (relative to ~/services/zaymusic/ or absolute)
MUSIC_DIR=./music
NAVIDROME_DATA_DIR=./navidrome
ZAYFETCH_DATA_DIR=./zayfetch/data

# The ONE published host port. Loopback only. Avoid 8080/8384/22000/51515/8888/3001.
APP_PORT=3847

# Optional (free): Last.fm key for correction + radio tiers. Get one at
# https://www.last.fm/api/account/create and paste it in .env after generating.
LASTFM_API_KEY=

# Managed browser session. browser/data/ holds the live Google session.
BROWSER_DATA_DIR=./browser/data
BROWSER_PORT=6080
# Over Tailscale, serve the loopback noVNC port and set this so the app's
# "Open remote browser" link points at it (replace <host>):
#   tailscale serve --bg --https=8446 http://127.0.0.1:6080
#   PUBLIC_REMOTE_BROWSER_URL=https://<host>.tailfc5dc.ts.net:8446/vnc.html
PUBLIC_REMOTE_BROWSER_URL=

# Optional Google API OAuth (API-only; downloads still need the browser
# session). Create an OAuth client of type "TVs and Limited Input devices"
# with the YouTube Data API enabled, then paste both values in .env:
# https://console.cloud.google.com/apis/credentials
YTM_OAUTH_CLIENT_ID=
YTM_OAUTH_CLIENT_SECRET=

# Automated re-login (optional): fill the Google account the managed browser
# signs in as. GOOGLE_TOTP_SECRET is the authenticator setup code (base32) so
# 2FA challenges can complete without you. Leave empty to sign in via noVNC.
GOOGLE_EMAIL=
GOOGLE_PASSWORD=
GOOGLE_TOTP_SECRET=
YOUTUBE_RELOGIN_MIN_MINUTES=60
EOF
