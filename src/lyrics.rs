use reqwest::StatusCode;
use serde::Deserialize;
use std::{error::Error, fmt, time::Duration};

const BINILYRICS_SEARCH_URL: &str = "https://lyrics-api.binimum.org/getLyrics";
const BINILYRICS_STORAGE_HOST: &str = "lyrics-storage.binimum.org";
const LRC_RED_HOST: &str = "lrc.red";
const LRCLIB_URL: &str = "https://lrclib.net/api/get";
const LRCLIB_SEARCH_URL: &str = "https://lrclib.net/api/search";
const UNISON_URL: &str = "https://unison.boidu.dev/lyrics";
const KPOE_MIRRORS: &[&str] = &[
    "https://lyricsplus.prjktla.my.id",
    "https://lyricsplus.binimum.org",
    "https://lyricsplus.prjktla.workers.dev",
];
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Title tokens that describe the *upload* rather than the song. A bracketed
/// group containing any of these is dropped before querying a provider.
const TITLE_NOISE_TOKENS: &[&str] = &[
    "official",
    "lyric",
    "lyrics",
    "clip",
    "visualizer",
    "visualiseur",
    "trailer",
    "teaser",
    "live",
    "remaster",
    "remastered",
    "remasterise",
    "sped",
    "slowed",
    "reverb",
    "nightcore",
    "audio",
    "video",
    "feat",
    "ft",
    "featuring",
    "4k",
    "1080p",
    "720p",
    "hd",
    "hq",
];

/// Same idea, but matched against the separator-free form so `M/V`, `Clip
/// Officiel` and `Bande Annonce` are recognised as single units.
const TITLE_NOISE_PHRASES: &[&str] = &[
    "mv",
    "mvofficial",
    "clipofficiel",
    "videoversio",
    "videoriginal",
    "bandeannonce",
    "lyrique",
    "paroles",
    "letra",
    "letras",
];

/// Channel-name decorations that are never part of a credited artist.
const ARTIST_NOISE_TOKENS: &[&str] = &["topic", "official"];

/// Channel-name decorations glued onto the end of an artist token.
const ARTIST_NOISE_SUFFIXES: &[&str] = &["vevo", "topic"];

