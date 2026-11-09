use poise::{
    CreateReply,
    serenity_prelude::{
        CreateActionRow, CreateAttachment, CreateButton, CreateEmbed, futures::future::join_all,
    },
};
use reqwest::{Client, RequestBuilder, Url};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, iter,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

const SPOTIFY: &str = "open.spotify.com";
const CACHE_TTL: Duration = Duration::from_hours(6);
/// Discord's smallest upload limit is 10 MB, and previews are around 1 MB.
const MAX_PREVIEW_BYTES: usize = 8_000_000;

static HTTP: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to build the HTTP client")
});
/// Spotify's API needs an app token, which lasts an hour.
static TOKEN: Mutex<Option<(Instant, String)>> = Mutex::new(None);
static CACHE: LazyLock<Mutex<HashMap<Url, (Instant, Song)>>> = LazyLock::new(Mutex::default);

#[derive(Clone, Default)]
struct Song {
    is_album: bool,
    title: String,
    artist: String,
    details: String,
    art: Option<String>,
    /// Which service the preview came from, and its URL.
    preview: Option<(&'static str, String)>,
    spotify: Option<String>,
    apple: Option<String>,
}

impl Song {
    /// Fills in whatever `other` (the same song on the other service) knows and this doesn't.
    fn merge(&mut self, other: Self) {
        self.spotify = self.spotify.take().or(other.spotify);
        self.apple = self.apple.take().or(other.apple);
        self.preview = self.preview.take().or(other.preview);
        self.art = self.art.take().or(other.art);
    }

