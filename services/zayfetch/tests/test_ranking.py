"""Ranking tests — assert order AND that nothing is filtered out (§15)."""

from zayfetch.models import Candidate
from zayfetch.ranking import rank, score, is_topic


def mk(vid, title, channel, source="youtube", dur=200.0, badges=None, album=None):
    return Candidate(
        videoId=vid,
        source=source,  # type: ignore[arg-type]
        title=title,
        channel=channel,
        album=album,
        durationSec=dur,
        badges=badges or [],  # type: ignore[arg-type]
    )


def test_topic_scores_higher():
    topic = mk("t1", "Song Name", "Artist - Topic", source="ytmusic")
    plain = mk("p1", "Song Name", "SomeUser", source="youtube")
    assert score(topic, "Song Name") > score(plain, "Song Name")


def test_explicit_remix_intent_beats_catalog_studio_recording():
    studio = mk("studio", "Song Name", "Artist - Topic", source="ytmusic")
    studio.views = 1_000_000_000
    remix = mk("remix", "Artist - Song Name (Fan Remix)", "Bedroom producer")
    ranked = rank([studio, remix], query="Song Name remix", catalog_matches={"studio"}, catalog_active=True)
    assert ranked[0].videoId == "remix"
    assert len(ranked) == 2


def test_live_cover_remix_neutral():
    """'live'/'cover'/'remix' titles must not be filtered or heavily demoted."""
    base = mk("b", "Song", "Artist - Topic", source="ytmusic")
    live = mk("l", "Song (Live)", "Artist - Topic", source="ytmusic")
    cover = mk("c", "Song (Cover)", "Artist - Topic", source="ytmusic")
    remix = mk("r", "Song (Remix)", "Artist - Topic", source="ytmusic")
    q = "Song"
    s_base = score(base, q)
    # they may score slightly differently due to fuzzy ratio, but must stay close
    for x in (live, cover, remix):
        assert abs(score(x, q) - s_base) < 3.0
    # an explicit music-video variant is demoted (rank-only) but stays listed
    mv = mk("v", "Song (Music Video)", "ArtistVEVO")
    ranked = rank([mv, base], query=q)
    assert len(ranked) == 2
    assert ranked[0].videoId == "b"


def test_rank_returns_everything():
    cands = [
        mk("1", "Obscure Live Recording", "SomeChannel"),
        mk("2", "Karaoke Version", "OtherChannel"),
        mk("3", "Instrumental", "Another"),
        mk("4", "Song", "Artist - Topic", source="ytmusic"),
        mk("5", "Song (Music Video)", "ArtistVEVO"),
    ]
    ranked = rank(cands, query="Song")
    assert len(ranked) == 5  # NOTHING filtered out
    # Topic + ytmusic should be on top for matching title
    assert ranked[0].videoId == "4"
    # music video soft-demoted but still present
    assert any(c.videoId == "5" for c in ranked)


def test_duration_mismatch_demotes_softly():
    expected = mk("e", "Song", "Artist - Topic", source="ytmusic", dur=200)
    weird = mk("w", "Song", "Artist - Topic", source="ytmusic", dur=3600)
    assert score(weird, "Song", expected_duration=200) < score(expected, "Song", expected_duration=200)


def test_badges_computed():
    cands = [mk("1", "Official Music Video - Song", "Artist VEVO")]
    ranked = rank(cands, query="Song")
    assert "likely-mv" in ranked[0].badges


def test_is_topic():
    assert is_topic("Somebody - Topic")
    assert not is_topic("Somebody")


def test_artist_and_title_query_beats_unrelated_title():
    exact = mk("a", "Dreams", "Fleetwood Mac", source="ytmusic")
    unrelated = mk("b", "Fleetwood Dreams (cover)", "Someone Else", source="ytmusic")
    assert rank([unrelated, exact], query="Fleetwood Mac Dreams")[0].videoId == "a"


def test_album_track_beats_unrelated_same_artist_song():
    """Album queries never match track titles, so the album bonus must lift
    album-expanded tracks above unrelated same-artist YTM songs."""
    album_track = mk("a", "9", "Drake", source="ytmusic", album="Views",
                     badges=["has-metadata"])
    unrelated = mk("b", "Chicago Freestyle", "Drake", source="ytmusic",
                   badges=["has-metadata"])
    assert score(album_track, "Drake Views") > score(unrelated, "Drake Views")
    assert rank([unrelated, album_track], query="Drake Views")[0].videoId == "a"


def test_album_bonus_needs_album_match():
    matched = mk("a", "9", "Drake", source="youtube", album="Views")
    plain = mk("b", "9", "Drake", source="youtube")
    assert score(matched, "Drake Views") > score(plain, "Drake Views")


def test_full_term_coverage_beats_same_artist_junk():
    """"J Cole Change" must rank the exact-titled upload above other J. Cole songs."""
    exact = mk("a", "J. Cole - Change", "Otar", source="youtube")
    junk = mk("b", "No Role Modelz", "J. Cole", source="ytmusic", badges=["has-metadata"])
    assert score(exact, "J Cole Change") > score(junk, "J Cole Change")
    assert rank([junk, exact], query="J Cole Change")[0].videoId == "a"


def test_exact_multiword_title_beats_longer_variant():
    exact = mk("a", "Hotline Bling", "Drake - Topic", source="ytmusic")
    longer = mk("b", "Hotline Bling (1950s Motown Choir Gospel)", "Mr. Soul - Topic", source="youtube")
    assert score(exact, "Hotline Bling") > score(longer, "Hotline Bling")


