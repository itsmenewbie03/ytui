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
const LUNA_SEARCH_URL: &str = "https://api.qishui.com/luna/search/track";
const LUNA_DETAIL_URL: &str = "https://beta-luna.douyin.com/luna/h5/seo_track";
const LUNA_SEARCH_UA: &str = "com.luna.music/100198030 (Linux; U; Android 15; zh_CN_#Hans; ABR-AL80; Build/V417IR;tt-ok/3.12.13.19)";
const LUNA_WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const LUNA_MAX_CANDIDATES: usize = 3;
const SIMPMUSIC_URL: &str = "https://api-lyrics.simpmusic.org/v1";
const SIMPMUSIC_UA: &str = "SimpMusicLyrics/1.0";
const SIMPMUSIC_LAST_WORD_MS: u64 = 800;
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

/// Distinct recordings that share a title. A candidate carrying one of these is
/// only a match if the target asks for it too, otherwise `Nothin' on You` would
/// accept the karaoke track and the remix as readily as the song.
const VERSION_MARKERS: &[&str] = &[
    "remix",
    "live",
    "acoustic",
    "instrumental",
    "acappella",
    "acapella",
    "cappella",
    "karaoke",
    "cover",
    "demo",
    "parody",
    "extended",
    "sped",
    "slowed",
    "nightcore",
    "reverb",
    "orchestral",
    "8d",
];

/// QQ Music prepends a credit block (writers, publishers, engineers) to the
/// timed lyrics. These prefixes identify such a line even when it is not a
/// slash-separated name list.
const CREDIT_LABELS: &[&str] = &[
    "lyrics by",
    "lyric by",
    "written by",
    "words by",
    "composed by",
    "music by",
    "produced by",
    "arranged by",
    "vocals by",
    "vocals produced by",
    "vocal produced by",
    "background vocals",
    "additional vocal",
    "executive producer",
    "mixing engineer",
    "mastered by",
    "recorded at",
    "published by",
    "repertoire owner",
    "copyright",
    "publisher",
    "producer",
    "composer",
    "lyricist",
    "作词",
    "作曲",
    "编曲",
    "制作",
    "监制",
    "出品",
    "发行",
    "录音",
    "混音",
    "母带",
];

/// Studio and rights-holder wording that only appears in the credit block.
/// Singular `studio` is deliberately absent: it reads like a lyric.
const CREDIT_INDUSTRY_MARKERS: &[&str] = &[
    "studios",
    "mastering",
    "recordings",
    "publishing",
    "copyright control",
    "chappel",
    "ltd",
    "llc",
    "inc.",
];