/// Artists a provider has no real name for, so matching must not require one.
const PLACEHOLDER_ARTISTS: &[&str] = &["unknown artist", "unknown artists", "unknown"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsTiming {
    Plain,
    LineSynced,
    SyllableSynced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricSyllable {
    pub text: String,
    pub tail: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricLine {
    pub text: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub syllables: Vec<LyricSyllable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lyrics {
    pub timing: LyricsTiming,
    pub lines: Vec<LyricLine>,
    pub source: String,
    pub songwriters: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LyricsTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_seconds: Option<u64>,
    pub video_id: Option<String>,
}

#[derive(Clone)]
pub struct LyricsClient {
    client: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TtmlError(String);

impl fmt::Display for TtmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for TtmlError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsFetchError(String);

impl fmt::Display for LyricsFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for LyricsFetchError {}

impl LyricsClient {
    pub fn new() -> Result<Self, LyricsFetchError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("ytui/0.1")
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if allowed_provider_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|error| {
                LyricsFetchError(format!("failed to create lyrics client: {error}"))
            })?;
        Ok(Self { client })
    }

    pub async fn fetch(&self, track: LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        let (bini, kpoe, unison, lrclib) = tokio::join!(
            self.fetch_binilyrics(&track),
            self.fetch_kpoe(&track),
            self.fetch_unison(&track),
            self.fetch_lrclib(&track)
        );
        let candidates = [
            bini.as_ref().ok(),
            kpoe.as_ref().ok(),
            unison.as_ref().ok(),
            lrclib.as_ref().ok(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|lyrics| lyrics.clone())
        .collect::<Vec<_>>();

        if let Some(lyrics) = Lyrics::best_available(candidates) {
            return Ok(Some(lyrics));
        }

        match (bini, kpoe, unison, lrclib) {
            (Err(bini), Err(kpoe), Err(unison), Err(lrclib)) => Err(LyricsFetchError(format!(
                "lyrics providers failed: {bini}; {kpoe}; {unison}; {lrclib}"
            ))),
            (Err(error), Ok(None), Ok(None), Ok(None)) => Err(error),
            (Ok(None), Err(error), Ok(None), Ok(None)) => Err(error),
            (Ok(None), Ok(None), Err(error), Ok(None)) => Err(error),
            (Ok(None), Ok(None), Ok(None), Err(error)) => Err(error),
            _ => Ok(None),
        }
    }

    async fn fetch_binilyrics(
        &self,
        track: &LyricsTrack,
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let terms = SearchTerms::from_track(track);
        let query = format!("{} {}", terms.title, terms.artist);
        let response = self
            .client
            .get(BINILYRICS_SEARCH_URL)
            .query(&[("q", query)])
            .send()
            .await
            .map_err(provider_error("BiniLyrics search"))?;
        let Some(body) = response_body(response, "BiniLyrics search").await? else {
            return Ok(None);
        };
        let response: BiniLyricsResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid BiniLyrics response: {error}")))?;
        let Some(result) = best_bini_match(track, response.results) else {
            return Ok(None);
        };
        let lyrics_url = reqwest::Url::parse(&result.lyrics_url)
            .map_err(|error| LyricsFetchError(format!("invalid BiniLyrics URL: {error}")))?;
        if lyrics_url.scheme() != "https"
            || !matches!(
                lyrics_url.host_str(),
                Some(BINILYRICS_STORAGE_HOST | LRC_RED_HOST)
            )
        {
            return Err(LyricsFetchError(
                "BiniLyrics returned an unsupported lyrics URL".to_owned(),
            ));
        }

        let response = self
            .client
            .get(lyrics_url)
            .send()
            .await
            .map_err(provider_error("BiniLyrics TTML"))?;
        let Some(body) = response_body(response, "BiniLyrics TTML").await? else {
            return Ok(None);
        };
        let document = std::str::from_utf8(&body)
            .map_err(|error| LyricsFetchError(format!("BiniLyrics TTML is not UTF-8: {error}")))?;
        let mut lyrics = Lyrics::from_ttml(document)
            .map_err(|error| LyricsFetchError(format!("invalid BiniLyrics TTML: {error}")))?;
        lyrics.source = "Apple (via BiniLyrics)".to_owned();
        Ok(Some(lyrics))
    }

    async fn fetch_lrclib(&self, track: &LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        let terms = SearchTerms::from_track(track);

        let mut query = vec![
            ("track_name", terms.title.as_str()),
            ("artist_name", terms.artist.as_str()),
        ];
        if let Some(album) = terms.album.as_deref() {
            query.push(("album_name", album));
        }
        let duration = terms.duration.map(|duration| duration.to_string());
        if let Some(duration) = duration.as_deref() {
            query.push(("duration", duration));
        }

        if let Some(lyrics) = self.lrclib_lookup(LRCLIB_URL, query).await? {
            return Ok(lyrics);
        }

        // `/api/get` only answers exact matches, so a track uploaded as
        // "GIMS - Est-ce que tu m'aimes ? (Clip officiel)" 404s even though
        // LRCLIB holds the track. Fall back to the fuzzy search endpoint.
        let mut search = vec![("track_name", terms.title.as_str())];
        if terms.artist_known {
            search.push(("artist_name", terms.artist.as_str()));
        }

        let response = self
            .client
            .get(LRCLIB_SEARCH_URL)
            .query(&search)
            .send()
            .await
            .map_err(provider_error("LRCLIB search"))?;
        let Some(body) = response_body(response, "LRCLIB search").await? else {
            return Ok(None);
        };
        let results: Vec<LrcLibSearchResult> = serde_json::from_slice(&body).map_err(|error| {
            LyricsFetchError(format!("invalid LRCLIB search response: {error}"))
        })?;
        let Some(result) = best_lrclib_match(&terms, results) else {
            return Ok(None);
        };

        Ok(parse_lrclib(LrcLibResponse {
            synced_lyrics: result.synced_lyrics,
            plain_lyrics: result.plain_lyrics,
        }))
    }

    async fn lrclib_lookup(
        &self,
        url: &str,
        query: Vec<(&str, &str)>,
    ) -> Result<Option<Option<Lyrics>>, LyricsFetchError> {
        let response = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await
            .map_err(provider_error("LRCLIB"))?;
        let Some(body) = response_body(response, "LRCLIB").await? else {
            return Ok(None);
        };
        let response: LrcLibResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid LRCLIB response: {error}")))?;
        Ok(Some(parse_lrclib(response)))
    }

    async fn fetch_unison(&self, track: &LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        if let Some(video_id) = track.video_id.as_deref()
            && let Ok(Some(lyrics)) = self.fetch_unison_query(vec![("v", video_id)]).await
        {
            return Ok(Some(lyrics));
        }

        let terms = SearchTerms::from_track(track);
        let mut query = vec![
            ("song", terms.title.as_str()),
            ("artist", terms.artist.as_str()),
        ];
        if let Some(album) = terms.album.as_deref() {
            query.push(("album", album));
        }
        let duration = terms.duration.map(|duration| duration.to_string());
        if let Some(duration) = duration.as_deref() {
            query.push(("duration", duration));
        }
        self.fetch_unison_query(query).await
    }

    async fn fetch_unison_query(
        &self,
        query: Vec<(&str, &str)>,
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let response = self
            .client
            .get(UNISON_URL)
            .query(&query)
            .send()
            .await
            .map_err(provider_error("Unison"))?;
        let Some(body) = response_body(response, "Unison").await? else {
            return Ok(None);
        };
        let response: UnisonResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid Unison response: {error}")))?;
        Ok(parse_unison(response))
    }

    /// Lyrics+ aggregates Apple Music, QQ, Musixmatch and Spotify, which is where
    /// most word-synced lyrics come from. It has no stable host, so try each
    /// mirror in turn and keep the first that answers.
    async fn fetch_kpoe(&self, track: &LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        let terms = SearchTerms::from_track(track);
        let duration = terms
            .duration
            .map(|duration| duration.to_string())
            .unwrap_or_default();
        let mut query = vec![
            ("title", terms.title.as_str()),
            ("artist", terms.artist.as_str()),
        ];
        if let Some(album) = terms.album.as_deref() {
            query.push(("album", album));
        }
        if !duration.is_empty() {
            query.push(("duration", duration.as_str()));
        }

        let mut last_error = None;
        for (index, mirror) in KPOE_MIRRORS.iter().enumerate() {
            match self.kpoe_request(mirror, "/v1/ttml/get", &query).await {
                Ok(Some(lyrics)) => return Ok(Some(lyrics)),
                Ok(None) => {}
                Err(error) => last_error = Some(error),
            }

            // The first mirror is the maintained one, so it is also the only one
            // worth a second ask for the JSON format.
            if index == 0
                && let Ok(Some(lyrics)) = self.kpoe_request(mirror, "/v2/lyrics/get", &query).await
            {
                return Ok(Some(lyrics));
            }
        }

        match last_error {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }

    async fn kpoe_request(
        &self,
        mirror: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let url = format!("{mirror}{path}");
        let response = self
            .client
            .get(&url)
            .query(query)
            .header(
                "X-Client-Package",
                "ytui <https://github.com/itsmenewbie03/ytui>",
            )
            .send()
            .await
            .map_err(provider_error("Lyrics+"))?;
        let Some(body) = response_body(response, "Lyrics+").await? else {
            return Ok(None);
        };
        let response: KpoeResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid Lyrics+ response: {error}")))?;

        // `/v1/ttml/get` already speaks the TTML we parse for Apple Music, which
        // keeps syllable timing exactly as the label published it.
        if let Some(document) = response.ttml.as_deref()
            && let Ok(lyrics) = Lyrics::from_ttml(document)
        {
            return Ok(Some(Lyrics {
                source: "Lyrics+".to_owned(),
                ..lyrics
            }));
        }

        Ok(parse_kpoe(response))
    }
}

impl Lyrics {
    pub fn best_available(candidates: impl IntoIterator<Item = Self>) -> Option<Self> {
        candidates.into_iter().fold(None, |best, candidate| {
            if best
                .as_ref()
                .is_none_or(|current| candidate.timing > current.timing)
            {
                Some(candidate)
            } else {
                best
            }
        })
    }

    pub fn footer_line_count(&self) -> usize {
        2 + usize::from(!self.songwriters.is_empty())
    }

    pub fn from_ttml(document: &str) -> Result<Self, TtmlError> {
        let document = roxmltree::Document::parse(document)
            .map_err(|error| TtmlError(format!("invalid TTML: {error}")))?;
        let mut songwriters = Vec::new();
        for songwriter in document.descendants().filter(|node| {
            node.is_element() && node.tag_name().name().eq_ignore_ascii_case("songwriter")
        }) {
            let name = songwriter.text().unwrap_or_default().trim();
            if !name.is_empty() && !songwriters.iter().any(|existing| existing == name) {
                songwriters.push(name.to_owned());
            }
        }
        let body = document
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "body")
            .ok_or_else(|| TtmlError("TTML has no body".to_owned()))?;
        let mut lines = Vec::new();

        for paragraph in body
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "p")
        {
            let paragraph_start = parse_optional_time(paragraph.attribute("begin"))?;
            let paragraph_end = resolve_end(paragraph, paragraph_start, None)?;
            let syllables = timed_spans(paragraph, paragraph_end)?;
            let text = if syllables.is_empty() {
                normalized_text(paragraph)
            } else {
                syllables
                    .iter()
                    .map(|syllable| format!("{}{}", syllable.text, syllable.tail))
                    .collect::<String>()
                    .trim()
                    .to_owned()
            };
            if text.is_empty() {
                continue;
            }

            let start_ms =
                paragraph_start.or_else(|| syllables.first().map(|syllable| syllable.start_ms));
            let end_ms = paragraph_end.or_else(|| syllables.last().map(|syllable| syllable.end_ms));

            if matches!((start_ms, end_ms), (Some(start), Some(end)) if end < start) {
                return Err(TtmlError("lyric line ends before it begins".to_owned()));
            }
            if syllables.iter().any(|syllable| {
                start_ms.is_some_and(|start| syllable.start_ms < start)
                    || end_ms.is_some_and(|end| syllable.end_ms > end)
            }) {
                return Err(TtmlError(
                    "timed span falls outside its lyric line".to_owned(),
                ));
            }

            lines.push(LyricLine {
                text,
                start_ms,
                end_ms,
                syllables,
            });
        }

        if lines.is_empty() {
            return Err(TtmlError("TTML contains no lyric lines".to_owned()));
        }

        let timing = lines
            .iter()
            .map(|line| {
                if !line.syllables.is_empty() {
                    LyricsTiming::SyllableSynced
                } else if line.start_ms.is_some() && line.end_ms.is_some() {
                    LyricsTiming::LineSynced
                } else {
                    LyricsTiming::Plain
                }
            })
            .min()
            .expect("lyrics are known to contain at least one line");

        Ok(Self {
            timing,
            lines,
            source: "TTML".to_owned(),
            songwriters,
        })
    }
}

#[derive(Deserialize)]
struct BiniLyricsResponse {
    #[serde(default)]
    results: Vec<BiniLyricsResult>,
}

#[derive(Deserialize)]
struct BiniLyricsResult {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    duration: f64,
    #[serde(rename = "lyricsUrl")]
    lyrics_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibResponse {
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibSearchResult {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    duration: f64,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

#[derive(Deserialize)]
struct KpoeResponse {
    #[serde(default)]
    ttml: Option<String>,
    #[serde(default, rename = "type")]
    timing: String,
    #[serde(default)]
    metadata: Option<KpoeMetadata>,
    #[serde(default)]
    lyrics: Vec<KpoeLine>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KpoeMetadata {
    #[serde(default)]
    source: String,
    #[serde(default)]
    song_writers: Vec<String>,
}

#[derive(Deserialize)]
struct KpoeLine {
    #[serde(default)]
    time: Option<u64>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    syllabus: Vec<KpoeSyllable>,
}

#[derive(Deserialize)]
struct KpoeSyllable {
    #[serde(default)]
    time: Option<u64>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct UnisonResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<UnisonData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnisonData {
    #[serde(default)]
    format: String,
    #[serde(default)]
    sync_type: String,
    #[serde(default)]
    lyrics: String,
}

async fn response_body(
    mut response: reqwest::Response,
    provider: &str,
) -> Result<Option<Vec<u8>>, LyricsFetchError> {
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(LyricsFetchError(format!(
            "{provider} returned HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(LyricsFetchError(format!(
            "{provider} response exceeds {MAX_RESPONSE_BYTES} bytes"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(provider_error(provider))? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(LyricsFetchError(format!(
                "{provider} response exceeds {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

fn provider_error(provider: &str) -> impl FnOnce(reqwest::Error) -> LyricsFetchError + '_ {
    move |error| LyricsFetchError(format!("{provider} request failed: {error}"))
}

fn allowed_provider_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some(
                "lyrics-api.binimum.org"
                    | "lyrics-storage.binimum.org"
                    | "lrc.red"
                    | "lrclib.net"
                    | "unison.boidu.dev"
                    | "lyricsplus.prjktla.my.id"
                    | "lyricsplus.binimum.org"
                    | "lyricsplus.prjktla.workers.dev"
            )
        )
}

fn best_bini_match(
    track: &LyricsTrack,
    results: Vec<BiniLyricsResult>,
) -> Option<BiniLyricsResult> {
    let terms = SearchTerms::from_track(track);
    results
        .into_iter()
        .filter(|result| !result.lyrics_url.is_empty())
        .filter_map(|result| {
            let score = match_score(
                &terms,
                &result.track_name,
                &result.artist_name,
                Some(result.duration),
            )?;
            Some((score, result))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, result)| result)
}

/// Picks the best LRCLIB search hit. Search is fuzzy, so unlike `/api/get` it
/// routinely returns several near-misses that need ranking.
fn best_lrclib_match(
    terms: &SearchTerms,
    results: Vec<LrcLibSearchResult>,
) -> Option<LrcLibSearchResult> {
    results
        .into_iter()
        .filter_map(|result| {
            let score = match_score(
                terms,
                &result.track_name,
                &result.artist_name,
                Some(result.duration),
            )?;
            Some((score, result))
        })
        .max_by_key(|(score, result)| {
            (
                *score,
                result.synced_lyrics.is_some(),
                result.plain_lyrics.is_some(),
            )
        })
        .map(|(_, result)| result)
}

fn normalized_match_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// The provider-facing view of a track: upload noise removed, and a flag for
/// whether the artist is real enough to insist on when matching results.
#[derive(Clone, Debug, PartialEq)]
struct SearchTerms {
    title: String,
    artist: String,
    album: Option<String>,
    duration: Option<f64>,
    artist_known: bool,
}

impl SearchTerms {
    fn from_track(track: &LyricsTrack) -> Self {
        let artist = clean_artist(&track.artist);
        let normalized_artist = normalized_match_text(&artist);
        let artist_known = !normalized_artist.is_empty()
            && !PLACEHOLDER_ARTISTS
                .iter()
                .any(|placeholder| normalized_artist == normalized_match_text(placeholder));

        Self {
            title: clean_title(&track.title, &artist),
            artist,
            album: track.album.as_deref().map(|album| album.trim().to_owned()),
            duration: track.duration_seconds.map(|duration| duration as f64),
            artist_known,
        }
    }
}

/// Removes upload noise from a raw video title: bracketed groups and trailing
/// ` - ...` segments that only describe the upload. Returns the original when
/// stripping would leave nothing behind.
fn clean_title(title: &str, artist: &str) -> String {
    let mut current = strip_artist_prefix(title.trim(), artist).trim();

    loop {
        current = trim_separators(current);
        let before = current;

        if let Some((open, inner)) = bracketed_suffix(current) {
            if is_title_noise(inner) {
                current = current[..open].trim_end();
            }
        } else if let Some(index) = trailing_segment(current)
            && is_title_noise(&current[index..])
        {
            current = current[..index].trim_end();
        }

        if current == before {
            break;
        }
    }

    let cleaned = trim_separators(current);
    if cleaned.is_empty() {
        title.trim().to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// Byte offset of the last ` - ` separator, whose trailing segment is the
/// next candidate for removal.
fn trailing_segment(value: &str) -> Option<usize> {
    value.rfind(" - ")
}

/// Drops a leading `ARTIST - ` prefix, but only when it really is the artist.
fn strip_artist_prefix<'a>(title: &'a str, artist: &str) -> &'a str {
    let normalized_artist = normalized_match_text(artist);
    if normalized_artist.is_empty() {
        return title;
    }

    for separator in [" - ", " – ", " — ", " | "] {
        let Some(index) = title.find(separator) else {
            continue;
        };
        let candidate = normalized_match_text(&title[..index]);
        if !candidate.is_empty()
            && (candidate == normalized_artist
                || candidate.contains(&normalized_artist)
                || normalized_artist.contains(&candidate))
        {
            return &title[index + separator.len()..];
        }
    }

    title
}

/// Returns the byte offset and inner text of a trailing `(...)` or `[...]` group.
fn bracketed_suffix(value: &str) -> Option<(usize, &str)> {
    let (open, close) = match value.as_bytes().last()? {
        b')' => (b'(', b')'),
        b']' => (b'[', b']'),
        _ => return None,
    };
    let open = value.rfind(open as char)?;
    let inner = &value[open + 1..value.len() - 1];
    (!inner.contains(close as char) && !inner.contains('\n')).then_some((open, inner))
}

fn is_title_noise(inner: &str) -> bool {
    let tokens = tokens(inner);
    if tokens
        .iter()
        .any(|token| TITLE_NOISE_TOKENS.contains(&token.as_str()))
    {
        return true;
    }
    TITLE_NOISE_PHRASES.contains(&tokens.concat().as_str())
}

/// Splits on any non-alphanumeric run and lowercases, so `feat. Jane` and
/// `M/V` tokenize the way a reader would.
fn tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Trims channel decoration from an artist while preserving its original
/// casing, since the cleaned value is still sent to providers as a query.
fn clean_artist(artist: &str) -> String {
    let mut kept = split_words(artist);
    loop {
        // `GIMS - Topic` leaves a dangling `-` once the marker is dropped.
        while let Some(last) = kept.last()
            && !last.chars().any(char::is_alphanumeric)
        {
            kept.pop();
        }
        let Some(last) = kept.last_mut() else {
            break;
        };
        let lowered = last.to_lowercase();
        if ARTIST_NOISE_TOKENS.contains(&lowered.as_str()) {
            kept.pop();
            continue;
        }
        match ARTIST_NOISE_SUFFIXES.iter().find_map(|suffix| {
            lowered
                .strip_suffix(suffix)
                .filter(|stem| !stem.is_empty())
                .map(str::len)
        }) {
            Some(new_len) => last.truncate(new_len),
            None => break,
        }
    }
    kept.join(" ")
}

/// Splits on whitespace only, so casing and punctuation survive.
fn split_words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

fn trim_separators(value: &str) -> &str {
    value
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(character, '-' | '–' | '—' | '|' | '·' | '/' | ',')
        })
        .trim()
}

/// Scores a provider result against the search terms, or rejects it outright.
/// Shared by BiniLyrics and LRCLIB so both rank candidates identically.
fn match_score(
    terms: &SearchTerms,
    title: &str,
    artist: &str,
    duration: Option<f64>,
) -> Option<u32> {
    let target_title = normalized_match_text(&terms.title);
    let title = normalized_match_text(title);
    let target_artist = normalized_match_text(&terms.artist);
    let artist = normalized_match_text(artist);

    // Title outweighs artist: a matching title with a `feat.` suffix on the
    // artist is still the same song, but a mismatched title is a different one.
    let title_score = if target_title.is_empty() || title.is_empty() {
        0
    } else if target_title == title {
        20
    } else if target_title.contains(&title) || title.contains(&target_title) {
        10
    } else {
        return None;
    };

    let artist_score = if target_artist.is_empty() || artist.is_empty() {
        0
    } else if target_artist == artist {
        15
    } else if target_artist.contains(&artist) || artist.contains(&target_artist) {
        10
    } else if terms.artist_known {
        return None;
    } else {
        0
    };

    let duration_score = match (terms.duration, duration) {
        (Some(target), Some(candidate)) if candidate > 0.0 => {
            let difference = (candidate - target).abs();
            if difference <= 3.0 {
                5
            } else if difference <= 8.0 {
                2
            } else {
                return None;
            }
        }
        _ => 0,
    };

    Some(title_score + artist_score + duration_score)
}

fn parse_lrclib(response: LrcLibResponse) -> Option<Lyrics> {
    if let Some(synced) = response.synced_lyrics
        && let Some(lines) = parse_lrc(&synced)
    {
        return Some(Lyrics {
            timing: LyricsTiming::LineSynced,
            lines,
            source: "LRCLIB".to_owned(),
            songwriters: Vec::new(),
        });
    }

    let lines = response
        .plain_lyrics?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|text| LyricLine {
            text: text.to_owned(),
            start_ms: None,
            end_ms: None,
            syllables: Vec::new(),
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(Lyrics {
        timing: LyricsTiming::Plain,
        lines,
        source: "LRCLIB".to_owned(),
        songwriters: Vec::new(),
    })
}

fn parse_unison(response: UnisonResponse) -> Option<Lyrics> {
    if !response.success {
        return None;
    }
    let data = response.data?;
    match data.format.as_str() {
        "ttml" => {
            let lyrics = Lyrics::from_ttml(&data.lyrics).ok();
            if let Some(mut lyrics) = lyrics {
                lyrics.source = "Unison".to_owned();
                return Some(lyrics);
            }
        }
        "lrc" => {
            if let Some(lines) = parse_lrc(&data.lyrics) {
                return Some(Lyrics {
                    timing: LyricsTiming::LineSynced,
                    lines,
                    source: "Unison".to_owned(),
                    songwriters: Vec::new(),
                });
            }
        }
        _ => {
            let timed = unison_sync_timing(&data.sync_type)
                .is_some_and(|timing| timing > LyricsTiming::Plain);
            if timed && let Some(lines) = parse_lrc(&data.lyrics) {
                return Some(Lyrics {
                    timing: LyricsTiming::LineSynced,
                    lines,
                    source: "Unison".to_owned(),
                    songwriters: Vec::new(),
                });
            }
            let lines = data
                .lyrics
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|text| LyricLine {
                    text: text.to_owned(),
                    start_ms: None,
                    end_ms: None,
                    syllables: Vec::new(),
                })
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                return Some(Lyrics {
                    timing: LyricsTiming::Plain,
                    lines,
                    source: "Unison".to_owned(),
                    songwriters: Vec::new(),
                });
            }
        }
    }
    None
}

fn unison_sync_timing(sync_type: &str) -> Option<LyricsTiming> {
    match sync_type {
        "richsync" => Some(LyricsTiming::SyllableSynced),
        "linesync" => Some(LyricsTiming::LineSynced),
        "plain" => Some(LyricsTiming::Plain),
        _ => None,
    }
}

/// Parses the Lyrics+ v2 payload, which carries timings in milliseconds with
/// `duration` rather than `end`. Syllables keep their own trailing space, which
/// is what glues consecutive words back together.
fn parse_kpoe(response: KpoeResponse) -> Option<Lyrics> {
    let mut pending = response
        .lyrics
        .into_iter()
        .filter_map(|line| {
            let start_ms = line.time?;
            let syllables = line
                .syllabus
                .into_iter()
                .filter_map(|syllable| {
                    let start_ms = syllable.time?;
                    let text = syllable.text.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let tail = if syllable.text.ends_with(char::is_whitespace) {
                        " "
                    } else {
                        ""
                    };
                    let end_ms = start_ms.saturating_add(syllable.duration.unwrap_or(0));
                    Some(LyricSyllable {
                        text: text.to_owned(),
                        tail: tail.to_owned(),
                        start_ms,
                        end_ms,
                    })
                })
                .collect::<Vec<_>>();
            let text = if syllables.is_empty() {
                let text = line.text.trim();
                if text.is_empty() {
                    return None;
                }
                text.to_owned()
            } else {
                syllables
                    .iter()
                    .map(|syllable| syllable.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            Some((
                start_ms,
                start_ms.saturating_add(line.duration.unwrap_or(0)),
                text,
                syllables,
            ))
        })
        .collect::<Vec<_>>();
    pending.sort_by_key(|(start, ..)| *start);
    if pending.is_empty() {
        return None;
    }

    let timing = if pending
        .iter()
        .any(|(_, _, _, syllables)| !syllables.is_empty())
    {
        LyricsTiming::SyllableSynced
    } else if response.timing.eq_ignore_ascii_case("line") {
        LyricsTiming::LineSynced
    } else {
        LyricsTiming::Plain
    };

    let last_index = pending.len() - 1;
    let lines = pending
        .iter()
        .enumerate()
        .map(|(index, (start_ms, end_ms, text, syllables))| LyricLine {
            text: text.clone(),
            start_ms: Some(*start_ms),
            end_ms: Some(if *end_ms > *start_ms {
                *end_ms
            } else if index == last_index {
                start_ms.saturating_add(5_000)
            } else {
                pending[index + 1].0
            }),
            syllables: syllables.clone(),
        })
        .collect::<Vec<_>>();

    let metadata = response.metadata.unwrap_or(KpoeMetadata {
        source: String::new(),
        song_writers: Vec::new(),
    });
    let source = if metadata.source.trim().is_empty() {
        "Lyrics+".to_owned()
    } else {
        format!("Lyrics+ ({})", metadata.source.trim())
    };

    Some(Lyrics {
        timing,
        lines,
        source,
        songwriters: metadata.song_writers,
    })
}

fn parse_lrc(document: &str) -> Option<Vec<LyricLine>> {
    let mut timed = document
        .lines()
        .filter_map(|line| {
            let (timestamp, text) = line.strip_prefix('[')?.split_once(']')?;
            let (minutes, seconds) = timestamp.split_once(':')?;
            let minutes = minutes.parse::<u64>().ok()?;
            let seconds = seconds.parse::<f64>().ok()?;
            if !seconds.is_finite() || seconds < 0.0 {
                return None;
            }
            let start_ms = minutes
                .checked_mul(60_000)?
                .checked_add((seconds * 1_000.0).round() as u64)?;
            let text = text.trim();
            (!text.is_empty() && text != "♪").then_some((start_ms, text.to_owned()))
        })
        .collect::<Vec<_>>();
    timed.sort_by_key(|(start, _)| *start);
    if timed.is_empty() {
        return None;
    }

    Some(
        timed
            .iter()
            .enumerate()
            .map(|(index, (start_ms, text))| LyricLine {
                text: text.clone(),
                start_ms: Some(*start_ms),
                end_ms: Some(
                    timed
                        .get(index + 1)
                        .map_or(start_ms.saturating_add(5_000), |(next, _)| *next),
                ),
                syllables: Vec::new(),
            })
            .collect(),
    )
}

fn timed_spans(
    paragraph: roxmltree::Node<'_, '_>,
    paragraph_end: Option<u64>,
) -> Result<Vec<LyricSyllable>, TtmlError> {
    let syllables = paragraph
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "span"
                && node.attribute("begin").is_some()
                && !has_ignored_role(*node)
                && !node
                    .ancestors()
                    .skip(1)
                    .take_while(|ancestor| *ancestor != paragraph)
                    .any(|ancestor| {
                        ancestor.is_element()
                            && ancestor.tag_name().name() == "span"
                            && ancestor.attribute("begin").is_some()
                    })
        })
        .map(|span| {
            let raw_text = span
                .descendants()
                .filter(|descendant| descendant.is_text())
                .filter_map(|descendant| descendant.text())
                .collect::<String>();
            let text = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
            let tail = normalized_tail(
                raw_text.chars().last().is_some_and(char::is_whitespace),
                span.next_sibling()
                    .filter(|node| node.is_text())
                    .and_then(|node| node.text())
                    .unwrap_or_default(),
            );
            let start_ms = parse_time(
                span.attribute("begin")
                    .expect("filtered spans always have a begin time"),
            )?;
            let end_ms = resolve_end(span, Some(start_ms), paragraph_end)?
                .ok_or_else(|| TtmlError("timed span has no end time".to_owned()))?;

            if text.is_empty() {
                return Err(TtmlError("timed span contains no text".to_owned()));
            }
            if end_ms < start_ms {
                return Err(TtmlError("timed span ends before it begins".to_owned()));
            }

            Ok(LyricSyllable {
                text,
                tail,
                start_ms,
                end_ms,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if syllables
        .windows(2)
        .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(TtmlError(
            "timed spans are not in chronological order".to_owned(),
        ));
    }

    Ok(syllables)
}

fn normalized_tail(text_has_trailing_space: bool, tail: &str) -> String {
    if (tail.contains('\n') || tail.contains('\r')) && tail.trim().is_empty() {
        return " ".to_owned();
    }

    let content = tail.split_whitespace().collect::<Vec<_>>().join(" ");
    if content.is_empty() {
        return if text_has_trailing_space || !tail.is_empty() {
            " ".to_owned()
        } else {
            String::new()
        };
    }

    let leading_space =
        text_has_trailing_space || tail.chars().next().is_some_and(char::is_whitespace);
    let trailing_space = tail.chars().last().is_some_and(char::is_whitespace);
    format!(
        "{}{}{}",
        if leading_space { " " } else { "" },
        content,
        if trailing_space { " " } else { "" }
    )
}

fn resolve_end(
    node: roxmltree::Node<'_, '_>,
    start_ms: Option<u64>,
    fallback: Option<u64>,
) -> Result<Option<u64>, TtmlError> {
    if let Some(end) = parse_optional_time(node.attribute("end"))? {
        return Ok(Some(end));
    }
    if let Some(duration) = parse_optional_time(node.attribute("dur"))? {
        let start = start_ms.ok_or_else(|| {
            TtmlError("TTML duration requires a corresponding begin time".to_owned())
        })?;
        return start
            .checked_add(duration)
            .map(Some)
            .ok_or_else(|| TtmlError("TTML time exceeds supported range".to_owned()));
    }
    Ok(fallback)
}

fn has_ignored_role(node: roxmltree::Node<'_, '_>) -> bool {
    node.ancestors().any(|ancestor| {
        ancestor.attributes().any(|attribute| {
            attribute.name() == "role" && matches!(attribute.value(), "x-bg" | "x-translation")
        })
    })
}

fn normalized_text(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|descendant| descendant.is_text() && !has_ignored_role(*descendant))
        .filter_map(|descendant| descendant.text())
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_optional_time(value: Option<&str>) -> Result<Option<u64>, TtmlError> {
    value.map(parse_time).transpose()
}

fn parse_time(value: &str) -> Result<u64, TtmlError> {
    let value = value.trim();
    let seconds = if let Some(milliseconds) = value.strip_suffix("ms") {
        parse_number(milliseconds, value)? / 1_000.0
    } else if let Some(seconds) = value.strip_suffix('s') {
        parse_number(seconds, value)?
    } else {
        let parts = value.split(':').collect::<Vec<_>>();
        match parts.as_slice() {
            [seconds] => parse_number(seconds, value)?,
            [minutes, seconds] => {
                parse_number(minutes, value)? * 60.0 + parse_number(seconds, value)?
            }
            [hours, minutes, seconds] => {
                parse_number(hours, value)? * 3_600.0
                    + parse_number(minutes, value)? * 60.0
                    + parse_number(seconds, value)?
            }
            _ => return Err(TtmlError(format!("invalid TTML time: {value}"))),
        }
    };

    if !seconds.is_finite() || seconds < 0.0 {
        return Err(TtmlError(format!("invalid TTML time: {value}")));
    }

    Ok((seconds * 1_000.0).round() as u64)
}

fn parse_number(value: &str, original: &str) -> Result<f64, TtmlError> {
    value
        .parse()
        .map_err(|_| TtmlError(format!("invalid TTML time: {original}")))
}

#[cfg(test)]
mod tests {
    use super::{
        BiniLyricsResult, KpoeLine, KpoeMetadata, KpoeResponse, KpoeSyllable, LrcLibResponse,
        LrcLibSearchResult, Lyrics, LyricsClient, LyricsTiming, LyricsTrack, SearchTerms,
        UnisonData, UnisonResponse, allowed_provider_url, best_bini_match, best_lrclib_match,
        clean_artist, clean_title, match_score, parse_kpoe, parse_lrclib, parse_unison, tokens,
        unison_sync_timing,
    };

    fn track(title: &str, artist: &str, duration: Option<u64>) -> LyricsTrack {
        LyricsTrack {
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: None,
            duration_seconds: duration,
            video_id: None,
        }
    }

    fn lrclib_hit(track_name: &str, artist_name: &str, duration: f64) -> LrcLibSearchResult {
        LrcLibSearchResult {
            track_name: track_name.to_owned(),
            artist_name: artist_name.to_owned(),
            duration,
            synced_lyrics: Some("[00:01.00]First".to_owned()),
            plain_lyrics: None,
        }
    }

    #[test]
    fn parses_syllable_synced_ttml() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
                <head><metadata><songwriters><songwriter>J. Tajor</songwriter><songwriter>J. Tajor</songwriter></songwriters></metadata></head>
                <body>
                    <div>
                        <p begin="00:01.000" end="00:04.000">
                            <span begin="00:01.000" end="00:01.500">Hel</span><span begin="00:01.500" end="00:02.000">lo </span><span begin="00:02.000" end="00:03.000">world</span>
                            <span ttm:role="x-bg" begin="00:02.000" end="00:03.000">ignored</span>
                        </p>
                    </div>
                </body>
            </tt>"#,
        )
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "Hello world");
        assert_eq!(lyrics.lines[0].start_ms, Some(1_000));
        assert_eq!(lyrics.lines[0].end_ms, Some(4_000));
        assert_eq!(lyrics.lines[0].syllables.len(), 3);
        assert_eq!(lyrics.lines[0].syllables[0].text, "Hel");
        assert_eq!(lyrics.lines[0].syllables[0].start_ms, 1_000);
        assert_eq!(lyrics.lines[0].syllables[1].text, "lo");
        assert_eq!(lyrics.lines[0].syllables[1].tail, " ");
        assert_eq!(lyrics.songwriters, ["J. Tajor"]);
    }

    #[test]
    fn falls_back_to_line_synced_ttml() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml">
                <body><div>
                    <p begin="1.25s" end="3.5s">First line</p>
                    <p begin="00:04.000" end="00:06.000"><span>Second</span> line</p>
                </div></body>
            </tt>"#,
        )
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "First line");
        assert_eq!(lyrics.lines[0].start_ms, Some(1_250));
        assert_eq!(lyrics.lines[0].end_ms, Some(3_500));
        assert_eq!(lyrics.lines[1].text, "Second line");
        assert!(lyrics.lines[0].syllables.is_empty());
    }

    #[test]
    fn falls_back_to_plain_ttml() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml">
                <body><div>
                    <p>First line</p>
                    <p>Second <span>line</span></p>
                </div></body>
            </tt>"#,
        )
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::Plain);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "First line");
        assert_eq!(lyrics.lines[1].text, "Second line");
        assert_eq!(lyrics.lines[0].start_ms, None);
        assert_eq!(lyrics.lines[0].end_ms, None);
    }

    #[test]
    fn selects_syllable_then_line_then_plain() {
        let plain = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p>Plain</p></body></tt>"#,
        )
        .unwrap();
        let line = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="1s" end="2s">Line</p></body></tt>"#,
        )
        .unwrap();
        let syllable = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p end="2s"><span begin="1s" end="2s">Syllable</span></p></body></tt>"#,
        )
        .unwrap();

        assert_eq!(
            Lyrics::best_available([plain.clone(), line.clone(), syllable.clone()]),
            Some(syllable)
        );
        assert_eq!(
            Lyrics::best_available([plain.clone(), line.clone()]),
            Some(line)
        );
        assert_eq!(Lyrics::best_available([plain.clone()]), Some(plain));
        assert_eq!(Lyrics::best_available([]), None);
    }

    #[test]
    fn preserves_punctuation_between_timed_spans() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p end="3s"><span begin="1s" end="2s">wait</span>—<span begin="2s" end="3s">what</span></p></body></tt>"#,
        )
        .unwrap();

        assert_eq!(lyrics.lines[0].text, "wait—what");
        assert_eq!(lyrics.lines[0].syllables[0].tail, "—");
    }

    #[test]
    fn accepts_duration_based_line_and_syllable_timing() {
        let line = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="1s" dur="2s">Line</p></body></tt>"#,
        )
        .unwrap();
        let syllable = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="1s" dur="3s"><span begin="1s" dur="500ms">Hi</span></p></body></tt>"#,
        )
        .unwrap();

        assert_eq!(line.lines[0].end_ms, Some(3_000));
        assert_eq!(syllable.lines[0].syllables[0].end_ms, 1_500);
    }

    #[test]
    fn rejects_reversed_line_and_non_monotonic_syllable_timing() {
        let reversed_line = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="3s" end="2s">Line</p></body></tt>"#,
        );
        let non_monotonic_syllables = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p end="4s"><span begin="2s" end="3s">First</span><span begin="1s" end="2s">Second</span></p></body></tt>"#,
        );

        assert!(reversed_line.is_err());
        assert!(non_monotonic_syllables.is_err());
    }

    #[test]
    fn ranks_a_document_by_its_weakest_line() {
        let mixed = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body>
                <p end="2s"><span begin="1s" end="2s">Synced</span></p>
                <p>Plain</p>
            </body></tt>"#,
        )
        .unwrap();

        assert_eq!(mixed.timing, LyricsTiming::Plain);
    }

    #[test]
    fn normalizes_inter_span_whitespace_and_ignores_translations() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body>
                <p end="3s"><span begin="1s" end="2s">hello</span>
                    <span begin="2s" end="3s">world</span><span ttm:role="x-translation" begin="2s" end="3s">hola</span></p>
            </body></tt>"#,
        )
        .unwrap();

        assert_eq!(lyrics.lines[0].text, "hello world");
        assert_eq!(lyrics.lines[0].syllables.len(), 2);
    }

    #[test]
    fn rejects_syllables_outside_their_line() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="2s" end="3s"><span begin="1s" end="4s">Outside</span></p></body></tt>"#,
        );

        assert!(lyrics.is_err());
    }

    #[test]
    fn parses_lrclib_line_timing_before_plain_fallback() {
        let lyrics = parse_lrclib(LrcLibResponse {
            synced_lyrics: Some("[00:01.25]First\n[00:03.500]Second".to_owned()),
            plain_lyrics: Some("Ignored plain text".to_owned()),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.source, "LRCLIB");
        assert_eq!(lyrics.lines[0].start_ms, Some(1_250));
        assert_eq!(lyrics.lines[0].end_ms, Some(3_500));
        assert_eq!(lyrics.lines[1].end_ms, Some(8_500));
    }

    #[test]
    fn parses_lrclib_plain_fallback() {
        let lyrics = parse_lrclib(LrcLibResponse {
            synced_lyrics: None,
            plain_lyrics: Some("First\n\nSecond".to_owned()),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::Plain);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text, "Second");
    }

    #[test]
    fn parses_unison_ttml_as_syllable_synced() {
        let lyrics = parse_unison(UnisonResponse {
            success: true,
            data: Some(UnisonData {
                format: "ttml".to_owned(),
                sync_type: "richsync".to_owned(),
                lyrics: r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><p begin="1s" end="3s"><span begin="1s" end="2s">Hello</span> <span begin="2s" end="3s">world</span></p></body></tt>"#
                    .to_owned(),
            }),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "Unison");
        assert_eq!(lyrics.lines[0].text, "Hello world");
        assert_eq!(lyrics.lines[0].syllables.len(), 2);
    }

    #[test]
    fn parses_unison_lrc_as_line_synced() {
        let lyrics = parse_unison(UnisonResponse {
            success: true,
            data: Some(UnisonData {
                format: "lrc".to_owned(),
                sync_type: "linesync".to_owned(),
                lyrics: "[00:01.25]First\n[00:03.500]Second".to_owned(),
            }),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.source, "Unison");
        assert_eq!(lyrics.lines[0].start_ms, Some(1_250));
        assert_eq!(lyrics.lines[1].end_ms, Some(8_500));
    }

    #[test]
    fn parses_unison_plain_text_fallback() {
        let lyrics = parse_unison(UnisonResponse {
            success: true,
            data: Some(UnisonData {
                format: "text".to_owned(),
                sync_type: "plain".to_owned(),
                lyrics: "First line\n\nSecond line".to_owned(),
            }),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::Plain);
        assert_eq!(lyrics.source, "Unison");
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text, "Second line");
    }

    #[test]
    fn rejects_failed_unison_responses() {
        assert!(
            parse_unison(UnisonResponse {
                success: false,
                data: None,
            })
            .is_none()
        );
        assert!(
            parse_unison(UnisonResponse {
                success: true,
                data: None,
            })
            .is_none()
        );
        assert!(
            parse_unison(UnisonResponse {
                success: true,
                data: Some(UnisonData {
                    format: "ttml".to_owned(),
                    sync_type: "richsync".to_owned(),
                    lyrics: "<not-ttml>".to_owned(),
                }),
            })
            .is_none()
        );
    }

    #[test]
    fn maps_unison_sync_types_to_timing() {
        assert_eq!(
            unison_sync_timing("richsync"),
            Some(LyricsTiming::SyllableSynced)
        );
        assert_eq!(
            unison_sync_timing("linesync"),
            Some(LyricsTiming::LineSynced)
        );
        assert_eq!(unison_sync_timing("plain"), Some(LyricsTiming::Plain));
        assert_eq!(unison_sync_timing("unknown"), None);
    }

    #[test]
    fn honors_unison_sync_type_for_untagged_lrc_payloads() {
        let lyrics = parse_unison(UnisonResponse {
            success: true,
            data: Some(UnisonData {
                format: "text".to_owned(),
                sync_type: "linesync".to_owned(),
                lyrics: "[00:01.25]First\n[00:03.500]Second".to_owned(),
            }),
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.source, "Unison");
        assert_eq!(lyrics.lines[0].start_ms, Some(1_250));
    }

    #[test]
    fn allows_unison_host_in_provider_validation() {
        assert!(allowed_provider_url(
            &reqwest::Url::parse("https://unison.boidu.dev/lyrics?v=abc123").unwrap()
        ));
        assert!(!allowed_provider_url(
            &reqwest::Url::parse("https://unison.boidu.dev.attacker.example/lyrics").unwrap()
        ));
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_synced_lyrics_for_a_noisy_video_title() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch(LyricsTrack {
                title: "GIMS - Est-ce que tu m'aimes ? (Clip officiel)".to_owned(),
                artist: "GIMS".to_owned(),
                album: Some("Est-ce que tu m'aimes ?".to_owned()),
                duration_seconds: Some(242),
                video_id: None,
            })
            .await
            .unwrap()
            .expect("this track exists on LRCLIB and Lyrics+");

        assert!(
            lyrics.timing > LyricsTiming::Plain,
            "expected timed lyrics, got {:?} from {}",
            lyrics.timing,
            lyrics.source
        );
        assert!(!lyrics.lines.is_empty());
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_syllable_synced_lyrics_from_live_kpoe() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_kpoe(&LyricsTrack {
                title: "Marilag".to_owned(),
                artist: "Dionela".to_owned(),
                album: Some("Marilag - Single".to_owned()),
                duration_seconds: Some(158),
                video_id: None,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert!(lyrics.source.starts_with("Lyrics+"));
        assert!(!lyrics.lines.is_empty());
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_synced_lyrics_for_a_placeholder_artist() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch(LyricsTrack {
                title: "Est-ce que tu m'aimes ?".to_owned(),
                artist: "Unknown artist".to_owned(),
                album: None,
                duration_seconds: Some(212),
                video_id: None,
            })
            .await;

        assert!(
            matches!(&lyrics, Ok(Some(lyrics)) if lyrics.timing > LyricsTiming::Plain),
            "a Topic channel track with no artist metadata should still resolve: {lyrics:?}"
        );
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_syllable_synced_lyrics_from_live_unison() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_unison(&LyricsTrack {
                title: "Tahanan".to_owned(),
                artist: "El Manu".to_owned(),
                album: Some("Tahanan".to_owned()),
                duration_seconds: Some(196),
                video_id: Some("XoBuaGgV80Y".to_owned()),
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "Unison");
        assert!(!lyrics.lines.is_empty());
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_syllable_synced_lyrics_from_live_binilyrics() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_binilyrics(&LyricsTrack {
                title: "Marilag".to_owned(),
                artist: "Dionela".to_owned(),
                album: Some("Marilag - Single".to_owned()),
                duration_seconds: Some(158),
                video_id: None,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "Apple (via BiniLyrics)");
        assert_eq!(lyrics.songwriters, ["Dionela"]);
        assert!(!lyrics.lines.is_empty());
    }

    #[test]
    fn strips_upload_noise_from_titles() {
        assert_eq!(
            clean_title("GIMS - Est-ce que tu m'aimes ? (Clip officiel)", "GIMS"),
            "Est-ce que tu m'aimes ?"
        );
        assert_eq!(
            clean_title("Marilag (Official Music Video) [4K]", "Dionela"),
            "Marilag"
        );
        assert_eq!(clean_title("Song [Official Lyric Video]", "X"), "Song");
        assert_eq!(clean_title("Song (feat. Someone)", "X"), "Song");
        assert_eq!(clean_title("Song - Live", "X"), "Song");
        assert_eq!(clean_title("Song (M/V)", "X"), "Song");
    }

    #[test]
    fn keeps_meaningful_title_detail() {
        assert_eq!(
            clean_title("Song (Acoustic Version)", "X"),
            "Song (Acoustic Version)"
        );
        assert_eq!(clean_title("Song (Sped Up)", "X"), "Song");
        assert_eq!(clean_title("Plain Title", "X"), "Plain Title");
    }

    #[test]
    fn keeps_the_original_title_when_stripping_would_empty_it() {
        assert_eq!(clean_title("(Official Video)", "X"), "(Official Video)");
    }

    #[test]
    fn leaves_artist_prefix_alone_when_it_is_not_the_artist() {
        assert_eq!(
            clean_title("Billie Eilish - Bad Guy", "Someone Else"),
            "Billie Eilish - Bad Guy"
        );
    }

    #[test]
    fn strips_channel_decoration_from_artists() {
        assert_eq!(clean_artist("GIMS - Topic"), "GIMS");
        assert_eq!(clean_artist("GIMS - topic"), "GIMS");
        assert_eq!(clean_artist("GIMSVEVO"), "GIMS");
        assert_eq!(clean_artist("Ed Sheeran"), "Ed Sheeran");
        assert_eq!(clean_artist("ArianaGrandeVEVO"), "ArianaGrande");
        assert_eq!(clean_artist("Topic"), "");
    }

    #[test]
    fn preserves_artist_casing_because_it_is_still_queried() {
        assert_eq!(clean_artist("GIMS"), "GIMS");
        assert_eq!(clean_artist("Billie Eilish"), "Billie Eilish");
        assert_eq!(clean_artist("GIMS - Topic"), "GIMS");
    }

    #[test]
    fn tokenizes_for_noise_detection_without_swallowing_real_words() {
        assert_eq!(tokens("feat. Jane"), ["feat", "jane"]);
        assert_eq!(tokens("M/V"), ["m", "v"]);
        assert_eq!(tokens("4K"), ["4k"]);
    }

    #[test]
    fn ranks_a_matching_title_above_an_exact_artist_on_another_song() {
        let terms = SearchTerms::from_track(&track("Tahanan", "Adie", Some(196)));
        assert!(
            match_score(&terms, "Tahanan", "Adie feat. Someone", Some(196.0))
                > match_score(&terms, "Tahanan Live", "Adie", Some(196.0))
        );
    }

    #[test]
    fn ranks_a_matching_title_above_a_mere_containment() {
        let terms = SearchTerms::from_track(&track("Tahanan", "Adie", Some(196)));
        let exact = best_lrclib_match(
            &terms,
            vec![
                lrclib_hit("Tahanan", "Adie feat. Someone", 196.0),
                lrclib_hit("Tahanan Live", "Adie", 196.0),
            ],
        )
        .unwrap();

        assert_eq!(exact.track_name, "Tahanan");
    }

    #[test]
    fn rejects_lrclib_hits_for_other_songs_and_versions() {
        let terms = SearchTerms::from_track(&track("Tahanan", "Adie", Some(196)));

        assert!(
            best_lrclib_match(&terms, vec![lrclib_hit("Different Song", "Adie", 196.0)]).is_none()
        );
        assert!(
            best_lrclib_match(&terms, vec![lrclib_hit("Tahanan", "Other Artist", 196.0)]).is_none()
        );
        assert!(
            best_lrclib_match(&terms, vec![lrclib_hit("Tahanan", "Adie", 294.0)]).is_none(),
            "a live cut at a different duration is a different song"
        );
    }

    #[test]
    fn ignores_the_artist_when_the_track_does_not_name_one() {
        let terms = SearchTerms::from_track(&track("Tahanan", "Unknown artist", Some(196)));
        assert!(!terms.artist_known);
        assert!(
            best_lrclib_match(&terms, vec![lrclib_hit("Tahanan", "Adie", 196.0)]).is_some(),
            "a placeholder artist must not veto an otherwise exact title match"
        );
    }

    fn kpoe_syllable(time: u64, duration: u64, text: &str) -> KpoeSyllable {
        KpoeSyllable {
            time: Some(time),
            duration: Some(duration),
            text: text.to_owned(),
        }
    }

    #[test]
    fn parses_word_synced_kpoe_payloads() {
        let lyrics = parse_kpoe(KpoeResponse {
            ttml: None,
            timing: "Word".to_owned(),
            metadata: Some(KpoeMetadata {
                source: "Apple".to_owned(),
                song_writers: vec!["GIMS".to_owned()],
            }),
            lyrics: vec![KpoeLine {
                time: Some(907),
                duration: Some(2991),
                text: "Hotshot, running nonstop".to_owned(),
                syllabus: vec![
                    kpoe_syllable(907, 782, "Hotshot, "),
                    kpoe_syllable(1689, 404, "running "),
                    kpoe_syllable(2093, 155, "nonstop"),
                ],
            }],
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "Lyrics+ (Apple)");
        assert_eq!(lyrics.songwriters, ["GIMS"]);
        assert_eq!(lyrics.lines[0].text, "Hotshot, running nonstop");
        assert_eq!(lyrics.lines[0].start_ms, Some(907));
        assert_eq!(lyrics.lines[0].end_ms, Some(3_898));
        assert_eq!(lyrics.lines[0].syllables.len(), 3);
        assert_eq!(lyrics.lines[0].syllables[0].text, "Hotshot,");
        assert_eq!(lyrics.lines[0].syllables[0].tail, " ");
        assert_eq!(lyrics.lines[0].syllables[2].tail, "");
    }

    #[test]
    fn parses_line_only_kpoe_payloads() {
        let lyrics = parse_kpoe(KpoeResponse {
            ttml: None,
            timing: "Line".to_owned(),
            metadata: Some(KpoeMetadata {
                source: "QQ Music".to_owned(),
                song_writers: Vec::new(),
            }),
            lyrics: vec![
                KpoeLine {
                    time: Some(1_000),
                    duration: Some(2_000),
                    text: "First".to_owned(),
                    syllabus: Vec::new(),
                },
                KpoeLine {
                    time: Some(3_000),
                    duration: Some(0),
                    text: "Second".to_owned(),
                    syllabus: Vec::new(),
                },
            ],
        })
        .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.source, "Lyrics+ (QQ Music)");
        assert_eq!(lyrics.lines[0].end_ms, Some(3_000));
        assert_eq!(
            lyrics.lines[1].end_ms,
            Some(8_000),
            "a zero-length final line should still get a highlight window"
        );
    }

    #[test]
    fn rejects_empty_kpoe_payloads() {
        assert!(
            parse_kpoe(KpoeResponse {
                ttml: None,
                timing: "Line".to_owned(),
                metadata: None,
                lyrics: Vec::new(),
            })
            .is_none()
        );
    }

    #[test]
    fn allows_kpoe_mirrors_and_rejects_spoofed_hosts() {
        for mirror in super::KPOE_MIRRORS {
            assert!(
                allowed_provider_url(
                    &reqwest::Url::parse(&format!("{mirror}/v1/ttml/get")).unwrap()
                ),
                "{mirror} should be reachable"
            );
            assert!(
                !allowed_provider_url(
                    &reqwest::Url::parse(&format!("{mirror}.attacker.example/v1/ttml/get"))
                        .unwrap()
                ),
                "{mirror} should not authorize a lookalike host"
            );
        }
    }

    #[test]
    fn rejects_unrelated_bini_matches_and_spoofed_storage_hosts() {
        let track = LyricsTrack {
            title: "Tahanan".to_owned(),
            artist: "Adie".to_owned(),
            album: None,
            duration_seconds: Some(294),
            video_id: None,
        };
        let unrelated = BiniLyricsResult {
            track_name: "Different Song".to_owned(),
            artist_name: "Different Artist".to_owned(),
            duration: 294.0,
            lyrics_url: "https://lyrics-storage.binimum.org/different.ttml".to_owned(),
        };

        assert!(best_bini_match(&track, vec![unrelated]).is_none());
        assert!(!allowed_provider_url(
            &reqwest::Url::parse("https://lyrics-storage.binimum.org.attacker.example/a.ttml")
                .unwrap()
        ));
        assert!(allowed_provider_url(
            &reqwest::Url::parse("https://lrc.red/s/PHUM72400160.ttml").unwrap()
        ));
        assert!(!allowed_provider_url(
            &reqwest::Url::parse("https://lrc.red.attacker.example/s/PHUM72400160.ttml").unwrap()
        ));
    }
}