    /// Whether `other` is this song on the other service: names match if one contains the other.
    fn is(&self, other: &Self) -> bool {
        let same = |a: &str, b: &str| {
            let (a, b) = (a.to_lowercase(), b.to_lowercase());
            a.contains(&b) || b.contains(&a)
        };
        same(&self.title, &other.title) && same(&self.artist, &other.artist)
    }
}

async fn get(request: RequestBuilder) -> Option<Value> {
    request.send().await.ok()?.json().await.ok()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().filter(|s| !s.is_empty()).map(String::from)
}

fn details(album: Option<String>, date: Option<String>, millis: Option<u64>) -> String {
    let clock = |ms: u64| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60);
    [
        album,
        date.and_then(|d| d.get(..4).map(String::from)),
        millis.map(clock),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

fn credentials() -> Option<(String, String)> {
    Some((
        env::var("SPOTIFY_CLIENT_ID").ok()?,
        env::var("SPOTIFY_CLIENT_SECRET").ok()?,
    ))
}

/// Warns at startup if Spotify links can't be looked up.
pub fn init() {
    if credentials().is_none() {
        eprintln!("SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET not set: Spotify links won't work");
    }
}

/// A client-credentials token for the Spotify app, cached until it expires.
async fn spotify_token() -> Option<String> {
    let cached = TOKEN
        .lock()
        .unwrap()
        .clone()
        .filter(|(expires, _)| *expires > Instant::now());
    if let Some((_, token)) = cached {
        return Some(token);
    }
    let (id, secret) = credentials()?;
    let response = get(HTTP
        .post("https://accounts.spotify.com/api/token")
        .basic_auth(id, Some(secret))
        .form(&[("grant_type", "client_credentials")]))
    .await?;
    let token = text(&response, "access_token")?;
    let lifetime = Duration::from_secs(response["expires_in"].as_u64()?.saturating_sub(60));
    *TOKEN.lock().unwrap() = Some((Instant::now() + lifetime, token.clone()));
    Some(token)
}

async fn spotify_api(path: &str, query: &[(&str, &str)]) -> Option<Value> {
    get(HTTP
        .get(format!("https://api.spotify.com/v1/{path}"))
        .bearer_auth(spotify_token().await?)
        .query(query))
    .await
}

/// A Spotify track or album, from the API's JSON for either (or a search result).
fn spotify_song(t: &Value) -> Option<Song> {
    let is_album = t["type"] == "album";
    let album = if is_album { t } else { &t["album"] };
    let artist = t["artists"]
        .as_array()?
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(Song {
        is_album,
        title: text(t, "name")?,
        artist: Some(artist).filter(|a| !a.is_empty())?,
        details: details(
            text(album, "name").filter(|_| !is_album),
            text(album, "release_date"),
            t["duration_ms"].as_u64(),
        ),
        art: text(&album["images"][0], "url"),
        preview: text(t, "preview_url").map(|u| ("Spotify", u)),
        spotify: text(&t["external_urls"], "spotify"),
        ..Song::default()
    })
}

async fn spotify(url: &Url) -> Option<Song> {
    let mut parts = url
        .path_segments()?
        .skip_while(|p| !matches!(*p, "track" | "album"));
    let (kind, id) = (parts.next()?, parts.next()?);
    spotify_song(&spotify_api(&format!("{kind}s/{id}"), &[]).await?)
}

async fn spotify_match(song: &Song) -> Option<Song> {
    let kind = if song.is_album { "album" } else { "track" };
    let query = format!("{} {}", song.artist, song.title);
    let found = spotify_api("search", &[("q", &query), ("type", kind), ("limit", "5")]).await?;
    found[format!("{kind}s")]["items"]
        .as_array()?
        .iter()
        .filter_map(spotify_song)
        .find(|candidate| song.is(candidate))
}

/// A song or album from iTunes results. An album lookup returns the album then its songs, so the first
/// result describes it and any result may have the preview.
fn itunes_song(results: &[Value], is_album: bool) -> Option<Song> {
    let t = results.first()?;
    Some(Song {
        is_album,
        title: text(t, "trackName").or_else(|| text(t, "collectionName"))?,
        artist: text(t, "artistName")?,
        details: details(
            text(t, "collectionName").filter(|_| !is_album),
            text(t, "releaseDate"),
            t["trackTimeMillis"].as_u64(),
        ),
        art: text(t, "artworkUrl100").map(|a| a.replace("100x100bb", "640x640bb")),
        preview: results
            .iter()
            .find_map(|t| text(t, "previewUrl"))
            .map(|u| ("Apple Music", u)),
        apple: text(t, "trackViewUrl").or_else(|| text(t, "collectionViewUrl")),
        ..Song::default()
    })
}

/// The `i` query parameter, which Apple Music uses to pick a song within an album link.
fn song_id(url: &Url) -> Option<String> {
    url.query_pairs()
        .find(|(k, _)| k == "i")
        .map(|(_, id)| id.into_owned())
}

async fn apple(url: &Url) -> Option<Song> {
    let song_id = song_id(url);
    let is_album = song_id.is_none() && !url.path().contains("/song/");
    let id = song_id.or_else(|| url.path_segments()?.next_back().map(String::from))?;
    let found = get(HTTP.get("https://itunes.apple.com/lookup").query(&[
        ("id", id.as_str()),
        ("entity", "song"),
        ("limit", "5"),
    ]))
    .await?;
    Some(Song {
        apple: Some(url.to_string()),
        ..itunes_song(found["results"].as_array()?, is_album)?
    })
}

async fn apple_match(song: &Song) -> Option<Song> {
    let kind = if song.is_album { "album" } else { "song" };
    let query = format!("{} {}", song.artist, song.title);
    let found = get(HTTP.get("https://itunes.apple.com/search").query(&[
        ("term", query.as_str()),
        ("entity", kind),
        ("limit", "5"),
    ]))
    .await?;
    found["results"]
        .as_array()?
        .iter()
        .filter_map(|t| itunes_song(std::slice::from_ref(t), song.is_album))
        .find(|candidate| song.is(candidate))
}

async fn lookup(url: &Url) -> Option<Song> {
    let cached = CACHE
        .lock()
        .unwrap()
        .get(url)
        .filter(|(at, _)| at.elapsed() < CACHE_TTL)
        .map(|(_, s)| s.clone());
    if cached.is_some() {
        return cached;
    }

    let from_spotify = url.host_str() == Some(SPOTIFY);
    let mut song = if from_spotify {
        spotify(url).await?
    } else {
        apple(url).await?
    };
    let other = if from_spotify {
        apple_match(&song).await
    } else {
        spotify_match(&song).await
    };
    if let Some(other) = other {
        song.merge(other);
    }

    {
        let mut cache = CACHE.lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        cache.insert(url.clone(), (Instant::now(), song.clone()));
    }
    Some(song)
}

/// A supported song URL, with tracking parameters dropped so a song is cached once.
fn normalize(url: &Url) -> Option<Url> {
    let mut url = url.clone();
    if ![SPOTIFY, "music.apple.com"].contains(&url.host_str()?) {
        return None;
    }
    let query = song_id(&url).map(|id| format!("i={id}"));
    url.set_query(query.as_deref());
    Some(url)
}

/// Downloads the preview so Discord shows its native audio player, on mobile too.
async fn download_preview(song: &Song) -> Option<CreateAttachment> {
    let (source, url) = song.preview.as_ref()?;
    let response = HTTP.get(url).send().await.ok()?.error_for_status().ok()?;
    let bytes = response.bytes().await.ok()?;
    let ext = if url.contains(".m4a") { "m4a" } else { "mp3" };
    let name = format!("{} - {} ({source} preview).{ext}", song.title, song.artist)
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "");
    (bytes.len() <= MAX_PREVIEW_BYTES).then(|| CreateAttachment::bytes(bytes.to_vec(), name))
}

/// The card for a song, then its audio preview as a second message if there is one.
async fn card(url: &Url) -> Vec<CreateReply> {
    let Some(song) = lookup(url).await else {
        return Vec::new();
    };
    let mut embed = CreateEmbed::new()
        .title(&song.title)
        .description(format!("**{}**\n{}", song.artist, song.details))
        .colour(if song.spotify.is_some() {
            0x001d_b954
        } else {
            0x00fa_243c
        });
    if let Some(url) = song.spotify.as_ref().or(song.apple.as_ref()) {
        embed = embed.url(url);
    }
    if let Some(art) = &song.art {
        embed = embed.thumbnail(art);
    }
    let buttons = [("Spotify", &song.spotify), ("Apple Music", &song.apple)]
        .into_iter()
        .filter_map(|(name, url)| Some(CreateButton::new_link(url.as_deref()?).label(name)))
        .collect();
    let card = CreateReply::default()
        .embed(embed)
        .components(vec![CreateActionRow::Buttons(buttons)]);
    let preview = download_preview(&song).await;
    iter::once(card)
        .chain(preview.map(|p| CreateReply::default().attachment(p)))
        .collect()
}

/// Messages for the Spotify and Apple Music links among `urls` (a card and maybe a preview each), deduplicated and resolved concurrently, in order.
pub async fn cards(urls: &[Url]) -> Vec<CreateReply> {
    let mut seen = HashSet::new();
    let urls: Vec<Url> = urls
        .iter()
        .filter_map(normalize)
        .filter(|url| seen.insert(url.clone()))
        .collect();
    join_all(urls.iter().map(card))
        .await
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Song;
    use super::spotify_song;
    use serde_json::json;

    #[test]
    fn matches_a_song_across_services() {
        let apple = Song {
            title: "Example Song".into(),
            artist: "Example Artist".into(),
            ..Song::default()
        };
        let spotify = spotify_song(&json!({
            "type": "track", "name": "Example Song (Remastered)", "duration_ms": 185_000,
            "artists": [{"name": "Other"}, {"name": "Example Artist"}],
            "album": {"name": "Album", "release_date": "2020-01-01",
                      "images": [{"url": "https://images.test/cover.jpg"}]},
            "external_urls": {"spotify": "https://open.spotify.com/track/1"},
            "preview_url": "https://audio.test/preview.mp3"
        }))
        .unwrap();
        assert!(apple.is(&spotify));
        assert_eq!(spotify.details, "Album · 2020 · 3:05");
        assert_eq!(
            spotify.art.as_deref(),
            Some("https://images.test/cover.jpg")
        );
        assert_eq!(spotify.preview.unwrap().1, "https://audio.test/preview.mp3");
        let other = Song {
            artist: "Nobody".into(),
            ..apple
        };
        assert!(
            !other.is(&spotify_song(
                &json!({"name": "Example Song", "artists": [{"name": "Example Artist"}]})
            )
            .unwrap())
        );
    }
}
