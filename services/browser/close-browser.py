"""Ask Chromium to flush its profile and quit, rather than killing it."""

import json
import sys
from urllib.request import urlopen

from websocket import create_connection


def main() -> None:
    port = int(sys.argv[1])
    with urlopen(f"http://127.0.0.1:{port}/json/version", timeout=3) as response:
        url = json.load(response)["webSocketDebuggerUrl"]
    # CDP is loopback-only here. Omitting Origin avoids Chromium rejecting
    # its own local shutdown client; no permissive remote-origin flag needed.
    socket = create_connection(url, timeout=5, suppress_origin=True)
    try:
        socket.send(json.dumps({"id": 1, "method": "Browser.close"}))
        socket.recv()
    finally:
        socket.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"Chromium graceful shutdown unavailable: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
