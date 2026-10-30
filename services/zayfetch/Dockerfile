FROM python:3.15.2-slim
COPY --from=ghcr.io/astral-sh/uv:0.13.6 /uv /usr/local/bin/uv

# ffmpeg for remux/loudness. yt-dlp also needs a supported JS runtime to
# solve YouTube's "n" signature challenge (EJS) — Deno is yt-dlp's
# recommended runtime and is enabled by default when on PATH.
ARG DENO_VERSION=2.9.8
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && python - <<'PY'
import io
import hashlib
import os
import urllib.request
import zipfile

arch = {"x86_64": "x86_64-unknown-linux-gnu", "aarch64": "aarch64-unknown-linux-gnu"}[os.uname().machine]
version = os.environ["DENO_VERSION"]
url = f"https://github.com/denoland/deno/releases/download/v{version}/deno-{arch}.zip"
data = urllib.request.urlopen(url, timeout=120).read()
expected = urllib.request.urlopen(url + ".sha256sum", timeout=120).read().decode().split()[0]
if hashlib.sha256(data).hexdigest() != expected:
    raise RuntimeError("Deno release checksum mismatch")
zipfile.ZipFile(io.BytesIO(data)).extract("deno", "/usr/local/bin")
os.chmod("/usr/local/bin/deno", 0o755)
PY
RUN deno --version

WORKDIR /app

# Resolve once in uv.lock; the image never silently falls back to unlocked pip installs.
ENV UV_LINK_MODE=copy
COPY pyproject.toml uv.lock ./
RUN uv sync --locked --no-dev --no-install-project --python /usr/local/bin/python
COPY zayfetch ./zayfetch
RUN uv sync --locked --no-dev --python /usr/local/bin/python
ENV PATH="/app/.venv/bin:$PATH"

ENV PYTHONUNBUFFERED=1
EXPOSE 8080

CMD ["uvicorn", "zayfetch.main:app", "--host", "0.0.0.0", "--port", "8080"]