/// Words that make a short line read like a lyric rather than a person's name,
/// so `Hold On` and `Dancing Queen` are not mistaken for credits.
const NAME_STOPWORDS: &[&str] = &[
    "a", "am", "an", "and", "any", "are", "as", "at", "baby", "be", "been", "both", "but", "by",
    "can", "come", "could", "day", "did", "do", "does", "down", "each", "few", "for", "from",
    "get", "give", "go", "got", "had", "has", "have", "he", "heart", "her", "hers", "hey", "him",
    "his", "hold", "how", "i", "if", "in", "into", "is", "it", "its", "just", "know", "let",
    "life", "love", "made", "make", "may", "me", "might", "mine", "more", "most", "must", "my",
    "need", "night", "no", "not", "now", "of", "off", "oh", "on", "one", "only", "or", "other",
    "our", "out", "over", "own", "said", "same", "say", "see", "seen", "shall", "she", "should",
    "so", "some", "stay", "such", "take", "than", "that", "the", "their", "them", "then", "these",
    "they", "this", "those", "three", "time", "to", "today", "tonight", "too", "two", "under",
    "up", "us", "very", "wake", "want", "was", "we", "were", "what", "when", "where", "which",
    "who", "why", "will", "with", "without", "world", "would", "yeah", "you", "your", "yours",
];

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
        let (bini, kpoe, unison, simpmusic, lrclib, luna) = tokio::join!(
            self.fetch_binilyrics(&track),
            self.fetch_kpoe(&track),
            self.fetch_unison(&track),
            self.fetch_simpmusic(&track),
            self.fetch_lrclib(&track),
            async {
                // Luna is an undocumented mobile API and is allowed to be slow.
                // The overall timeout keeps a bad day there from delaying the
                // aggregate lookup indefinitely.
                match tokio::time::timeout(Duration::from_secs(5), self.fetch_luna(&track)).await {
                    Ok(result) => result,
                    Err(_) => Ok(None),
                }
            }
        );
        let results = [bini, kpoe, unison, simpmusic, lrclib, luna];
        let candidates = results
            .iter()
            // Order is the tie-break: on equal timing, the first provider that
            // returned lyrics wins, so Apple's label timing is preferred over
            // the aggregated mirrors.
            .filter_map(|result| {
                result
                    .as_ref()
                    .ok()
                    .and_then(|lyrics| lyrics.as_ref())
                    .cloned()
            })
            .collect::<Vec<_>>();

        if let Some(lyrics) = Lyrics::best_available(candidates) {
            return Ok(Some(lyrics));
        }

        let errors = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect::<Vec<_>>();
        if errors.len() == results.len() {
            return Err(LyricsFetchError(format!(
                "lyrics providers failed: {}",
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        if let [error] = errors.as_slice() {
            return Err(LyricsFetchError(error.to_string()));
        }
        Ok(None)
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
        // Deliberately no album: passing one makes the server pick a different
        // source, which for some tracks returns a variant that reads the
        // songwriter credits out as lyrics. Title and artist are enough for it
        // to find the song on its own.
        let mut query = vec![
            ("title", terms.title.as_str()),
            ("artist", terms.artist.as_str()),
        ];
        if !duration.is_empty() {
            query.push(("duration", duration.as_str()));
        }

        let mut last_error = None;
        for (index, mirror) in KPOE_MIRRORS.iter().enumerate() {
            // The JSON endpoint is the maintained one and is the only endpoint
            // that reports which source it settled on. The TTML endpoint can
            // serve a different, lower-quality variant of the same track, which
            // renders songwriter credits as timed lyric lines, so it is only a
            // fallback.
            match self
                .kpoe_request(mirror, "/v2/lyrics/get", &query, &terms)
                .await
            {
                Ok(Some(lyrics)) => return Ok(Some(lyrics)),
                Ok(None) => {}
                Err(error) => last_error = Some(error),
            }

            if index == 0
                && let Ok(Some(lyrics)) = self
                    .kpoe_request(mirror, "/v1/ttml/get", &query, &terms)
                    .await
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
        terms: &SearchTerms,
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

        Ok(parse_kpoe(response, terms))
    }

    /// SimpMusic stores community-submitted lyrics keyed by the exact YouTube
    /// video ID. Unlike the search-based providers it has no title fallback, so
    /// a missing video ID means there is nothing to ask for.
    async fn fetch_simpmusic(
        &self,
        track: &LyricsTrack,
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let Some(video_id) = track.video_id.as_deref().map(str::trim) else {
            return Ok(None);
        };
        if !valid_youtube_video_id(video_id) {
            return Ok(None);
        }

        let url = format!("{SIMPMUSIC_URL}/{video_id}");
        let response = self
            .client
            .get(&url)
            .header("User-Agent", SIMPMUSIC_UA)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(provider_error("SimpMusic"))?;
        let Some(body) = response_body(response, "SimpMusic").await? else {
            return Ok(None);
        };
        let response: SimpMusicResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid SimpMusic response: {error}")))?;
        if !response.success {
            return Ok(None);
        }

        Ok(parse_simpmusic(response))
    }

    /// Luna (Qishui/Douyin) is a mobile music service whose web endpoint serves
    /// KRC word timing. Its catalog is UGC-heavy, so candidates are matched
    /// strictly before any lyrics detail is requested.
    async fn fetch_luna(&self, track: &LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        let terms = SearchTerms::from_track(track);
        let keyword = format!("{} {}", terms.artist, terms.title);
        let keyword = keyword.trim();
        let response = self
            .client
            .get(LUNA_SEARCH_URL)
            .query(&[
                ("device_platform", "android"),
                ("os", "android"),
                ("ssmix", "a"),
                ("cdid", "46556f98-1720-4248-83da-62b74b60b46a"),
                ("channel", "xiaomi_8478_64"),
                ("aid", "386088"),
                ("app_name", "luna"),
                ("version_code", "100198030"),
                ("version_name", "19.8.0"),
                ("manifest_version_code", "100198030"),
                ("update_version_code", "100198030"),
                ("resolution", "1080*1920"),
                ("dpi", "480"),
                ("device_type", "ABR-AL80"),
                ("device_brand", "HUAWEI"),
                ("language", "en"),
                ("os_api", "35"),
                ("os_version", "15"),
                ("ac", "wifi"),
                ("device_model", "ABR-AL80"),
                ("tz_name", "Asia/Shanghai"),
                ("tz_offset", "28800"),
                ("package", "com.luna.music"),
                ("sim_region", "cn"),
                ("q", keyword),
                ("cursor", "0"),
                ("count", "20"),
            ])
            .header("User-Agent", LUNA_SEARCH_UA)
            .header("Accept", "*/*")
            .send()
            .await
            .map_err(provider_error("Luna search"))?;
        let Some(body) = response_body(response, "Luna search").await? else {
            return Ok(None);
        };
        let response: LunaSearchResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid Luna search response: {error}")))?;
        let candidates = luna_candidates(&terms, response);

        let mut last_error = None;
        for (index, candidate) in candidates.iter().take(LUNA_MAX_CANDIDATES).enumerate() {
            if index > 0 {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            match self.fetch_luna_lyrics(candidate, &terms).await {
                Ok(Some(lyrics)) => return Ok(Some(lyrics)),
                Ok(None) => {}
                Err(error) => last_error = Some(error),
            }
        }

        match last_error {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }

    async fn fetch_luna_lyrics(
        &self,
        candidate: &LunaCandidate,
        terms: &SearchTerms,
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let response = self
            .client
            .get(LUNA_DETAIL_URL)
            .query(&[
                ("track_id", candidate.id.as_str()),
                ("device_platform", "web"),
            ])
            .header("User-Agent", LUNA_WEB_UA)
            .header("Accept", "*/*")
            .send()
            .await
            .map_err(provider_error("Luna lyrics"))?;
        let Some(body) = response_body(response, "Luna lyrics").await? else {
            return Ok(None);
        };
        let response: LunaDetailResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid Luna lyrics response: {error}")))?;
        Ok(parse_luna(response, terms))
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

#[derive(Deserialize)]
struct LunaSearchResponse {
    #[serde(default)]
    result_groups: Vec<LunaResultGroup>,
}

#[derive(Deserialize)]
struct LunaResultGroup {
    #[serde(default)]
    data: Vec<LunaSearchItem>,
}

#[derive(Deserialize)]
struct LunaSearchItem {
    #[serde(default)]
    entity: Option<LunaEntity>,
}

#[derive(Deserialize)]
struct LunaEntity {
    #[serde(default)]
    track: Option<LunaTrack>,
}

#[derive(Deserialize)]
struct LunaDetailResponse {
    #[serde(default)]
    lyric: Option<LunaLyric>,
    #[serde(default)]
    seo_track: Option<LunaSeoTrack>,
}

#[derive(Deserialize)]
struct LunaSeoTrack {
    #[serde(default)]
    track: Option<LunaTrack>,
}

#[derive(Deserialize)]
struct LunaLyric {
    #[serde(default)]
    content: String,
    #[serde(default, rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct LunaTrack {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    artists: Vec<LunaArtist>,
    #[serde(default)]
    duration: u64,
    #[serde(default)]
    label_info: Option<LunaLabelInfo>,
    #[serde(default)]
    song_maker_team: Option<LunaSongMakerTeam>,
}

#[derive(Deserialize)]
struct LunaArtist {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct LunaLabelInfo {
    #[serde(default)]
    is_original: bool,
}

#[derive(Deserialize)]
struct LunaSongMakerTeam {
    #[serde(default)]
    composers: Vec<LunaCredit>,
    #[serde(default)]
    lyricists: Vec<LunaCredit>,
}

#[derive(Deserialize)]
struct LunaCredit {
    #[serde(default)]
    name: String,
}

struct LunaCandidate {
    id: String,
}

#[derive(Deserialize)]
struct SimpMusicResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<Vec<SimpMusicEntry>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimpMusicEntry {
    #[serde(default)]
    vote: Option<i64>,
    #[serde(default)]
    contributor: Option<String>,
    #[serde(default)]
    rich_sync_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
    #[serde(default)]
    plain_lyric: Option<String>,
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
                    | "api.qishui.com"
                    | "beta-luna.douyin.com"
                    | "api-lyrics.simpmusic.org"
            )
        )
}

/// Ranks Luna search hits before any detail request. Luna returns a lot of
/// covers and karaoke uploads, so only candidates that pass the shared match
/// rules are kept; originals win ties.
fn luna_candidates(terms: &SearchTerms, response: LunaSearchResponse) -> Vec<LunaCandidate> {
    let mut candidates = Vec::new();
    for group in response.result_groups {
        for item in group.data {
            let Some(track) = item.entity.and_then(|entity| entity.track) else {
                continue;
            };
            if track.id.is_empty() || track.name.is_empty() {
                continue;
            }
            if candidates
                .iter()
                .any(|(_, _, candidate): &(u32, bool, LunaCandidate)| candidate.id == track.id)
            {
                continue;
            }
            let artist = track
                .artists
                .iter()
                .map(|artist| artist.name.trim())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let duration = (track.duration > 0).then_some(track.duration as f64 / 1_000.0);
            let Some(score) = match_score(terms, &track.name, &artist, duration) else {
                continue;
            };
            let original = track
                .label_info
                .as_ref()
                .is_some_and(|label| label.is_original);
            candidates.push((score, original, LunaCandidate { id: track.id }));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    candidates
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect()
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

fn version_markers(title: &str) -> Vec<&'static str> {
    let words = tokens(title);
    VERSION_MARKERS
        .iter()
        .copied()
        .filter(|marker| words.iter().any(|word| word == marker))
        .collect()
}

/// Scores a provider result against the search terms, or rejects it outright.
/// Shared by every provider so candidates rank identically.
fn match_score(
    terms: &SearchTerms,
    candidate_title: &str,
    artist: &str,
    duration: Option<f64>,
) -> Option<u32> {
    let target_title = normalized_match_text(&terms.title);
    let title = normalized_match_text(candidate_title);
    let target_artist = normalized_match_text(&terms.artist);
    let artist = normalized_match_text(artist);

    let target_markers = version_markers(&terms.title);
    if version_markers(candidate_title)
        .into_iter()
        .any(|marker| !target_markers.contains(&marker))
    {
        return None;
    }

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

    // A music video is routinely a trimmed edit of the album track, so a loose
    // duration still points at the right song. It only ranks, it never vetoes.
    let duration_score = match (terms.duration, duration) {
        (Some(target), Some(candidate)) if candidate > 0.0 => {
            let difference = (candidate - target).abs();
            if difference <= 3.0 {
                5
            } else if difference <= 8.0 {
                3
            } else if difference <= f64::max(45.0, target * 0.2) {
                1
            } else {
                return None;
            }
        }
        _ => 0,
    };

    // Title outweighs artist: a matching title with a `feat.` suffix on the
    // artist is still the same song, but a mismatched title is a different one.
    let title_score = if target_title.is_empty() || title.is_empty() {
        0
    } else if target_title == title {
        20
    } else if target_title.contains(&title) || title.contains(&target_title) {
        10
    } else if artist_score >= 10
        && duration_score >= 1
        && titles_differ_by_one_edit(&target_title, &title)
    {
        // Providers spell elided titles differently (`Nothin'` vs `Nothing`).
        // Requiring a known artist and a plausible duration keeps this from
        // turning into a loose match.
        8
    } else {
        return None;
    };

    Some(title_score + artist_score + duration_score)
}

/// Whether two normalized titles differ by at most one inserted, deleted, or
/// substituted character. Short titles are excluded so `Sing` and `Sin` are
/// never treated as the same song.
fn titles_differ_by_one_edit(left: &str, right: &str) -> bool {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().min(right.len()) < 5 || left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    if left.first() != right.first() {
        return false;
    }

    let (shorter, longer) = if left.len() <= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let mut short_index = 0;
    let mut long_index = 0;
    let mut edits = 0;
    while short_index < shorter.len() && long_index < longer.len() {
        if shorter[short_index] == longer[long_index] {
            short_index += 1;
            long_index += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if shorter.len() == longer.len() {
                short_index += 1;
                long_index += 1;
            } else {
                long_index += 1;
            }
        }
    }
    edits + (shorter.len() - short_index) + (longer.len() - long_index) <= 1
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

/// Drops the metadata block QQ Music puts in front of the timed lyrics. It is a
/// contiguous run at the start of the payload; the first real lyric ends it, so
/// a credit-looking line later in the song is never touched.
fn strip_leading_credits(
    lines: &mut Vec<(u64, u64, String, Vec<LyricSyllable>)>,
    terms: &SearchTerms,
) {
    const MAX_CREDIT_LINES: usize = 24;

    let mut dropped = 0;
    while dropped < lines.len() && dropped < MAX_CREDIT_LINES {
        let next_start_ms = lines.get(dropped + 1).map(|line| line.0);
        let (start_ms, end_ms, text, _) = &lines[dropped];
        if !is_credit_line(text, terms, *start_ms, *end_ms, next_start_ms) {
            break;
        }
        dropped += 1;
    }

    // An all-credit payload is still better than no lyrics at all.
    if dropped < lines.len() {
        lines.drain(..dropped);
    }
}

fn is_credit_line(
    text: &str,
    terms: &SearchTerms,
    start_ms: u64,
    end_ms: u64,
    next_start_ms: Option<u64>,
) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return true;
    }
    let lowered = text.to_lowercase();
    if CREDIT_LABELS.iter().any(|label| lowered.starts_with(label)) {
        return true;
    }
    if text.contains(':') || text.contains('：') {
        return true;
    }
    if is_slash_name_list(text) || is_title_artist_header(text, terms) {
        return true;
    }
    if CREDIT_INDUSTRY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        return true;
    }

    if looks_like_person_name(text)
        && normalized_match_text(text) != normalized_match_text(&terms.title)
    {
        let duration_ms = end_ms.saturating_sub(start_ms);
        let gap_ms = next_start_ms.map(|next| next.saturating_sub(end_ms));
        return duration_ms < 1_000 || gap_ms.is_some_and(|gap| gap > 5_000);
    }

    false
}

fn is_slash_name_list(text: &str) -> bool {
    if !text.contains('/') {
        return false;
    }
    let parts = text.split('/').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    parts.iter().all(|part| {
        let words = part.split_whitespace().collect::<Vec<_>>();
        !words.is_empty() && words.len() <= 5 && words.iter().all(|word| is_name_word(word))
    })
}

fn is_title_artist_header(text: &str, terms: &SearchTerms) -> bool {
    let normalized = normalized_match_text(text);
    let title = normalized_match_text(&terms.title);
    if title.chars().count() < 3 || !normalized.starts_with(&title) {
        return false;
    }
    if ![" - ", " – ", " — ", " | "]
        .iter()
        .any(|separator| text.contains(separator))
    {
        return false;
    }
    let artist = normalized_match_text(&terms.artist);
    (!artist.is_empty() && normalized.contains(&artist)) || text.contains('(') || text.contains('[')
}

fn looks_like_person_name(text: &str) -> bool {
    // Syllable reconstruction can leave punctuation-only tokens behind, as in
    // `Juan " Saucy " Peña`; they are not part of the name.
    let words = text
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .collect::<Vec<_>>();
    if !(2..=4).contains(&words.len()) {
        return false;
    }
    if !words.iter().all(|word| is_name_word(word)) {
        return false;
    }
    if words
        .iter()
        .any(|word| NAME_STOPWORDS.contains(&bare_name_word(word).as_str()))
    {
        return false;
    }
    if !words.iter().any(|word| {
        word.chars()
            .filter(|character| character.is_alphabetic())
            .count()
            >= 2
    }) {
        return false;
    }
    !matches!(text.trim_end().chars().last(), Some('?' | '!' | ';' | ','))
}

fn is_name_word(word: &str) -> bool {
    let core = word.trim_matches(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '.' | ',' | '\'' | '"' | '“' | '”' | '‘' | '’' | '(' | ')' | '-'
            )
    });
    if core.is_empty() {
        return false;
    }
    if core
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    {
        return true;
    }
    let mut characters = core.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_uppercase()
        && characters.all(|character| {
            character.is_alphabetic() || matches!(character, '.' | '\'' | '’' | '-')
        })
}

fn bare_name_word(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_ascii_alphabetic())
        .collect()
}

/// Parses the Lyrics+ v2 payload, which carries timings in milliseconds with
/// `duration` rather than `end`. Syllables keep their own trailing space, which
/// is what glues consecutive words back together.
fn parse_kpoe(response: KpoeResponse, terms: &SearchTerms) -> Option<Lyrics> {
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
    strip_leading_credits(&mut pending, terms);

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
    } else if metadata.source.starts_with("Lyrics+") {
        metadata.source.trim().to_owned()
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

fn parse_luna(response: LunaDetailResponse, terms: &SearchTerms) -> Option<Lyrics> {
    let lyric = response.lyric?;
    let content = lyric.content.trim();
    if content.is_empty() {
        return None;
    }

    let kind = lyric.kind.to_ascii_lowercase();
    let (mut timing, mut lines) = if kind == "krc" {
        if let Some(lines) = parse_krc(content) {
            (LyricsTiming::SyllableSynced, lines)
        } else if let Some(lines) = parse_lrc(content) {
            (LyricsTiming::LineSynced, lines)
        } else {
            (LyricsTiming::Plain, parse_plain_lyrics(content)?)
        }
    } else if let Some(lines) = parse_lrc(content) {
        (LyricsTiming::LineSynced, lines)
    } else {
        (LyricsTiming::Plain, parse_plain_lyrics(content)?)
    };

    if lines.iter().any(|line| !line.syllables.is_empty()) {
        timing = LyricsTiming::SyllableSynced;
    } else if timing == LyricsTiming::SyllableSynced {
        timing = LyricsTiming::LineSynced;
    }

    strip_leading_credits_from_lines(&mut lines, terms);
    if lines.is_empty() {
        return None;
    }

    Some(Lyrics {
        timing,
        lines,
        source: "Luna".to_owned(),
        songwriters: luna_songwriters(response.seo_track),
    })
}

/// Parses SimpMusic's JSON payload. Entries are community submissions, so the
/// highest-voted one wins; within an entry the richest available timing is used.
fn parse_simpmusic(response: SimpMusicResponse) -> Option<Lyrics> {
    let entries = response.data?;
    let mut best: Option<(i64, Lyrics)> = None;
    for entry in entries {
        let vote = entry.vote.unwrap_or(0);
        let Some(lyrics) = parse_simpmusic_entry(entry) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_vote, _)| vote > *best_vote) {
            best = Some((vote, lyrics));
        }
    }
    best.map(|(_, lyrics)| lyrics)
}

fn parse_simpmusic_entry(entry: SimpMusicEntry) -> Option<Lyrics> {
    let contributor = entry
        .contributor
        .as_deref()
        .map(str::trim)
        .filter(|contributor| !contributor.is_empty());
    let source = contributor.map_or_else(
        || "SimpMusic".to_owned(),
        |contributor| format!("SimpMusic (submitted by {contributor})"),
    );

    let mut parsed = None;
    for document in [entry.rich_sync_lyrics, entry.synced_lyrics] {
        let Some(document) = document else {
            continue;
        };
        if document.trim().is_empty() {
            continue;
        }
        if let Some(lines) = parse_simpmusic_lrc(&document) {
            let timing = if lines.iter().any(|line| !line.syllables.is_empty()) {
                LyricsTiming::SyllableSynced
            } else {
                LyricsTiming::LineSynced
            };
            parsed = Some((timing, lines));
            break;
        }
    }

    if parsed.is_none()
        && let Some(document) = entry.plain_lyric
        && let Some(lines) = parse_simpmusic_plain(&document)
    {
        parsed = Some((LyricsTiming::Plain, lines));
    }

    parsed.map(|(timing, lines)| Lyrics {
        timing,
        lines,
        source,
        songwriters: Vec::new(),
    })
}

/// Parses SimpMusic's LRC payload, including its `[offset:seconds]` tag and the
/// `<mm:ss.xx>` word markers used by rich sync. Section-only lines are dropped.
fn parse_simpmusic_lrc(document: &str) -> Option<Vec<LyricLine>> {
    let offset_ms = simpmusic_offset(document);
    let mut lines = Vec::new();

    for raw_line in document.lines() {
        let raw_line = raw_line.trim();
        if raw_line.is_empty() {
            continue;
        }
        let (timestamps, body) = split_lrc_tags(raw_line);
        if timestamps.is_empty() {
            continue;
        }
        let body = body.trim();
        if body.is_empty() {
            continue;
        }

        for timestamp in timestamps {
            let start_ms = adjust_simpmusic_time(timestamp, offset_ms);
            let (text, syllables) = parse_simpmusic_body(body, start_ms, offset_ms);
            if is_break_marker(&text) || is_section_label(&text) {
                continue;
            }
            let start_ms = syllables
                .iter()
                .map(|syllable| syllable.start_ms)
                .min()
                .map_or(start_ms, |first| first.min(start_ms));
            let end_ms = syllables.iter().map(|syllable| syllable.end_ms).max();
            lines.push(LyricLine {
                text,
                start_ms: Some(start_ms),
                end_ms,
                syllables,
            });
        }
    }

    if lines.is_empty() {
        return None;
    }

    lines.sort_by_key(|line| line.start_ms.unwrap_or(0));
    for index in 0..lines.len() {
        let start_ms = lines[index].start_ms.unwrap_or(0);
        let end_ms = lines[index].end_ms.unwrap_or_else(|| {
            lines
                .get(index + 1)
                .and_then(|line| line.start_ms)
                .unwrap_or_else(|| start_ms.saturating_add(5_000))
        });
        lines[index].end_ms = Some(end_ms.max(start_ms));
    }

    Some(lines)
}

fn parse_simpmusic_body(
    body: &str,
    line_start_ms: u64,
    offset_ms: i64,
) -> (String, Vec<LyricSyllable>) {
    let markers = simpmusic_markers(body);
    if markers.is_empty() {
        return (
            normalize_lyric_text(&decode_html_entities(body)),
            Vec::new(),
        );
    }

    let mut raw_words = Vec::new();
    let prefix = &body[..markers[0].0];
    if !prefix.trim().is_empty() {
        raw_words.push((line_start_ms, prefix));
    }
    for (index, &(_, marker_end, timestamp)) in markers.iter().enumerate() {
        let raw_end = markers.get(index + 1).map_or(body.len(), |next| next.0);
        raw_words.push((
            adjust_simpmusic_time(timestamp, offset_ms),
            &body[marker_end..raw_end],
        ));
    }

    let decoded_words = raw_words
        .into_iter()
        .map(|(start_ms, raw)| (start_ms, decode_html_entities(raw)))
        .collect::<Vec<_>>();

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut trailing_marker = None;
    for (index, (start_ms, raw)) in decoded_words.iter().enumerate() {
        let text = normalize_lyric_text(raw);
        if text.is_empty() {
            if let Some(last) = syllables.last_mut()
                && !last.tail.ends_with(' ')
            {
                last.tail.push(' ');
            }
            if index + 1 == decoded_words.len() {
                trailing_marker = Some(*start_ms);
            }
            continue;
        }

        let trailing_space = raw.chars().last().is_some_and(char::is_whitespace);
        let next_leading_space = decoded_words
            .get(index + 1)
            .is_some_and(|(_, next)| next.chars().next().is_some_and(char::is_whitespace));
        let tail = if trailing_space || next_leading_space {
            " "
        } else {
            ""
        };
        syllables.push(LyricSyllable {
            text,
            tail: tail.to_owned(),
            start_ms: *start_ms,
            end_ms: *start_ms,
        });
    }

    for index in 0..syllables.len() {
        let start_ms = syllables[index].start_ms;
        if let Some(next) = syllables.get(index + 1) {
            syllables[index].end_ms = next.start_ms.max(start_ms);
        } else {
            let end_ms = trailing_marker
                .filter(|marker| *marker > start_ms)
                .unwrap_or_else(|| start_ms.saturating_add(SIMPMUSIC_LAST_WORD_MS));
            syllables[index].end_ms = end_ms.max(start_ms);
        }
    }

    let text = syllables
        .iter()
        .map(|syllable| format!("{}{}", syllable.text, syllable.tail))
        .collect::<String>();
    (normalize_lyric_text(&text), syllables)
}

/// Finds `<mm:ss.xx>` and `[mm:ss.xx]` word markers in a rich-sync body. A
/// bracket that is not a timestamp (a section label, for example) is left as
/// literal text, and a nested marker inside such a bracket is still found.
fn simpmusic_markers(body: &str) -> Vec<(usize, usize, u64)> {
    let indices = body.char_indices().collect::<Vec<_>>();
    let mut markers = Vec::new();
    let mut position = 0;

    while position < indices.len() {
        let (start, character) = indices[position];
        let close_character = match character {
            '<' => '>',
            '[' => ']',
            _ => {
                position += 1;
                continue;
            }
        };
        let close = indices[position + 1..]
            .iter()
            .find(|(_, candidate)| *candidate == close_character)
            .map(|(index, _)| *index);
        if let Some(close) = close {
            let tag = &body[start + character.len_utf8()..close];
            if let Some(timestamp) = parse_lrc_timestamp(tag) {
                let next = close + close_character.len_utf8();
                markers.push((start, next, timestamp));
                position = indices.partition_point(|entry| entry.0 < next);
                continue;
            }
        }
        position += 1;
    }

    markers
}

/// Splits the leading `[timestamp]` and `[section]` tags from an LRC line.
/// Metadata tags are ignored, while a bracketed word that is not a known section
/// stays in the body so it can still be sung.
fn split_lrc_tags(line: &str) -> (Vec<u64>, &str) {
    let mut timestamps = Vec::new();
    let mut rest = line.trim_start();

    while let Some(after_open) = rest.strip_prefix('[') {
        let Some(close) = after_open.find(']') else {
            break;
        };
        let tag = &after_open[..close];
        if let Some(timestamp) = parse_lrc_timestamp(tag) {
            timestamps.push(timestamp);
            rest = after_open[close + 1..].trim_start();
            continue;
        }
        if is_section_tag(tag) || is_metadata_tag(tag) {
            rest = after_open[close + 1..].trim_start();
            continue;
        }
        break;
    }

    (timestamps, rest)
}

fn simpmusic_offset(document: &str) -> i64 {
    let mut offset_ms = 0;
    for line in document.lines() {
        let mut rest = line.trim_start();
        while let Some(after_open) = rest.strip_prefix('[') {
            let Some(close) = after_open.find(']') else {
                break;
            };
            if let Some(offset) = parse_offset_tag(&after_open[..close]) {
                offset_ms = offset;
            }
            rest = after_open[close + 1..].trim_start();
        }
    }
    offset_ms
}

fn parse_offset_tag(tag: &str) -> Option<i64> {
    let (key, value) = tag.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case("offset") {
        return None;
    }
    let seconds = value.trim().parse::<f64>().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    Some((seconds * 1_000.0).round() as i64)
}

fn parse_lrc_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.trim().split_once(':')?;
    let minutes = minutes.trim().parse::<u64>().ok()?;
    let seconds = seconds.trim().parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    minutes
        .checked_mul(60_000)?
        .checked_add((seconds * 1_000.0).round() as u64)
}

fn adjust_simpmusic_time(milliseconds: u64, offset_ms: i64) -> u64 {
    (i128::from(milliseconds) + i128::from(offset_ms)).clamp(0, i128::from(u64::MAX)) as u64
}

fn parse_simpmusic_plain(document: &str) -> Option<Vec<LyricLine>> {
    let lines = document
        .lines()
        .map(|line| normalize_lyric_text(&decode_html_entities(line)))
        .filter(|line| !is_break_marker(line) && !is_section_label(line))
        .map(|text| LyricLine {
            text,
            start_ms: None,
            end_ms: None,
            syllables: Vec::new(),
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(lines)
}

fn normalize_lyric_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(index) = rest.find('&') {
        decoded.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        let Some(semicolon) = after.find(';') else {
            decoded.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..semicolon];
        if let Some(character) = decode_html_entity(entity) {
            decoded.push(character);
            rest = &after[semicolon + 1..];
        } else {
            decoded.push('&');
            rest = after;
        }
    }
    decoded.push_str(rest);
    decoded
}

fn decode_html_entity(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .filter(|character| *character != '\0');
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .filter(|character| *character != '\0');
    }
    match entity.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "hellip" => Some('…'),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "trade" => Some('™'),
        _ => None,
    }
}

