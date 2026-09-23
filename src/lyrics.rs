use reqwest::StatusCode;
use serde::Deserialize;
use std::{error::Error, fmt, time::Duration};

const BINILYRICS_SEARCH_URL: &str = "https://lyrics-api.binimum.org/getLyrics";
const BINILYRICS_STORAGE_HOST: &str = "lyrics-storage.binimum.org";
const LRCLIB_URL: &str = "https://lrclib.net/api/get";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

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
}

#[derive(Clone, Debug)]
pub struct LyricsTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_seconds: Option<u64>,
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
        let (bini, lrclib) = tokio::join!(self.fetch_binilyrics(&track), self.fetch_lrclib(&track));
        let candidates = [bini.as_ref().ok(), lrclib.as_ref().ok()]
            .into_iter()
            .flatten()
            .filter_map(|lyrics| lyrics.clone())
            .collect::<Vec<_>>();

        if let Some(lyrics) = Lyrics::best_available(candidates) {
            return Ok(Some(lyrics));
        }

        match (bini, lrclib) {
            (Err(bini), Err(lrclib)) => Err(LyricsFetchError(format!(
                "lyrics providers failed: {bini}; {lrclib}"
            ))),
            (Err(error), Ok(None)) | (Ok(None), Err(error)) => Err(error),
            _ => Ok(None),
        }
    }

    async fn fetch_binilyrics(
        &self,
        track: &LyricsTrack,
    ) -> Result<Option<Lyrics>, LyricsFetchError> {
        let query = format!("{} {}", track.title, track.artist);
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
        if lyrics_url.scheme() != "https" || lyrics_url.host_str() != Some(BINILYRICS_STORAGE_HOST)
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
        lyrics.source = "BiniLyrics".to_owned();
        Ok(Some(lyrics))
    }

    async fn fetch_lrclib(&self, track: &LyricsTrack) -> Result<Option<Lyrics>, LyricsFetchError> {
        let mut query = vec![
            ("track_name", track.title.as_str()),
            ("artist_name", track.artist.as_str()),
        ];
        if let Some(album) = track.album.as_deref() {
            query.push(("album_name", album));
        }
        let duration = track.duration_seconds.map(|duration| duration.to_string());
        if let Some(duration) = duration.as_deref() {
            query.push(("duration", duration));
        }
        let response = self
            .client
            .get(LRCLIB_URL)
            .query(&query)
            .send()
            .await
            .map_err(provider_error("LRCLIB"))?;
        let Some(body) = response_body(response, "LRCLIB").await? else {
            return Ok(None);
        };
        let response: LrcLibResponse = serde_json::from_slice(&body)
            .map_err(|error| LyricsFetchError(format!("invalid LRCLIB response: {error}")))?;
        Ok(parse_lrclib(response))
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

    pub fn from_ttml(document: &str) -> Result<Self, TtmlError> {
        let document = roxmltree::Document::parse(document)
            .map_err(|error| TtmlError(format!("invalid TTML: {error}")))?;
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
            Some("lyrics-api.binimum.org" | "lyrics-storage.binimum.org" | "lrclib.net")
        )
}

fn best_bini_match(
    track: &LyricsTrack,
    results: Vec<BiniLyricsResult>,
) -> Option<BiniLyricsResult> {
    results
        .into_iter()
        .filter(|result| !result.lyrics_url.is_empty())
        .filter(|result| {
            let target_title = normalized_match_text(&track.title);
            let result_title = normalized_match_text(&result.track_name);
            let target_artist = normalized_match_text(&track.artist);
            let result_artist = normalized_match_text(&result.artist_name);
            let title_matches = !target_title.is_empty()
                && !result_title.is_empty()
                && (target_title == result_title
                    || target_title.contains(&result_title)
                    || result_title.contains(&target_title));
            let artist_matches = !target_artist.is_empty()
                && !result_artist.is_empty()
                && (target_artist == result_artist
                    || target_artist.contains(&result_artist)
                    || result_artist.contains(&target_artist));
            let duration_matches = track.duration_seconds.is_none_or(|duration| {
                result.duration <= 0.0 || (result.duration - duration as f64).abs() <= 8.0
            });
            title_matches && artist_matches && duration_matches
        })
        .max_by_key(|result| {
            let target_title = normalized_match_text(&track.title);
            let result_title = normalized_match_text(&result.track_name);
            let target_artist = normalized_match_text(&track.artist);
            let result_artist = normalized_match_text(&result.artist_name);
            let mut score = 0;
            if !target_title.is_empty() && target_title == result_title {
                score += 15;
            } else if !result_title.is_empty()
                && (target_title.contains(&result_title) || result_title.contains(&target_title))
            {
                score += 10;
            }
            if !target_artist.is_empty() && target_artist == result_artist {
                score += 15;
            } else if !result_artist.is_empty()
                && (target_artist.contains(&result_artist)
                    || result_artist.contains(&target_artist))
            {
                score += 10;
            }
            if let Some(duration) = track.duration_seconds {
                let difference = (result.duration - duration as f64).abs();
                score += if difference <= 3.0 {
                    5
                } else if difference <= 8.0 {
                    2
                } else {
                    0
                };
            }
            score
        })
}

fn normalized_match_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn parse_lrclib(response: LrcLibResponse) -> Option<Lyrics> {
    if let Some(synced) = response.synced_lyrics
        && let Some(lines) = parse_lrc(&synced)
    {
        return Some(Lyrics {
            timing: LyricsTiming::LineSynced,
            lines,
            source: "LRCLIB".to_owned(),
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
        BiniLyricsResult, LrcLibResponse, Lyrics, LyricsClient, LyricsTiming, LyricsTrack,
        allowed_provider_url, best_bini_match, parse_lrclib,
    };

    #[test]
    fn parses_syllable_synced_ttml() {
        let lyrics = Lyrics::from_ttml(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
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

    #[tokio::test]
    #[ignore = "live lyrics provider compatibility probe"]
    async fn loads_syllable_synced_lyrics_from_live_providers() {
        let lyrics = LyricsClient::new()
            .unwrap()
            .fetch(LyricsTrack {
                title: "Tahanan".to_owned(),
                artist: "Adie".to_owned(),
                album: Some("Tahanan".to_owned()),
                duration_seconds: Some(294),
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(lyrics.timing, LyricsTiming::SyllableSynced);
        assert_eq!(lyrics.source, "BiniLyrics");
        assert!(!lyrics.lines.is_empty());
    }

    #[test]
    fn rejects_unrelated_bini_matches_and_spoofed_storage_hosts() {
        let track = LyricsTrack {
            title: "Tahanan".to_owned(),
            artist: "Adie".to_owned(),
            album: None,
            duration_seconds: Some(294),
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
    }
}
