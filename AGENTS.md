# Agent instructions

## Pull requests and validation

Use pull requests exclusively for iOS builds and their validation. Attach Apple
build and simulator checks to the iOS PR so its watcher receives check results.
All other work, including Linux builds, is developed and validated locally; do
not create a PR or start hosted CI for it. Keep this distinction when working
on shared code: use an iOS PR only when an iOS build is part of the task.

## Rebuild and serve app changes

After every gateway or service change, rebuild and serve the version that is actually being reviewed before reporting completion. The API gateway (`crates/zay-gateway`, compose service `zaymusic`, container `zaymusic-gateway`) is served on loopback port `3847`. Native client changes are verified with their own scripts (`scripts/native-*.sh`); there is no web client.

From the main checkout:

```sh
docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d --build --no-deps zaymusic
```

From a worktree, keep data and `dev.env` anchored at `/home/isaiah/Projects/ZayMusic` and add a build-context override (see README). Then verify `/healthz` reports healthy and that `/rest/ping` and `/api/fetch/healthz` succeed through the gateway, without printing credentials, user IDs or resolved compose config.

## UI direction

When in doubt, copy Spotify’s UI and familiar music-app interaction patterns.
