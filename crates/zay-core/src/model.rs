use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplayGain {
    pub track_gain: Option<f64>,
    pub fallback_gain: Option<f64>,
    pub base_gain: Option<f64>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    #[serde(deserialize_with = "nullable")]
    pub title: String,
    #[serde(deserialize_with = "nullable")]
    pub artist: String,
    #[serde(deserialize_with = "nullable")]
    pub album: String,
    pub starred: Option<String>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub duration: f64,
    pub suffix: Option<String>,
    pub content_type: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub preview_url: Option<String>,
    pub artwork_url: Option<String>,
    pub via_radio: bool,
    pub replay_gain: Option<ReplayGain>,
}
fn nullable<'de, T: Deserialize<'de> + Default, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
impl Song {
    pub fn gain(&self) -> f64 {
        self.replay_gain
            .as_ref()
            .map(|r| {
                10_f64
                    .powf(
                        (r.track_gain.or(r.fallback_gain).unwrap_or(0.)
                            + r.base_gain.unwrap_or(0.))
                            / 20.,
                    )
                    .clamp(0., 1.)
            })
            .unwrap_or(1.)
    }
}
pub fn sort_album(songs: &mut [Song]) {
    songs.sort_by_key(|s| {
        (
            s.disc_number.filter(|n| *n > 0).unwrap_or(1),
            s.track.filter(|n| *n > 0).unwrap_or(u32::MAX),
        )
    });
}
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .trim_end_matches(" - topic")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn fresh_radio(existing: &[Song], recommendations: Vec<Song>) -> Vec<Song> {
    let mut ids: std::collections::HashSet<_> = existing.iter().map(|s| s.id.clone()).collect();
    let mut recordings: std::collections::HashSet<_> = existing
        .iter()
        .map(|s| (normalize(&s.artist), normalize(&s.title)))
        .collect();
    recommendations
        .into_iter()
        .filter(|s| {
            let key = (normalize(&s.artist), normalize(&s.title));
            if ids.contains(&s.id) || recordings.contains(&key) {
                return false;
            }
            ids.insert(s.id.clone());
            recordings.insert(key);
            true
        })
        .collect()
}
