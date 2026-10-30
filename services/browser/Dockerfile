# Managed Chromium: the single owner of the Google/YouTube session (§5.1).
# Xvfb + x11vnc + noVNC give a human a real browser to sign in once; the
# CDP port lets zayfetch export live cookies without touching the UI.
FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium \
        xvfb \
        x11vnc \
        x11-utils \
        novnc \
        websockify \
        socat \
        procps \
        python3-websocket \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
COPY close-browser.py /usr/local/bin/close-browser.py
RUN chmod +x /usr/local/bin/entrypoint.sh

# noVNC is served here; CDP (9222) is internal-network only.
EXPOSE 6080 9222

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