def test_album_score_same_scale():
    from zayfetch.ranking import album_score

    assert album_score("Views", "Drake", "Drake Views") > album_score("Scorpion", "Drake", "Drake Views")
    assert album_score("Views", "Drake", "") == 0.0


def test_parse_views_formats():
    from zayfetch.ranking import parse_views

    assert parse_views("24M") == 24_000_000
    assert parse_views("5.9K") == 5900
    assert parse_views("2.5B") == 2_500_000_000
    assert parse_views("1,234") == 1234
    assert parse_views(2155424848) == 2155424848
    assert parse_views(None) is None
    assert parse_views("") is None
    assert parse_views("No views") is None


def test_popular_official_beats_obscure_imposter():
    """2.1B-view official upload beats a 10K-view mislabeled re-upload."""
    real = mk("r", "Hotline Bling", "Drake", source="youtube", badges=[], album=None)
    real.views = 2_155_424_848
    imposter = mk("i", "Hotline Bling", "Drake", source="ytmusic", badges=["has-metadata"])
    imposter.views = 10_000
    assert score(real, "hotline bling", top_artist="Drake") > score(imposter, "hotline bling", top_artist="Drake")
    assert rank([imposter, real], query="hotline bling", top_artist="Drake")[0].videoId == "r"


def test_top_artist_breaks_exact_title_ties():
    """Same exact title: the query's top artist wins over a popular cover."""
    official = mk("r", "Hotline Bling", "Drake", source="youtube")
    official.views = 2_155_424_848
    cover = mk("c", "Hotline Bling", "William Singe", source="ytmusic", badges=["has-metadata"])
    cover.views = 24_000_000
    assert rank([cover, official], query="hotline bling", top_artist="Drake")[0].videoId == "r"


def test_popularity_cannot_hijack_album_query():
    """A huge-view unrelated song must not beat the requested album's tracks."""
    junk = mk("j", "Chicago Freestyle", "Drake", source="ytmusic", badges=["has-metadata"])
    junk.views = 500_000_000
    track = mk("t", "9", "Drake", source="ytmusic", badges=["has-metadata"], album="Views")
    assert rank([junk, track], query="Drake Views", top_artist="Drake")[0].videoId == "t"


def test_similar_artist_name_does_not_steal_query():
    """"Change j cole" must prefer J. Cole's song over Keyshia Cole's."""
    exact = mk("a", "J. Cole - Change", "Wole E", source="youtube")
    exact.views = 50_000_000
    nearmiss = mk("b", "You've Changed", "Keyshia Cole", source="ytmusic", badges=["has-metadata"])
    nearmiss.views = 200_000_000
    assert rank([nearmiss, exact], query="Change j cole", top_artist="J. Cole")[0].videoId == "a"


def test_official_audio_beats_music_video():
    """For library intent the clean audio outranks the music video."""
    audio = mk("a", "Drake - Hotline Bling (Official Audio)", "Drake Media", source="youtube")
    audio.views = 3_177_174
    video = mk("v", "Drake - Hotline Bling", "Drake", source="youtube")
    video.views = 2_155_424_848
    assert rank([video, audio], query="hotline bling", top_artist="Drake")[0].videoId == "a"


def test_artist_prefixed_youtube_title_matches_clean():
    """"Drake - Hotline Bling" must rank as the exact song, not a fuzzy cousin."""
    official = mk("r", "Drake - Hotline Bling", "Drake", source="youtube")
    official.views = 2_155_424_848
    cover = mk("c", "Hotline Bling", "William Singe", source="ytmusic", badges=["has-metadata"])
    cover.views = 24_000_000
    assert rank([cover, official], query="hotline bling", top_artist="Drake")[0].videoId == "r"


def test_album_intent_pins_album_and_tracks():
    from zayfetch.ranking import is_album_intent

    assert is_album_intent("iceman drake", "ICEMAN", "Drake")
    assert is_album_intent("Drake Views", "Views", "Drake")
    assert not is_album_intent("hotline bling", "Hotline Bling", "Brandon Lofi")
    assert not is_album_intent("Drake Passionfruit", "Passionfruit (Karaoke Version)", "Release")
    track = mk("t", "Make Them Cry", "Drake", source="ytmusic", album="ICEMAN")
    junk = mk("j", "Drake - Iceman Album (Playlist)", "El Camarada", source="youtube")
    ranked = rank([junk, track], query="iceman drake", top_artist="Drake", pinned_album="ICEMAN")
    assert ranked[0].videoId == "t"


def test_album_intent_rejects_lookalike_artist():
    """A knockoff album ("One Dance" by "Drakee") must never pin over the
    real record just because the names fuzz-match."""
    from zayfetch.ranking import is_album_intent

    assert not is_album_intent("One Dance Drake", "One Dance", "Drakee")


def test_absurd_durations_demoted_not_removed():
    long = mk("v", "Drake Iceman Full Album Video", "SomeChannel", source="youtube", dur=1557.0)
    short = mk("s", "Make Them Cry", "Drake", source="ytmusic", album="ICEMAN")
    ranked = rank([long, short], query="iceman drake", top_artist="Drake", pinned_album="ICEMAN")
    assert len(ranked) == 2
    assert ranked[0].videoId == "s"