fn is_break_marker(text: &str) -> bool {
    text.chars()
        .all(|character| character.is_whitespace() || matches!(character, '♪' | '♫'))
}

fn is_section_label(text: &str) -> bool {
    let trimmed = text.trim().trim_end_matches(|character: char| {
        character.is_whitespace() || matches!(character, '♪' | '♫')
    });
    trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .is_some_and(is_section_name)
}

fn is_section_tag(tag: &str) -> bool {
    is_section_name(tag)
}

fn is_metadata_tag(tag: &str) -> bool {
    let Some((key, _)) = tag.split_once(':') else {
        return false;
    };
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "ti" | "ar" | "al" | "au" | "by" | "offset" | "length" | "re" | "ve" | "tool" | "encoding"
    )
}

fn is_section_name(inner: &str) -> bool {
    let inner = inner.trim().trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '♪' | '♫')
    });
    if inner.is_empty() {
        return true;
    }

    let lower = inner.to_ascii_lowercase();
    if lower == "music" || lower.starts_with("music ") || lower.starts_with("instrumental") {
        return true;
    }
    let base = lower
        .split([':', '|'])
        .next()
        .unwrap_or("")
        .split(" - ")
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches(|character: char| {
            character.is_ascii_digit() || character.is_whitespace()
        });

    matches!(
        base,
        "music"
            | "intro"
            | "outro"
            | "verse"
            | "pre-chorus"
            | "prechorus"
            | "post-chorus"
            | "postchorus"
            | "chorus"
            | "bridge"
            | "hook"
            | "refrain"
            | "interlude"
            | "solo"
            | "guitar solo"
            | "break"
            | "breakdown"
            | "skit"
            | "spoken"
            | "spoken word"
            | "ad-lib"
            | "adlib"
            | "ad libs"
            | "adlibs"
            | "background"
            | "background vocals"
            | "backing vocals"
            | "vocals"
            | "bg"
            | "duet"
            | "male"
            | "female"
            | "v1"
            | "v2"
            | "v3"
            | "applause"
            | "laughs"
            | "laughter"
            | "fade out"
            | "fadeout"
            | "end"
    )
}

