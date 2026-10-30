"""Idempotent Navidrome admin bootstrap (§6.1, §11.3).

Contract:
  - Idempotent: running twice is a no-op success.
  - After exit 0, NAVIDROME_USER / NAVIDROME_PASS authenticate via Subsonic ping.
  - Never prints secrets. Never writes .env.
  - No UI click-through, no ND_ENABLEAUTH=false, no docker exec.

Usage:
  docker compose run --rm zayfetch python -m zayfetch.bootstrap_admin
"""

from __future__ import annotations

import hashlib
import os
import secrets
import sys
import time

import httpx

NAVIDROME_URL = os.environ.get("NAVIDROME_URL", "http://navidrome:4533").rstrip("/")
USER = os.environ.get("NAVIDROME_USER", "admin")
PASS = os.environ.get("NAVIDROME_PASS", "")

# The first-run admin Navidrome creates on an empty DB. We try a small set of
# well-known defaults, then fall back to whatever getUsers reveals if we have
# an admin-capable account already.
FIRST_RUN_CANDIDATES = ["admin", "navidrome"]


def subsonic_params(password: str, user: str | None = None, salt: str | None = None) -> dict[str, str]:
    u = user or USER
    s = salt or secrets.token_hex(16)
    t = hashlib.md5((password + s).encode("utf-8")).hexdigest()
    return {"u": u, "t": t, "s": s, "v": "1.16.1", "c": "ZayFetchBootstrap", "f": "json"}


def ping(client: httpx.Client, *, user: str | None = None, password: str | None = None) -> bool:
    pw = password if password is not None else PASS
    try:
        r = client.get(
            f"{NAVIDROME_URL}/rest/ping",
            params=subsonic_params(pw, user=user),
            timeout=10,
        )
        if r.status_code != 200:
            return False
        body = r.json().get("subsonic-response", {})
        return body.get("status") == "ok"
    except Exception:
        return False


def subsonic_call(
    client: httpx.Client,
    endpoint: str,
    *,
    user: str,
    password: str,
    method: str = "GET",
    extra: dict | None = None,
) -> dict:
    params = subsonic_params(password, user=user)
    if extra:
        params.update(extra)
    url = f"{NAVIDROME_URL}/rest/{endpoint}"
    if method == "POST":
        r = client.post(url, params=params, timeout=15)
    else:
        r = client.get(url, params=params, timeout=15)
    r.raise_for_status()
    body = r.json().get("subsonic-response", {})
    if body.get("status") != "ok":
        raise RuntimeError(f"{endpoint} failed: {body.get('error')}")
    return body


def wait_for_navidrome(client: httpx.Client, timeout_sec: int = 120) -> None:
    deadline = time.time() + timeout_sec
    while time.time() < deadline:
        try:
            r = client.get(f"{NAVIDROME_URL}/rest/ping", params=subsonic_params("x", user="probe", salt="probe" + str(int(time.time()))), timeout=5)
            # any HTTP answer means the server is up (even 401/failed status)
            if r.status_code < 500:
                return
        except Exception:
            pass
        time.sleep(2)
    raise SystemExit("navidrome did not become ready in time")


def create_user(
    client: httpx.Client,
    *,
    admin_user: str,
    admin_pass: str,
    username: str,
    password: str,
) -> None:
    """createUser is admin-only. password is the PLAINTEXT new user password."""
    subsonic_call(
        client,
        "createUser",
        user=admin_user,
        password=admin_pass,
        method="GET",
        extra={
            "username": username,
            "password": password,
            "adminRole": "true",
            "email": f"{username}@zaymusic.local",
        },
    )


def ensure_password(
    client: httpx.Client,
    *,
    admin_user: str,
    admin_pass: str,
    username: str,
    password: str,
) -> None:
    """updateUser can change a password; ignore 'already correct' style errors."""
    try:
        subsonic_call(
            client,
            "updateUser",
            user=admin_user,
            password=admin_pass,
            method="GET",
            extra={"username": username, "password": password},
        )
    except Exception:
        # If ping with the target creds already works, we're fine.
        if ping(client, user=username, password=password):
            return
        raise


def create_first_admin(client: httpx.Client) -> bool:
    """Navidrome first-run: POST /auth/createAdmin when zero users exist."""
    try:
        r = client.post(
            f"{NAVIDROME_URL}/auth/createAdmin",
            json={"username": USER, "password": PASS},
            timeout=15,
            headers={"Content-Type": "application/json"},
        )
        if r.status_code in (200, 201):
            return ping(client)
        # already exists / forbidden
        if r.status_code in (403, 409, 422):
            return False
        return False
    except Exception:
        return False


def main() -> int:
    if not PASS:
        print("FATAL: NAVIDROME_PASS is empty", file=sys.stderr)
        return 1

    with httpx.Client() as client:
        print("waiting for navidrome…")
        wait_for_navidrome(client)

        # Fast path: target creds already work.
        if ping(client):
            print("ok: navidrome admin already matches env credentials")
            return 0

        # Fresh DB: create first admin via Navidrome's one-shot endpoint.
        if create_first_admin(client):
            print(f"ok: created initial admin via /auth/createAdmin")
            return 0

        # Find a working admin account.
        admin_user: str | None = None
        admin_pass: str | None = None

        for cand in FIRST_RUN_CANDIDATES:
            for pw in ("", "admin", "navidrome", PASS):
                if ping(client, user=cand, password=pw):
                    admin_user, admin_pass = cand, pw
                    break
            if admin_user:
                break

        if not admin_user and ping(client, password=""):
            admin_user, admin_pass = USER, ""

        if not admin_user:
            print(
                "FATAL: could not obtain a Navidrome admin account. "
                "If this is a fresh DB, ensure navidrome has finished first-run init, "
                "or open http://navidrome:4533 once and create the admin via UI, "
                "then re-run bootstrap.",
                file=sys.stderr,
            )
            return 1

        # Ensure the target user exists and has the target password.
        try:
            create_user(
                client,
                admin_user=admin_user,
                admin_pass=admin_pass or "",
                username=USER,
                password=PASS,
            )
            print(f"created user {USER}")
        except Exception as e:
            msg = str(e).lower()
            if "already exists" in msg or "exist" in msg:
                ensure_password(
                    client,
                    admin_user=admin_user,
                    admin_pass=admin_pass or "",
                    username=USER,
                    password=PASS,
                )
                print(f"user {USER} already exists; password ensured")
            elif ping(client, user=USER, password=PASS):
                print(f"ok: user {USER} already authenticates")
            else:
                print(f"FATAL: createUser/updateUser failed: {e}", file=sys.stderr)
                return 1

        if not ping(client):
            print("FATAL: final ping with env credentials failed", file=sys.stderr)
            return 1
        print("ok: navidrome admin ready")
        return 0


if __name__ == "__main__":
    sys.exit(main())
