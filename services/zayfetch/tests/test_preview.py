"""New-music playback forwards byte ranges and rejects invalid IDs."""

import httpx
from fastapi.testclient import TestClient

from zayfetch import main


def test_preview_rejects_invalid_id(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    response = TestClient(main.app).get("/preview/invalid!")
    assert response.status_code == 400


def test_preview_streams_audio_range(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(main, "_resolve_playable_url", lambda *a: "https://audio.example/audio.m4a")
    requests = []

    def respond(request):
        requests.append(request)
        return httpx.Response(206, headers={
            "content-type": "audio/mp4", "content-range": "bytes 0-3/100",
            "accept-ranges": "bytes", "content-length": "4",
        }, content=b"test")

    original_client = httpx.AsyncClient
    monkeypatch.setattr(main.httpx, "AsyncClient", lambda **kwargs: original_client(
        transport=httpx.MockTransport(respond), **kwargs
    ))
    response = TestClient(main.app).get("/preview/test1234567", headers={"Range": "bytes=0-3"})
    assert response.status_code == 206
    assert response.content == b"test"
    assert response.headers["content-range"] == "bytes 0-3/100"
    assert requests[0].headers["range"] == "bytes=0-3"


def test_preview_and_warm_routes_use_the_resolver(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(main.previews, "audio_url", lambda vid: (_ for _ in ()).throw(main.previews.Unavailable("x")))
    warmed = []
    monkeypatch.setattr(main.previews, "warm", lambda ids: warmed.extend(ids) or len(ids))
    client = TestClient(main.app)
    monkeypatch.setattr(main.downloader, "cookies_file", lambda: "/tmp/cookies.txt")
    assert client.get("/preview/test1234567").status_code == 502
    response = client.post("/preview/warm", json={"videoIds": ["test1234567", "bad!"]})
    assert response.json() == {"started": 1}
    assert warmed == ["test1234567"]