fn valid_youtube_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Parses Luna's KRC payload. Each line is `[start,duration]` followed by
/// `<offset,duration,?>text` segments; the offsets are relative to the line.
fn parse_krc(document: &str) -> Option<Vec<LyricLine>> {
    let mut lines = Vec::new();
    for raw_line in document.lines() {
        let raw_line = raw_line.trim();
        let Some(rest) = raw_line.strip_prefix('[') else {
            continue;
        };
        let Some((header, content)) = rest.split_once(']') else {
            continue;
        };
        let Some((start, duration)) = header.split_once(',') else {
            continue;
        };
        let Ok(start_ms) = start.trim().parse::<u64>() else {
            continue;
        };
        let Ok(duration_ms) = duration.trim().parse::<u64>() else {
            continue;
        };
        let line_end = start_ms.saturating_add(duration_ms);

        let mut segments = Vec::new();
        let mut remaining = content;
        while let Some(open) = remaining.find('<') {
            let after_open = &remaining[open + 1..];
            let Some(close) = after_open.find('>') else {
                break;
            };
            let header = &after_open[..close];
            let after = &after_open[close + 1..];
            let next = after.find('<').unwrap_or(after.len());
            if let Some(offset) = header
                .split(',')
                .next()
                .and_then(|value| value.trim().parse::<u64>().ok())
            {
                segments.push((offset, &after[..next]));
            }
            remaining = &after[next..];
        }

        let mut syllables: Vec<LyricSyllable> = Vec::new();
        let mut full = String::new();
        for (index, (offset, raw_text)) in segments.iter().enumerate() {
            let text = raw_text.trim();
            if text.is_empty() {
                if let Some(last) = syllables.last_mut()
                    && !last.tail.ends_with(' ')
                {
                    last.tail.push(' ');
                }
                continue;
            }
            let start = start_ms.saturating_add(*offset);
            let end = segments
                .get(index + 1)
                .map(|(next, _)| start_ms.saturating_add(*next))
                .unwrap_or(line_end);
            let tail = if raw_text.ends_with(char::is_whitespace) {
                " "
            } else {
                ""
            };
            syllables.push(LyricSyllable {
                text: text.to_owned(),
                tail: tail.to_owned(),
                start_ms: start,
                end_ms: end.max(start),
            });
            full.push_str(text);
            full.push_str(tail);
        }

        let text = if syllables.is_empty() {
            content.trim().to_owned()
        } else {
            full.trim().to_owned()
        };
        if text.is_empty() {
            continue;
        }
        lines.push(LyricLine {
            text,
            start_ms: Some(start_ms),
            end_ms: Some(line_end.max(start_ms)),
            syllables,
        });
    }

    if lines.is_empty() {
        return None;
    }
    lines.sort_by_key(|line| line.start_ms.unwrap_or(0));
    let last_index = lines.len() - 1;
    for index in 0..lines.len() {
        let start = lines[index].start_ms.unwrap_or(0);
        if lines[index].end_ms.unwrap_or(start) <= start {
            lines[index].end_ms = Some(
                lines
                    .get(index + 1)
                    .and_then(|line| line.start_ms)
                    .or_else(|| (index == last_index).then_some(start.saturating_add(5_000)))
                    .unwrap_or(start),
            );
        }
    }
    Some(lines)
}

