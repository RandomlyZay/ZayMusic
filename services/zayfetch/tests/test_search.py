"""Search sources can return independently for progressive results."""

from fastapi.testclient import TestClient

from zayfetch import main
from zayfetch.models import Candidate


def test_search_source_selects_one_provider(monkeypatch):
    monkeypatch.setattr(main, "TOKEN", "")
    calls = []

    def music_search(query, count):
        calls.append("music")
        return [Candidate(videoId="track-1", source="ytmusic", title=query, channel="Artist")], [], ""

    def youtube(query, count):
        calls.append("youtube")
        return []

    def find_albums(query, limit=5):
        calls.append("find_albums")
        return [], ""

    monkeypatch.setattr(main, "_music_search", music_search)
    monkeypatch.setattr(main, "_search_yt_dlp", youtube)
    monkeypatch.setattr(main, "_find_albums", find_albums)
    # hermetic: suggestions must not hit the live network
    from zayfetch import discovery
    monkeypatch.setattr(discovery, "ytm_suggestions", lambda q, limit=8: [])
    client = TestClient(main.app)
    response = client.get("/search", params={"q": "Dreams", "source": "music", "limit": 50})
    assert response.status_code == 200
    assert response.json()["candidates"][0]["title"] == "Dreams"
    assert calls == ["music"]

    calls.clear()
    assert client.get("/search", params={"q": "Dreams", "source": "youtube"}).status_code == 200
    # the youtube path computes the same artist signal so the frontend can
    # merge both responses by score
    assert calls == ["youtube", "find_albums"]