fn parse_plain_lyrics(document: &str) -> Option<Vec<LyricLine>> {
    let lines = document
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "♪")
        .map(|text| LyricLine {
            text: text.to_owned(),
            start_ms: None,
            end_ms: None,
            syllables: Vec::new(),
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(lines)
}

fn strip_leading_credits_from_lines(lines: &mut Vec<LyricLine>, terms: &SearchTerms) {
    // Plain lyrics carry no timing, so there is no safe way to tell a short
    // opening lyric from a bare credit name.
    if lines.iter().all(|line| line.start_ms.is_none()) {
        return;
    }

    const MAX_CREDIT_LINES: usize = 24;
    let mut dropped = 0;
    while dropped < lines.len() && dropped < MAX_CREDIT_LINES {
        let next_start_ms = lines.get(dropped + 1).and_then(|line| line.start_ms);
        let line = &lines[dropped];
        let start_ms = line.start_ms.unwrap_or(0);
        let end_ms = line.end_ms.unwrap_or(start_ms);
        if !is_credit_line(&line.text, terms, start_ms, end_ms, next_start_ms) {
            break;
        }
        dropped += 1;
    }
    if dropped < lines.len() {
        lines.drain(..dropped);
    }
}

fn luna_songwriters(seo_track: Option<LunaSeoTrack>) -> Vec<String> {
    let Some(team) = seo_track
        .and_then(|seo_track| seo_track.track)
        .and_then(|track| track.song_maker_team)
    else {
        return Vec::new();
    };

    let mut songwriters = Vec::new();
    for credit in team.composers.into_iter().chain(team.lyricists) {
        let name = credit.name.trim();
        if !name.is_empty() && !songwriters.iter().any(|existing| existing == name) {
            songwriters.push(name.to_owned());
        }
    }
    songwriters
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
        LrcLibSearchResult, LunaDetailResponse, LunaSearchResponse, Lyrics, LyricsClient,
        LyricsTiming, LyricsTrack, SearchTerms, SimpMusicResponse, UnisonData, UnisonResponse,
        allowed_provider_url, best_bini_match, best_lrclib_match, clean_artist, clean_title,
        decode_html_entities, luna_candidates, match_score, parse_kpoe, parse_krc, parse_lrclib,
        parse_luna, parse_simpmusic, parse_unison, titles_differ_by_one_edit, tokens,
        unison_sync_timing, valid_youtube_video_id,
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
    async fn loads_luna_word_synced_lyrics() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_luna(&LyricsTrack {
                title: "Nothin' On You".to_owned(),
                artist: "B.o.B".to_owned(),
                album: None,
                duration_seconds: Some(230),
                video_id: None,
            })
            .await
            .unwrap()
            .expect("Luna carries the original Nothing On You");

        assert_eq!(lyrics.source, "Luna");
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert!(
            lyrics.lines[0].text.contains("Beautiful girls"),
            "unexpected first line: {:?}",
            lyrics.lines[0].text
        );
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_luna_lyrics_for_a_chinese_track() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_luna(&LyricsTrack {
                title: "起风了".to_owned(),
                artist: "买辣椒也用券".to_owned(),
                album: None,
                duration_seconds: Some(325),
                video_id: None,
            })
            .await
            .unwrap()
            .expect("Luna carries the original 起风了");

        assert_eq!(lyrics.source, "Luna");
        assert!(lyrics.timing > LyricsTiming::Plain);
        assert!(!lyrics.lines.is_empty());
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_simpmusic_word_synced_lyrics() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_simpmusic(&LyricsTrack {
                title: "Nothin' On You".to_owned(),
                artist: "B.o.B".to_owned(),
                album: None,
                duration_seconds: Some(230),
                video_id: Some("8PTDv_szmL0".to_owned()),
            })
            .await
            .unwrap()
            .expect("SimpMusic carries the exact video's rich sync");

        assert_eq!(lyrics.source, "SimpMusic");
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.lines[0].text, "Beautiful girls all over the world");
        assert!(
            lyrics
                .lines
                .iter()
                .any(|line| line.text.contains("Nothin' on you, ba by, yeah")),
            "unexpected lines: {:?}",
            lyrics
                .lines
                .iter()
                .map(|line| &line.text)
                .collect::<Vec<_>>()
        );
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
    async fn prefers_apple_when_the_video_is_a_shortened_edit() {
        // The official video runs 230s but Apple's master is 268s, so the
        // duration gap must not disqualify the only real match.
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch(LyricsTrack {
                title: "B.o.B - Nothin' On You (feat. Bruno Mars) [Official Video]".to_owned(),
                artist: "B.o.B".to_owned(),
                album: Some("B.o.B Presents: The Adventures of Bobby Ray".to_owned()),
                duration_seconds: Some(230),
                video_id: None,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(lyrics.source, "Apple (via BiniLyrics)");
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(
            lyrics.lines[0].text, "Beautiful girls all over the world",
            "songwriter credits must never be rendered as a lyric line"
        );
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn lyrics_plus_serves_no_credit_lines_as_lyrics() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_kpoe(&LyricsTrack {
                title: "B.o.B - Nothin' On You (feat. Bruno Mars) [Official Video]".to_owned(),
                artist: "B.o.B".to_owned(),
                album: None,
                duration_seconds: Some(263),
                video_id: None,
            })
            .await
            .unwrap()
            .unwrap();

        assert!(
            !lyrics.lines.iter().any(|line| line.text.contains('/')),
            "credit blocks are not lyrics: {:?}",
            lyrics.lines.first().map(|line| &line.text)
        );
    }

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn lyrics_plus_strips_a_bare_songwriter_credit() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch_kpoe(&LyricsTrack {
                title: "The Script - Superheroes (Official Video)".to_owned(),
                artist: "The Script".to_owned(),
                album: None,
                duration_seconds: Some(250),
                video_id: None,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            lyrics.lines.first().map(|line| line.text.as_str()),
            Some("All her life she has seen"),
            "the QQ Music payload opens with a songwriter credit, not a lyric"
        );
        assert!(
            !lyrics.lines.iter().any(|line| line.text == "James Barry"),
            "songwriter credits are metadata, never lyric lines"
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
        assert_eq!(
            clean_title("Song (Official Live Video)", "X"),
            "Song",
            "an 'official live video' marker is upload noise"
        );
        assert_eq!(
            clean_title("Song - Live", "X"),
            "Song - Live",
            "a bare 'Live' marks a different recording and must survive"
        );
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
                > match_score(&terms, "Tahanan Na Naman", "Adie", Some(196.0))
        );
    }

    #[test]
    fn rejects_a_credit_line_rendered_as_a_lyric() {
        let terms = SearchTerms::from_track(&track(
            "Nothin' on You (feat. Bruno Mars)",
            "B.o.B",
            Some(230),
        ));

        assert!(
            match_score(
                &terms,
                "Ari Levine/Bobby Simmons Jr./Bruno Mars/Philip Lawrence",
                "B.o.B",
                Some(230.0)
            )
            .is_none(),
            "songwriter credits are metadata, never a lyric line"
        );
    }

    #[test]
    fn rejects_versions_the_target_did_not_ask_for() {
        let terms = SearchTerms::from_track(&track(
            "Nothin' on You (feat. Bruno Mars)",
            "B.o.B",
            Some(230),
        ));

        for unwanted in [
            "Nothin' on You (Acappella) [feat. Bruno Mars]",
            "Nothin' On You (TC's Switch-Up Remix) [feat. Bruno Mars]",
            "Nothin' On You (TC's Switch-Up Extended Version)",
            "Nothin' on You (Karaoke Version)",
        ] {
            assert!(
                match_score(&terms, unwanted, "B.o.B", Some(237.0)).is_none(),
                "{unwanted} is a different recording"
            );
        }

        assert!(
            match_score(
                &terms,
                "Nothin' on You (feat. Bruno Mars)",
                "B.o.B",
                Some(268.0)
            )
            .is_some(),
            "the album track still matches despite being 38s longer than the video"
        );
    }

    #[test]
    fn still_matches_a_version_the_target_does_ask_for() {
        let terms = SearchTerms::from_track(&track("Nothin' on You (Live)", "B.o.B", Some(237)));
        assert!(match_score(&terms, "Nothin' On You (Live)", "B.o.B", Some(237.0)).is_some());
    }

    #[test]
    fn matches_elided_title_variants_without_loosening_artist_matching() {
        let terms = SearchTerms::from_track(&track("Nothin' On You", "B.o.B", Some(263)));
        assert!(
            match_score(&terms, "Nothing On You", "Bruno Mars, B.o.B", Some(255.0)).is_some(),
            "providers spell elided titles differently"
        );
        assert!(
            match_score(&terms, "Nothing On You", "Some Cover Band", Some(255.0)).is_none(),
            "a fuzzy title still needs the right artist"
        );
        assert!(titles_differ_by_one_edit("nothinonyou", "nothingonyou"));
        assert!(!titles_differ_by_one_edit("sing", "sin"));
    }

    #[test]
    fn parses_krc_word_and_cjk_timings() {
        let lines = parse_krc(
            "[20,2020]<0,350,0>Beautiful <880,1140,0>girls\n\
             [2040,1450]<0,250,0>All <340,270,0>over <860,270,0>the <1180,270,0>world",
        )
        .unwrap();

        assert_eq!(lines[0].text, "Beautiful girls");
        assert_eq!(lines[0].start_ms, Some(20));
        assert_eq!(lines[0].end_ms, Some(2040));
        assert_eq!(lines[0].syllables.len(), 2);
        assert_eq!(lines[0].syllables[0].text, "Beautiful");
        assert_eq!(lines[0].syllables[0].tail, " ");
        assert_eq!(lines[0].syllables[0].start_ms, 20);
        assert_eq!(lines[0].syllables[0].end_ms, 900);
        assert_eq!(lines[0].syllables[1].start_ms, 900);
        assert_eq!(lines[0].syllables[1].end_ms, 2040);

        let cjk =
            parse_krc("[332,1536]<0,296,0>写<320,296,0>一<640,288,0>个<928,288,0>剧<1240,296,0>本")
                .unwrap();
        assert_eq!(cjk[0].text, "写一个剧本");
        assert_eq!(cjk[0].syllables.len(), 5);
        assert_eq!(cjk[0].syllables[1].start_ms, 652);
        assert_eq!(cjk[0].syllables[4].end_ms, 1868);
    }

    #[test]
    fn parses_luna_krc_and_songwriters() {
        let terms = SearchTerms::from_track(&track("Nothin' On You", "B.o.B", Some(263)));
        let response: LunaDetailResponse = serde_json::from_str(
            r#"{
                "lyric": {
                    "content": "[20,2020]<0,350,0>Beautiful <880,1140,0>girls",
                    "type": "krc"
                },
                "seo_track": {
                    "track": {
                        "song_maker_team": {
                            "composers": [{"name": "Bruno Mars"}],
                            "lyricists": [{"name": "Philip Lawrence"}, {"name": "Bruno Mars"}]
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let lyrics = parse_luna(response, &terms).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "Luna");
        assert_eq!(lyrics.lines[0].text, "Beautiful girls");
        assert_eq!(lyrics.songwriters, ["Bruno Mars", "Philip Lawrence"]);
    }

    #[test]
    fn parses_luna_lrc_fallback() {
        let terms = SearchTerms::from_track(&track("Nothin' On You", "B.o.B", Some(263)));
        let response: LunaDetailResponse = serde_json::from_str(
            r#"{
                "lyric": {
                    "content": "[00:01.00]First\n[00:02.00]Second",
                    "type": "lrc"
                }
            }"#,
        )
        .unwrap();

        let lyrics = parse_luna(response, &terms).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.lines[0].text, "First");
        assert_eq!(lyrics.lines[0].end_ms, Some(2_000));
    }

    #[test]
    fn ranks_luna_originals_and_rejects_cover_candidates() {
        let terms = SearchTerms::from_track(&track("Nothin' On You", "B.o.B", Some(263)));
        let response: LunaSearchResponse = serde_json::from_str(
            r#"{
                "result_groups": [{
                    "data": [
                        {
                            "entity": {"track": {
                                "id": "cover",
                                "name": "Nothin' On You (Cover)",
                                "artists": [{"name": "Plusmusic"}],
                                "duration": 302000
                            }}
                        },
                        {
                            "entity": {"track": {
                                "id": "original",
                                "name": "Nothing On You",
                                "artists": [{"name": "Bruno Mars"}, {"name": "B.o.B"}],
                                "duration": 255295,
                                "label_info": {"is_original": true}
                            }}
                        }
                    ]
                }]
            }"#,
        )
        .unwrap();

        let candidates = luna_candidates(&terms, response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "original");
    }

    #[test]
    fn strips_luna_credits_before_timed_lyrics() {
        let terms = SearchTerms::from_track(&track("Superheroes", "The Script", Some(250)));
        let response: LunaDetailResponse = serde_json::from_str(
            r#"{
                "lyric": {
                    "content": "[9070,2475]James Barry\n[23118,2628]All her life she has seen",
                    "type": "krc"
                }
            }"#,
        )
        .unwrap();

        let lyrics = parse_luna(response, &terms).unwrap();
        assert_eq!(lyrics.lines[0].text, "All her life she has seen");
        assert_eq!(lyrics.lines[0].start_ms, Some(23_118));
    }

    #[test]
    fn parses_simpmusic_rich_sync_with_overlapping_words() {
        let response: SimpMusicResponse = serde_json::from_str(
            r#"{
                "success": true,
                "data": [{
                    "vote": 5,
                    "contributor": "alice",
                    "richSyncLyrics": "[offset:-0.196136956521738]\n[00:04.59] <00:04.59>Beautiful <00:05.47>girls <00:06.59>all <00:06.94>over <00:07.49>the <00:07.80>world\n[00:36.86] <00:36.86>Nothin&#x27; <00:37.49>on <00:37.83>you, <00:38.89>ba <00:39.55>by, <00:41.30>yeah <00:38.36>(No- <00:38.72>no- <00:39.00>no- <00:39.30>nothin&#x27; <00:39.58>on <00:39.87>you, <00:40.15>babe)",
                    "syncedLyrics": "[00:01.00]Wrong",
                    "plainLyric": "Wrong"
                }]
            }"#,
        )
        .unwrap();

        let lyrics = parse_simpmusic(response).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "SimpMusic (submitted by alice)");
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "Beautiful girls all over the world");
        assert_eq!(lyrics.lines[0].start_ms, Some(4_394));
        assert_eq!(lyrics.lines[0].end_ms, Some(8_404));
        assert_eq!(lyrics.lines[0].syllables[0].text, "Beautiful");
        assert_eq!(lyrics.lines[0].syllables[0].tail, " ");
        assert_eq!(lyrics.lines[0].syllables[0].start_ms, 4_394);
        assert_eq!(lyrics.lines[0].syllables[0].end_ms, 5_274);

        let overlapping = &lyrics.lines[1];
        assert_eq!(overlapping.start_ms, Some(36_664));
        assert_eq!(overlapping.end_ms, Some(41_104));
        assert!(overlapping.text.contains("Nothin' on you, ba by, yeah"));
        assert!(
            overlapping
                .text
                .contains("(No- no- no- nothin' on you, babe)")
        );
        let yeah = overlapping
            .syllables
            .iter()
            .find(|syllable| syllable.text == "yeah")
            .unwrap();
        assert_eq!(yeah.start_ms, 41_104);
        assert_eq!(
            yeah.end_ms, 41_104,
            "an out-of-order word must not lose its own highlight window"
        );
        assert!(
            overlapping
                .syllables
                .windows(2)
                .any(|pair| pair[1].start_ms < pair[0].start_ms),
            "overlapping backing vocals keep their own timestamps"
        );
    }

    #[test]
    fn parses_simpmusic_lrc_with_offset_and_section_markers() {
        let response: SimpMusicResponse = serde_json::from_str(
            r#"{
                "success": true,
                "data": [{
                    "vote": 1,
                    "syncedLyrics": "[offset:-0.25]\n[ti:Song]\n[00:01.00][Music]\n[00:02.50] First &amp; second\n[00:05.00] It&#x27;s &quot;fine&quot;\n[00:07.00] ♪\n[00:08.00][Chorus] Last"
                }]
            }"#,
        )
        .unwrap();

        let lyrics = parse_simpmusic(response).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::LineSynced);
        assert_eq!(lyrics.source, "SimpMusic");
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.lines[0].text, "First & second");
        assert_eq!(lyrics.lines[0].start_ms, Some(2_250));
        assert_eq!(lyrics.lines[0].end_ms, Some(4_750));
        assert_eq!(lyrics.lines[1].text, "It's \"fine\"");
        assert_eq!(lyrics.lines[1].start_ms, Some(4_750));
        assert_eq!(lyrics.lines[2].text, "Last");
        assert_eq!(lyrics.lines[2].start_ms, Some(7_750));
        assert_eq!(lyrics.lines[2].end_ms, Some(12_750));
    }

    #[test]
    fn prefers_the_highest_voted_simpmusic_entry() {
        let response: SimpMusicResponse = serde_json::from_str(
            r#"{
                "success": true,
                "data": [
                    {"vote": 1, "richSyncLyrics": "[00:01.00] <00:01.00>Low"},
                    {"vote": 9, "plainLyric": "High\n[Music]\nStill high"},
                    {"vote": 5, "syncedLyrics": "[00:01.00]Mid"}
                ]
            }"#,
        )
        .unwrap();

        let lyrics = parse_simpmusic(response).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::Plain);
        assert_eq!(lyrics.source, "SimpMusic");
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["High", "Still high"]
        );
    }

    #[test]
    fn prefers_rich_sync_over_synced_and_plain_within_a_simpmusic_entry() {
        let response: SimpMusicResponse = serde_json::from_str(
            r#"{
                "success": true,
                "data": [{
                    "vote": 3,
                    "richSyncLyrics": "[00:01.00] <00:01.00>Rich <00:02.00>sync",
                    "syncedLyrics": "[00:01.00]Synced",
                    "plainLyric": "Plain"
                }]
            }"#,
        )
        .unwrap();

        let lyrics = parse_simpmusic(response).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.lines[0].text, "Rich sync");
        assert_eq!(lyrics.lines[0].syllables.len(), 2);
    }

    #[test]
    fn falls_back_to_simpmusic_plain_lyrics() {
        let response: SimpMusicResponse = serde_json::from_str(
            r#"{
                "success": true,
                "data": [{
                    "vote": 1,
                    "richSyncLyrics": "[00:01.00][Music]",
                    "syncedLyrics": "[00:01.00][Chorus]",
                    "plainLyric": "Line one\n[Music]\nLine &amp; two"
                }]
            }"#,
        )
        .unwrap();

        let lyrics = parse_simpmusic(response).unwrap();
        assert_eq!(lyrics.timing, LyricsTiming::Plain);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].text, "Line one");
        assert_eq!(lyrics.lines[1].text, "Line & two");
    }

    #[test]
    fn decodes_simpmusic_html_entities_without_touching_unknown_text() {
        assert_eq!(
            decode_html_entities("&#x27;Cause &amp; &quot;effect&quot; &#39;"),
            "'Cause & \"effect\" '"
        );
        assert_eq!(decode_html_entities("&unknown; &amp"), "&unknown; &amp");
    }

    #[test]
    fn validates_simpmusic_video_ids_and_host() {
        assert!(valid_youtube_video_id("8PTDv_szmL0"));
        assert!(!valid_youtube_video_id(""));
        assert!(!valid_youtube_video_id("short"));
        assert!(!valid_youtube_video_id("8PTDv_szmL0/../x"));
        assert!(allowed_provider_url(
            &reqwest::Url::parse("https://api-lyrics.simpmusic.org/v1/8PTDv_szmL0").unwrap()
        ));
        assert!(!allowed_provider_url(
            &reqwest::Url::parse(
                "https://api-lyrics.simpmusic.org.attacker.example/v1/8PTDv_szmL0"
            )
            .unwrap()
        ));
    }

    #[test]
    fn ranks_a_matching_title_above_a_mere_containment() {
        let terms = SearchTerms::from_track(&track("Tahanan", "Adie", Some(196)));
        let exact = best_lrclib_match(
            &terms,
            vec![
                lrclib_hit("Tahanan", "Adie feat. Someone", 196.0),
                lrclib_hit("Tahanan Na Naman", "Adie", 196.0),
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

    fn kpoe_line(time: u64, duration: u64, text: &str) -> KpoeLine {
        KpoeLine {
            time: Some(time),
            duration: Some(duration),
            text: text.to_owned(),
            syllabus: Vec::new(),
        }
    }

    fn kpoe_lyrics(lines: Vec<KpoeLine>, terms: &SearchTerms) -> Option<Lyrics> {
        parse_kpoe(
            KpoeResponse {
                ttml: None,
                timing: "Line".to_owned(),
                metadata: Some(KpoeMetadata {
                    source: "QQ Music".to_owned(),
                    song_writers: Vec::new(),
                }),
                lyrics: lines,
            },
            terms,
        )
    }

    #[test]
    fn parses_word_synced_kpoe_payloads() {
        let lyrics = parse_kpoe(
            KpoeResponse {
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
            },
            &SearchTerms::from_track(&track("Hotshot", "GIMS", Some(240))),
        )
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
        let lyrics = parse_kpoe(
            KpoeResponse {
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
            },
            &SearchTerms::from_track(&track("First", "Test", Some(10))),
        )
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
    fn strips_a_bare_name_credit_before_the_first_lyric() {
        let terms = SearchTerms::from_track(&track("Superheroes", "The Script", Some(250)));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(9_070, 2_475, "James Barry"),
                kpoe_line(23_118, 2_628, "All her life she has seen"),
                kpoe_line(25_747, 2_770, "Oh the meanest side of me"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(
            lyrics.lines[0].text, "All her life she has seen",
            "James Barry is a songwriter, not the opening lyric"
        );
        assert_eq!(lyrics.lines[0].start_ms, Some(23_118));
        assert_eq!(lyrics.lines.len(), 2);
    }

    #[test]
    fn strips_a_slash_list_and_a_credit_colon() {
        let terms = SearchTerms::from_track(&track(
            "Nothin' on You (feat. Bruno Mars)",
            "B.o.B",
            Some(230),
        ));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(
                    1_599,
                    1_439,
                    "Ari Levine/Bobby Simmons Jr./Bruno Mars/Philip Lawrence",
                ),
                kpoe_line(4_477, 800, "Bruno Mars/B.o.B："),
                kpoe_line(5_278, 1_641, "Beautiful girls"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "Beautiful girls");
    }

    #[test]
    fn strips_a_title_artist_header_and_writer_list() {
        let terms = SearchTerms::from_track(&track("Perfect", "Ed Sheeran", Some(263)));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(0, 880, "Perfect - Ed Sheeran (艾德·希兰)"),
                kpoe_line(1_200, 560, "Ed Sheeran/Matthew Sheeran"),
                kpoe_line(2_652, 5_836, "I found a love for me"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(lyrics.lines[0].text, "I found a love for me");
    }

    #[test]
    fn recognizes_a_header_even_when_the_artist_string_is_longer() {
        let terms = SearchTerms::from_track(&track(
            "See You Again",
            "Wiz Khalifa feat. Charlie Puth",
            Some(230),
        ));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(
                    0,
                    1_830,
                    "See You Again - Wiz Khalifa (维兹·卡利法)/Charlie Puth (查理·普斯)",
                ),
                kpoe_line(2_066, 1_593, "Andrew Cedar/Cameron Thomaz/Charlie Puth"),
                kpoe_line(10_993, 6_716, "It's been a long day without you my friend"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(lyrics.lines.len(), 1);
        assert_eq!(
            lyrics.lines[0].text,
            "It's been a long day without you my friend"
        );
    }

    #[test]
    fn keeps_a_short_name_like_line_when_it_flows_into_the_next_lyric() {
        let terms = SearchTerms::from_track(&track("Summer Night", "Test", Some(200)));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(1_000, 2_000, "Mamma Mia"),
                kpoe_line(3_500, 2_000, "Here we go again"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(
            lyrics.lines[0].text, "Mamma Mia",
            "a normal-length opening line is not a credit"
        );
    }

    #[test]
    fn keeps_a_credit_looking_line_after_the_first_lyric() {
        let terms = SearchTerms::from_track(&track("Superheroes", "The Script", Some(250)));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(23_118, 2_628, "All her life she has seen"),
                kpoe_line(60_000, 2_000, "James Barry"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[1].text, "James Barry");
    }

    #[test]
    fn does_not_empty_an_all_credit_payload() {
        let terms = SearchTerms::from_track(&track("Superheroes", "The Script", Some(250)));
        let lyrics = kpoe_lyrics(
            vec![
                kpoe_line(1_000, 500, "James Barry"),
                kpoe_line(2_000, 500, "Ryan Tedder"),
            ],
            &terms,
        )
        .unwrap();

        assert_eq!(
            lyrics.lines.len(),
            2,
            "an all-credit payload is kept rather than returning no lyrics"
        );
    }

    #[test]
    fn rejects_empty_kpoe_payloads() {
        assert!(
            parse_kpoe(
                KpoeResponse {
                    ttml: None,
                    timing: "Line".to_owned(),
                    metadata: None,
                    lyrics: Vec::new(),
                },
                &SearchTerms::from_track(&track("First", "Test", Some(10)))
            )
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
