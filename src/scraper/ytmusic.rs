use innertube_rs::{
    FormatFilter, FormatType, GetVideoInfoOptions, Innertube, MusicHomeFeed, MusicSearchResults,
    NodeListExt, Parser, QualityPreference, SessionOptions,
};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const MUSIC_CLIENT_ID: &str = "67";
const MUSIC_CLIENT_VERSION_FALLBACK: &str = "1.20250219.01.00";

pub struct AudioStreamInfo {
    pub url: String,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub tracking: Option<WatchTracking>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchTracking {
    pub cpn: String,
    pub playback_url: Option<String>,
    pub watchtime_url: Option<String>,
}

pub struct AccountIdentity {
    pub display_name: String,
    pub username: Option<String>,
}

pub struct MusicSearchTopResult {
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub video_id: Option<String>,
    pub browse_id: Option<String>,
    pub art_url: Option<String>,
}

pub struct YTMusicSearchResults {
    pub top_result: Option<MusicSearchTopResult>,
    pub sections: MusicSearchResults,
}

pub struct YTMusicHomeFeed {
    pub feed: MusicHomeFeed,
    pub play_targets: Vec<HomePlayTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomePlayTarget {
    pub item_id: String,
    pub title: String,
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
}

pub struct UpNextTrack {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<String>,
    pub art_url: Option<String>,
}

pub struct UpNextQueue {
    pub tracks: Vec<UpNextTrack>,
    pub current_index: usize,
}

pub struct PlaylistPlayback {
    pub title: String,
    pub tracks: Vec<UpNextTrack>,
}

struct PanelExtras {
    artist: Option<String>,
    album: Option<String>,
    art_url: Option<String>,
}

#[derive(Clone)]
pub struct YTMusic {
    yt: Innertube,
    playback: Innertube,
    music_client_version: String,
}

impl YTMusic {
    pub async fn new(cookie: Option<String>) -> innertube_rs::error::Result<Self> {
        let authenticated = cookie.is_some();
        let yt = Innertube::with_options(SessionOptions {
            cookie,
            ..Default::default()
        })
        .await?;
        let playback = if authenticated {
            Innertube {
                session: Arc::new(innertube_rs::Session::create(SessionOptions::default()).await?),
                player: yt.player.clone(),
            }
        } else {
            yt.clone()
        };
        let music_client_version = fetch_music_client_version(&yt.session.http_client)
            .await
            .unwrap_or_else(|_| MUSIC_CLIENT_VERSION_FALLBACK.to_owned());
        Ok(Self {
            yt,
            playback,
            music_client_version,
        })
    }

    pub async fn account_identity(&self) -> innertube_rs::error::Result<AccountIdentity> {
        let response = self
            .yt
            .session
            .post_innertube(
                "/account/accounts_list",
                json!({
                    "requestType": "ACCOUNTS_LIST_REQUEST_TYPE_CHANNEL_SWITCHER",
                    "callCircumstance": "SWITCHING_USERS_FULL"
                }),
            )
            .await?;
        let value: Value = response.json().await?;
        parse_account_identity(&value).ok_or_else(|| {
            innertube_rs::InnertubeError::Other(
                "YouTube did not accept the account cookie".to_owned(),
            )
        })
    }

    pub async fn get_home(&self) -> innertube_rs::error::Result<YTMusicHomeFeed> {
        let response = self
            .yt
            .session
            .post_innertube_client("YTMUSIC", "/browse", json!({ "browseId": "FEmusic_home" }))
            .await?;
        let raw: Value = response
            .json()
            .await
            .map_err(innertube_rs::InnertubeError::Network)?;
        parse_home_response(&raw)
    }

    pub async fn search(&self, query: &str) -> innertube_rs::error::Result<YTMusicSearchResults> {
        let response = self
            .yt
            .session
            .post_innertube_client("YTMUSIC", "/search", json!({ "query": query }))
            .await?;
        let raw: Value = response
            .json()
            .await
            .map_err(innertube_rs::InnertubeError::Network)?;
        let mut sections =
            innertube_rs::endpoints::music::parse_music_search_response(query, None, &raw)?;
        let mut playlists = parse_search_playlists(&raw);
        if playlists.is_empty() {
            let response = self
                .yt
                .session
                .post_innertube_client(
                    "YTMUSIC",
                    "/search",
                    json!({
                        "query": query,
                        "params": innertube_rs::MusicSearchFilter::Playlists.to_param_str(),
                    }),
                )
                .await?;
            let playlist_raw: Value = response
                .json()
                .await
                .map_err(innertube_rs::InnertubeError::Network)?;
            playlists = parse_search_playlists(&playlist_raw);
        }
        sections.songs.retain(|song| {
            !playlists
                .iter()
                .any(|playlist| playlist.title == song.title)
        });
        sections.playlists = playlists;
        Ok(YTMusicSearchResults {
            top_result: parse_search_top_result(&raw),
            sections,
        })
    }

    pub async fn get_up_next(
        &self,
        video_id: &str,
        playlist_id: Option<&str>,
    ) -> innertube_rs::error::Result<UpNextQueue> {
        let response = self
            .yt
            .session
            .post_innertube_client("YTMUSIC", "/next", up_next_payload(video_id, playlist_id))
            .await?;
        let value: Value = response.json().await?;
        let panel_value = find_playlist_panel(&value).ok_or_else(|| {
            innertube_rs::InnertubeError::Other("Could not fetch Automix queue".to_owned())
        })?;
        let panel = innertube_rs::PlaylistPanelNode::from_value(panel_value).ok_or_else(|| {
            innertube_rs::InnertubeError::Other("Could not fetch Automix queue".to_owned())
        })?;
        let extras = parse_panel_extras(panel_value);
        Ok(up_next_queue(panel, video_id, extras))
    }

    pub async fn get_playlist(
        &self,
        playlist_id: &str,
    ) -> innertube_rs::error::Result<PlaylistPlayback> {
        let browse_id = if playlist_id.starts_with("VL") {
            playlist_id.to_owned()
        } else {
            format!("VL{playlist_id}")
        };
        let response = self
            .yt
            .session
            .post_innertube_client("YTMUSIC", "/browse", json!({ "browseId": browse_id }))
            .await?;
        let raw: Value = response.json().await?;
        let title = find_music_playlist_title(&raw).unwrap_or_else(|| "Playlist".to_owned());
        let (mut tracks, mut continuation) = parse_music_playlist_page(&raw);
        let mut seen_continuations = HashSet::new();
        while let Some(token) = continuation {
            if !seen_continuations.insert(token.clone()) {
                break;
            }
            let response = self
                .yt
                .session
                .post_innertube_client("YTMUSIC", "/browse", json!({ "continuation": token }))
                .await?;
            let raw: Value = response.json().await?;
            let (page_tracks, next) = parse_music_playlist_page(&raw);
            tracks.extend(page_tracks);
            continuation = next;
        }
        if tracks.is_empty() {
            return Err(innertube_rs::InnertubeError::Other(
                "Playlist has no playable tracks".to_owned(),
            ));
        }
        Ok(PlaylistPlayback { title, tracks })
    }

    pub async fn get_audio_url(&self, video_id: &str) -> innertube_rs::error::Result<String> {
        let info = self
            .playback
            .get_basic_info(
                video_id,
                Some(&GetVideoInfoOptions {
                    client: Some("VISIONOS".to_owned()),
                    ..Default::default()
                }),
            )
            .await?;
        info.get_stream_url(
            &FormatFilter {
                format_type: FormatType::AudioOnly,
                quality: QualityPreference::Highest,
                container: None,
            },
            &self.playback.player.decipherer,
        )
    }

    pub async fn get_audio_stream(
        &self,
        video_id: &str,
        sync_history: bool,
    ) -> innertube_rs::error::Result<AudioStreamInfo> {
        let options = GetVideoInfoOptions {
            client: Some(if sync_history {
                "YTMUSIC".to_owned()
            } else {
                "VISIONOS".to_owned()
            }),
            ..Default::default()
        };
        let playback = if sync_history {
            &self.yt
        } else {
            &self.playback
        };
        let info_request = playback.get_basic_info(video_id, Some(&options));
        let likes_request = self.get_like_count(video_id);
        let (info, likes) = tokio::join!(info_request, likes_request);
        let info = info?;
        let views = info
            .player_response
            .video_details
            .as_ref()
            .and_then(|details| details.view_count.as_deref())
            .and_then(parse_count);
        let url = info.get_stream_url(
            &FormatFilter {
                format_type: FormatType::AudioOnly,
                quality: QualityPreference::Highest,
                container: None,
            },
            &playback.player.decipherer,
        )?;
        let tracking = if sync_history {
            extract_tracking(&info)
        } else {
            None
        };
        Ok(AudioStreamInfo {
            url,
            views,
            likes: likes.unwrap_or_default(),
            tracking,
        })
    }

    pub async fn report_playback_start(
        &self,
        tracking: &WatchTracking,
    ) -> innertube_rs::error::Result<()> {
        let Some(url) = tracking.playback_url.as_deref() else {
            return Ok(());
        };
        self.ping_stats(
            url,
            &[
                ("cpn", tracking.cpn.clone()),
                ("fmt", "251".to_owned()),
                ("rtn", "0".to_owned()),
                ("rt", "0".to_owned()),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn report_watch_time(
        &self,
        tracking: &WatchTracking,
        position: f64,
        final_ping: bool,
    ) -> innertube_rs::error::Result<()> {
        let Some(url) = tracking.watchtime_url.as_deref() else {
            return Ok(());
        };
        let ts = format!("{position:.3}");
        self.ping_stats(
            url,
            &[
                ("cpn", tracking.cpn.clone()),
                ("cmt", ts.clone()),
                ("st", ts.clone()),
                ("et", ts),
                ("final", if final_ping { "1" } else { "0" }.to_owned()),
            ],
        )
        .await?;
        Ok(())
    }

    /// Ping the YouTube Music videostats endpoint with account authentication.
    async fn ping_stats(
        &self,
        base_url: &str,
        params: &[(&str, String)],
    ) -> innertube_rs::error::Result<reqwest::Response> {
        let url = build_music_stats_url(base_url, &self.music_client_version, params)?;
        let mut headers = self.yt.session.build_innertube_headers();
        self.yt
            .session
            .apply_auth_headers(&mut headers, false)
            .await?;
        let resp = self
            .yt
            .session
            .http_client
            .get(url)
            .headers(headers)
            .header("Origin", MUSIC_ORIGIN)
            .header("Referer", format!("{MUSIC_ORIGIN}/"))
            .header("X-Youtube-Client-Name", MUSIC_CLIENT_ID)
            .header("X-Youtube-Client-Version", &self.music_client_version)
            .send()
            .await
            .map_err(innertube_rs::InnertubeError::Network)?;
        Ok(resp)
    }

    async fn get_like_count(&self, video_id: &str) -> innertube_rs::error::Result<Option<u64>> {
        let response = self
            .yt
            .session
            .post_innertube("/next", json!({ "videoId": video_id }))
            .await?;
        let value: Value = response.json().await?;
        Ok(find_like_count(&value))
    }
}

fn parse_music_playlist_page(raw: &Value) -> (Vec<UpNextTrack>, Option<String>) {
    let shelf = find_music_playlist_shelf(raw).unwrap_or(raw);
    let parsed = Parser::parse_tree(shelf);
    let tracks = parsed
        .find_music_items()
        .into_iter()
        .filter_map(|item| {
            let video_id = item.id.clone().filter(|id| !id.is_empty())?;
            let artist = item
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Some(UpNextTrack {
                video_id,
                title: item.title.clone(),
                artist: if artist.is_empty() {
                    "Unknown artist".to_owned()
                } else {
                    artist
                },
                album: item.album.clone(),
                duration: item.duration.clone(),
                art_url: item.thumbnails.best_url().map(ToOwned::to_owned),
            })
        })
        .collect();
    (
        tracks,
        parsed
            .find_continuation_token()
            .or_else(|| find_continuation_token(shelf)),
    )
}

fn find_continuation_token(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get("nextContinuationData")
            .and_then(|data| data.get("continuation"))
            .or_else(|| {
                object
                    .get("continuationCommand")
                    .and_then(|command| command.get("token"))
            })
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| object.values().find_map(find_continuation_token)),
        Value::Array(values) => values.iter().find_map(find_continuation_token),
        _ => None,
    }
}

fn find_music_playlist_shelf(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => object
            .get("musicPlaylistShelfRenderer")
            .or_else(|| object.get("musicPlaylistShelfContinuation"))
            .or_else(|| object.values().find_map(find_music_playlist_shelf)),
        Value::Array(values) => values.iter().find_map(find_music_playlist_shelf),
        _ => None,
    }
}

fn find_music_playlist_title(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in ["musicDetailHeaderRenderer", "musicResponsiveHeaderRenderer"] {
                if let Some(header) = object.get(key)
                    && let Some(title) = header.get("title").and_then(text_value)
                {
                    return Some(title);
                }
            }
            object.values().find_map(find_music_playlist_title)
        }
        Value::Array(values) => values.iter().find_map(find_music_playlist_title),
        _ => None,
    }
}

fn up_next_payload(video_id: &str, playlist_id: Option<&str>) -> Value {
    json!({
        "videoId": video_id,
        "playlistId": playlist_id
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("RDAMVM{video_id}")),
        "enablePersistentPlaylistPanel": true,
        "isAudioOnly": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
    })
}

fn parse_home_response(raw: &Value) -> innertube_rs::error::Result<YTMusicHomeFeed> {
    let feed = innertube_rs::endpoints::music::parse_music_home_response(raw)?;
    let mut play_targets = Vec::new();
    collect_home_play_targets(raw, &mut play_targets);
    Ok(YTMusicHomeFeed { feed, play_targets })
}

fn collect_home_play_targets(value: &Value, targets: &mut Vec<HomePlayTarget>) {
    match value {
        Value::Object(object) => {
            if let Some(renderer) = object.get("musicTwoRowItemRenderer")
                && let Some(target) = parse_home_play_target(renderer)
            {
                targets.push(target);
            }
            for child in object.values() {
                collect_home_play_targets(child, targets);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_home_play_targets(child, targets);
            }
        }
        _ => {}
    }
}

fn parse_home_play_target(renderer: &Value) -> Option<HomePlayTarget> {
    let navigation = renderer.get("navigationEndpoint")?;
    let item_id = navigation
        .pointer("/browseEndpoint/browseId")
        .or_else(|| navigation.pointer("/watchEndpoint/videoId"))
        .or_else(|| navigation.pointer("/watchPlaylistEndpoint/playlistId"))
        .and_then(Value::as_str)?;
    let play_navigation = renderer
        .pointer(
            "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint",
        )
        .unwrap_or(navigation);
    let watch_endpoint = play_navigation
        .get("watchEndpoint")
        .or_else(|| play_navigation.pointer("/commandExecutorCommand/commands/0/watchEndpoint"))?;
    let video_id = watch_endpoint
        .get("videoId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let playlist_id = watch_endpoint
        .get("playlistId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    if video_id.is_none() && playlist_id.is_none() {
        return None;
    }
    let title = renderer
        .pointer("/title/runs/0/text")
        .or_else(|| renderer.pointer("/title/simpleText"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(HomePlayTarget {
        item_id: item_id.to_owned(),
        title,
        video_id,
        playlist_id,
    })
}

async fn fetch_music_client_version(
    client: &reqwest::Client,
) -> innertube_rs::error::Result<String> {
    let response = client
        .get(format!("{MUSIC_ORIGIN}/sw.js_data"))
        .send()
        .await
        .map_err(innertube_rs::InnertubeError::Network)?;
    if !response.status().is_success() {
        return Err(innertube_rs::InnertubeError::Api {
            status: response.status().to_string(),
            message: "Failed to retrieve YouTube Music client version".to_owned(),
        });
    }
    let body = response
        .text()
        .await
        .map_err(innertube_rs::InnertubeError::Network)?;
    parse_music_client_version(&body).ok_or_else(|| {
        innertube_rs::InnertubeError::Other(
            "YouTube Music service worker data has no client version".to_owned(),
        )
    })
}

fn parse_music_client_version(data: &str) -> Option<String> {
    data.as_bytes().windows(16).find_map(|candidate| {
        let valid = candidate[0] == b'1'
            && candidate[1] == b'.'
            && candidate[10] == b'.'
            && candidate[13] == b'.'
            && candidate
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 1 | 10 | 13) || byte.is_ascii_digit());
        valid.then(|| String::from_utf8_lossy(candidate).into_owned())
    })
}

fn build_music_stats_url(
    base_url: &str,
    client_version: &str,
    params: &[(&str, String)],
) -> innertube_rs::error::Result<String> {
    let music_url = if let Some(rest) = base_url.strip_prefix("https://s.youtube.com/") {
        format!("https://music.youtube.com/{rest}")
    } else if base_url.starts_with("https://music.youtube.com/") {
        base_url.to_owned()
    } else {
        return Err(innertube_rs::InnertubeError::Format(
            "Invalid YouTube Music stats host".to_owned(),
        ));
    };
    let Some((base, query)) = music_url.split_once('?') else {
        return Err(innertube_rs::InnertubeError::Format(
            "Invalid stats URL: missing query string".to_owned(),
        ));
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in query.split('&') {
        let Some((key, value)) = part.split_once('=') else {
            pairs.push((part.to_owned(), String::new()));
            continue;
        };
        pairs.push((key.to_owned(), value.to_owned()));
    }
    set_query_param(&mut pairs, "ver", "2".to_owned());
    set_query_param(&mut pairs, "c", "web_remix".to_owned());
    set_query_param(&mut pairs, "cbrver", client_version.to_owned());
    set_query_param(&mut pairs, "cver", client_version.to_owned());
    for (key, value) in params {
        set_query_param(&mut pairs, key, value.clone());
    }
    let query = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{base}?{query}"))
}

fn set_query_param(pairs: &mut Vec<(String, String)>, name: &str, value: String) {
    for (k, v) in &mut *pairs {
        if *k == name {
            *k = name.to_owned();
            *v = value;
            return;
        }
    }
    pairs.push((name.to_owned(), value));
}

fn extract_tracking(info: &innertube_rs::VideoInfo) -> Option<WatchTracking> {
    let tracking = info.player_response.playback_tracking.as_ref()?;
    Some(WatchTracking {
        cpn: info.cpn.clone(),
        playback_url: tracking
            .videostats_playback_url
            .as_ref()
            .and_then(|url| url.base_url.clone()),
        watchtime_url: tracking
            .videostats_watchtime_url
            .as_ref()
            .and_then(|url| url.base_url.clone()),
    })
}

fn up_next_queue(
    panel: innertube_rs::PlaylistPanelNode,
    current_video_id: &str,
    extras: Vec<PanelExtras>,
) -> UpNextQueue {
    let current_index = panel
        .items
        .iter()
        .position(|item| item.selected)
        .or_else(|| {
            panel
                .items
                .iter()
                .position(|item| item.id == current_video_id)
        })
        .unwrap_or_default();
    let tracks = panel
        .items
        .into_iter()
        .zip(extras)
        .map(|(item, extra)| UpNextTrack {
            video_id: item.id,
            title: item.title,
            artist: extra.artist.unwrap_or(item.author.unwrap_or_default()),
            album: extra.album,
            duration: item.duration,
            art_url: extra.art_url,
        })
        .collect();
    UpNextQueue {
        tracks,
        current_index,
    }
}

fn parse_panel_extras(panel: &Value) -> Vec<PanelExtras> {
    let Some(contents) = panel.get("contents").and_then(Value::as_array) else {
        return Vec::new();
    };
    contents
        .iter()
        .map(|item| {
            let renderer = item.get("playlistPanelVideoRenderer");
            let artist = renderer
                .and_then(|renderer| renderer.pointer("/longBylineText/runs/0/text"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let album = renderer
                .and_then(|renderer| renderer.pointer("/longBylineText/runs"))
                .and_then(Value::as_array)
                .and_then(|runs| {
                    runs.iter().find(|run| {
                        run.pointer(
                            "/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
                        )
                        .and_then(Value::as_str)
                        == Some("MUSIC_PAGE_TYPE_ALBUM")
                    })
                })
                .and_then(|run| run.get("text"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let art_url = renderer
                .and_then(|renderer| {
                    renderer
                        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
                        .or_else(|| {
                            renderer.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
                        })
                        .or_else(|| renderer.get("thumbnail").and_then(|t| t.get("thumbnails")))
                })
                .and_then(Value::as_array)
                .and_then(|list| {
                    list.iter()
                        .max_by_key(|thumbnail| thumbnail["width"].as_u64().unwrap_or(0))
                })
                .and_then(|thumbnail| thumbnail["url"].as_str())
                .map(str::to_owned);
            PanelExtras {
                artist,
                album,
                art_url,
            }
        })
        .collect()
}

fn find_playlist_panel(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => object
            .get("playlistPanelRenderer")
            .or_else(|| object.values().find_map(find_playlist_panel)),
        Value::Array(values) => values.iter().find_map(find_playlist_panel),
        _ => None,
    }
}

fn parse_search_top_result(value: &Value) -> Option<MusicSearchTopResult> {
    let card = find_music_card(value)?;
    let title = card.get("title").and_then(text_value)?;
    let subtitle_runs = card.pointer("/subtitle/runs").and_then(Value::as_array);
    let kind = subtitle_runs
        .and_then(|runs| runs.first())
        .and_then(|run| run.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("Top result")
        .to_owned();
    let artists = subtitle_runs
        .into_iter()
        .flatten()
        .filter(|run| {
            run.pointer(
                "/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
            )
            .and_then(Value::as_str)
                == Some("MUSIC_PAGE_TYPE_ARTIST")
        })
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    let detail = if artists.is_empty() {
        subtitle_runs
            .into_iter()
            .flatten()
            .filter_map(|run| run.get("text").and_then(Value::as_str))
            .filter(|text| {
                *text != kind
                    && !text.contains('•')
                    && !text
                        .split(':')
                        .all(|part| part.chars().all(|character| character.is_ascii_digit()))
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        artists
    };
    let video_id = card
        .pointer("/onTap/watchEndpoint/videoId")
        .or_else(|| card.pointer("/buttons/0/buttonRenderer/command/watchEndpoint/videoId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let browse_id = card
        .pointer("/onTap/browseEndpoint/browseId")
        .or_else(|| card.pointer("/buttons/0/buttonRenderer/command/browseEndpoint/browseId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let art_url = card
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|thumbnails| {
            thumbnails
                .iter()
                .max_by_key(|thumbnail| thumbnail["width"].as_u64().unwrap_or_default())
        })
        .and_then(|thumbnail| thumbnail.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(MusicSearchTopResult {
        kind,
        title,
        detail,
        video_id,
        browse_id,
        art_url,
    })
}

fn find_music_card(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => object
            .get("musicCardShelfRenderer")
            .or_else(|| object.values().find_map(find_music_card)),
        Value::Array(values) => values.iter().find_map(find_music_card),
        _ => None,
    }
}

fn parse_search_playlists(value: &Value) -> Vec<innertube_rs::MusicPlaylistItem> {
    let mut playlists = Vec::new();
    collect_search_playlists(value, &mut playlists);
    playlists
}

fn collect_search_playlists(value: &Value, playlists: &mut Vec<innertube_rs::MusicPlaylistItem>) {
    match value {
        Value::Object(object) => {
            if let Some(renderer) = object.get("musicResponsiveListItemRenderer")
                && let Some(playlist) = parse_search_playlist(renderer)
                && !playlists
                    .iter()
                    .any(|existing| existing.browse_id == playlist.browse_id)
            {
                playlists.push(playlist);
            }
            for child in object.values() {
                collect_search_playlists(child, playlists);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_search_playlists(child, playlists);
            }
        }
        _ => {}
    }
}

fn parse_search_playlist(renderer: &Value) -> Option<innertube_rs::MusicPlaylistItem> {
    let title_run =
        renderer.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0")?;
    let browse = renderer
        .pointer("/navigationEndpoint/browseEndpoint")
        .or_else(|| title_run.pointer("/navigationEndpoint/browseEndpoint"))?;
    let page_type = browse
        .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
        .and_then(Value::as_str);
    if page_type != Some("MUSIC_PAGE_TYPE_PLAYLIST") {
        return None;
    }
    let browse_id = browse.get("browseId").and_then(Value::as_str)?.to_owned();
    let title = title_run.get("text").and_then(Value::as_str)?.to_owned();
    let detail = renderer
        .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text")
        .and_then(text_value);
    let author = detail.as_deref().and_then(|detail| {
        detail
            .split('•')
            .map(str::trim)
            .find(|part| !part.is_empty() && *part != "Playlist" && !part.contains("songs"))
            .map(ToOwned::to_owned)
    });
    let thumbnail = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|thumbnails| {
            thumbnails
                .iter()
                .max_by_key(|thumbnail| thumbnail["width"].as_u64().unwrap_or_default())
        })
        .and_then(|thumbnail| thumbnail.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(innertube_rs::MusicPlaylistItem {
        browse_id,
        title,
        author,
        track_count: None,
        thumbnail,
    })
}

fn parse_account_identity(value: &Value) -> Option<AccountIdentity> {
    let account = find_account_item(value, true).or_else(|| find_account_item(value, false))?;
    let display_name = account.get("accountName").and_then(text_value)?;
    let username = account.get("channelHandle").and_then(text_value);
    Some(AccountIdentity {
        display_name,
        username,
    })
}

fn find_account_item(
    value: &Value,
    selected_only: bool,
) -> Option<&serde_json::Map<String, Value>> {
    match value {
        Value::Object(object) => {
            if let Some(account) = object.get("accountItemRenderer").and_then(Value::as_object)
                && (!selected_only
                    || account.get("isSelected").and_then(Value::as_bool) == Some(true))
            {
                return Some(account);
            }
            object
                .values()
                .find_map(|value| find_account_item(value, selected_only))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_account_item(value, selected_only)),
        _ => None,
    }
}

fn text_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("simpleText")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            value
                .get("runs")
                .and_then(Value::as_array)
                .map(|runs| {
                    runs.iter()
                        .filter_map(|run| run.get("text").and_then(Value::as_str))
                        .collect::<String>()
                })
                .filter(|text| !text.is_empty())
        })
        .filter(|text| !text.trim().is_empty())
}

fn find_like_count(value: &Value) -> Option<u64> {
    match value {
        Value::Object(object) => {
            if object.get("accessibilityId").and_then(Value::as_str) == Some("id.video.like.button")
                && let Some(count) = object
                    .get("accessibilityText")
                    .and_then(Value::as_str)
                    .and_then(parse_count)
            {
                return Some(count);
            }
            object.values().find_map(find_like_count)
        }
        Value::Array(values) => values.iter().find_map(find_like_count),
        _ => None,
    }
}

fn parse_count(value: &str) -> Option<u64> {
    let digits = value
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_playable_playlist_order() {
        let response = json!({
            "musicPlaylistShelfRenderer": {
                "contents": [
                    music_playlist_item(Some("first")),
                    music_playlist_item(None),
                    music_playlist_item(Some("last"))
                ],
                "continuations": [{
                    "nextContinuationData": { "continuation": "next-page" }
                }]
            }
        });
        let (tracks, continuation) = parse_music_playlist_page(&response);

        assert_eq!(
            tracks
                .iter()
                .map(|track| track.video_id.as_str())
                .collect::<Vec<_>>(),
            ["first", "last"]
        );
        assert_eq!(continuation.as_deref(), Some("next-page"));
    }

    fn music_playlist_item(video_id: Option<&str>) -> Value {
        json!({
            "musicResponsiveListItemRenderer": {
                "playlistItemData": { "videoId": video_id },
                "flexColumns": [
                    {
                        "musicResponsiveListItemFlexColumnRenderer": {
                            "text": { "runs": [{ "text": video_id.unwrap_or("Unavailable") }] }
                        }
                    },
                    {
                        "musicResponsiveListItemFlexColumnRenderer": {
                            "text": { "runs": [{
                                "text": "Artist",
                                "navigationEndpoint": { "browseEndpoint": { "browseId": "UCartist" } }
                            }] }
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn extracts_playlist_browse_id_instead_of_seed_video_id() {
        let response = json!({
            "musicResponsiveListItemRenderer": {
                "playlistItemData": { "videoId": "seed-video" },
                "navigationEndpoint": {
                    "browseEndpoint": {
                        "browseId": "VLPLfocus",
                        "browseEndpointContextSupportedConfigs": {
                            "browseEndpointContextMusicConfig": {
                                "pageType": "MUSIC_PAGE_TYPE_PLAYLIST"
                            }
                        }
                    }
                },
                "flexColumns": [
                    {
                        "musicResponsiveListItemFlexColumnRenderer": {
                            "text": { "runs": [{
                                "text": "Focus Mix"
                            }] }
                        }
                    },
                    {
                        "musicResponsiveListItemFlexColumnRenderer": {
                            "text": { "runs": [{ "text": "Playlist • YouTube Music • 50 songs" }] }
                        }
                    }
                ]
            }
        });

        let playlists = parse_search_playlists(&response);

        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].browse_id, "VLPLfocus");
        assert_eq!(playlists[0].title, "Focus Mix");
    }

    #[test]
    fn extracts_play_target_from_home_card() {
        let response = json!({
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [{
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [{
                                        "musicCarouselShelfRenderer": {
                                            "header": {
                                                "musicCarouselShelfBasicHeaderRenderer": {
                                                    "title": { "runs": [{ "text": "Listen again" }] }
                                                }
                                            },
                                            "contents": [{
                                                "musicTwoRowItemRenderer": {
                                                    "title": { "runs": [{ "text": "Wala Man Sa'yo Ang Lahat" }] },
                                                    "subtitle": { "runs": [
                                                        { "text": "Song" },
                                                        { "text": " • " },
                                                        { "text": "Myrus" }
                                                    ] },
                                                    "navigationEndpoint": {
                                                        "watchEndpoint": {
                                                            "videoId": "8i_VTKjtRkk",
                                                            "playlistId": "RDAMVM8i_VTKjtRkk"
                                                        }
                                                    },
                                                    "thumbnailRenderer": {
                                                        "musicThumbnailRenderer": {
                                                            "thumbnail": {
                                                                "thumbnails": [{ "url": "https://example.com/art.jpg" }]
                                                            }
                                                        }
                                                    },
                                                    "overlay": {
                                                        "musicItemThumbnailOverlayRenderer": {
                                                            "content": {
                                                                "musicPlayButtonRenderer": {
                                                                    "playNavigationEndpoint": {
                                                                        "watchEndpoint": {
                                                                            "videoId": "8i_VTKjtRkk",
                                                                            "playlistId": "RDAMVM8i_VTKjtRkk"
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }]
                                        }
                                    }]
                                }
                            }
                        }
                    }]
                }
            }
        });

        let feed = parse_home_response(&response).expect("home response should parse");
        let target = feed
            .play_targets
            .first()
            .expect("Listen Again card should be playable");

        assert_eq!(target.video_id.as_deref(), Some("8i_VTKjtRkk"));
        assert_eq!(target.playlist_id.as_deref(), Some("RDAMVM8i_VTKjtRkk"));
    }

    #[test]
    fn uses_home_cards_playlist_context_for_up_next() {
        let payload = up_next_payload("8i_VTKjtRkk", Some("RDAMVM8i_VTKjtRkk"));

        assert_eq!(payload["videoId"], "8i_VTKjtRkk");
        assert_eq!(payload["playlistId"], "RDAMVM8i_VTKjtRkk");
    }

    #[test]
    fn extracts_like_count_from_accessibility_text() {
        let response = json!({
            "buttonViewModel": {
                "accessibilityId": "id.video.like.button",
                "accessibilityText": "like this video along with 150,881 other people"
            }
        });

        assert_eq!(find_like_count(&response), Some(150_881));
    }

    #[test]
    fn extracts_selected_account_name_and_handle() {
        let response = json!({
            "contents": [
                {
                    "accountItemRenderer": {
                        "accountName": { "simpleText": "Other Account" },
                        "channelHandle": { "simpleText": "@other" },
                        "isSelected": false
                    }
                },
                {
                    "accountItemRenderer": {
                        "accountName": { "runs": [{ "text": "Display Name" }] },
                        "channelHandle": { "runs": [{ "text": "@username" }] },
                        "isSelected": true
                    }
                }
            ]
        });

        let identity = parse_account_identity(&response).expect("selected account should parse");
        assert_eq!(identity.display_name, "Display Name");
        assert_eq!(identity.username.as_deref(), Some("@username"));
    }

    #[test]
    fn accepts_account_without_handle() {
        let response = json!({
            "accountItemRenderer": {
                "accountName": { "simpleText": "Display Name" },
                "isSelected": true
            }
        });

        let identity = parse_account_identity(&response).expect("account should parse");
        assert_eq!(identity.display_name, "Display Name");
        assert_eq!(identity.username, None);
    }

    #[test]
    fn maps_automix_queue_and_selected_track() {
        let panel = innertube_rs::PlaylistPanelNode {
            title: "Up next".to_owned(),
            playlist_id: Some("RDAMVM".to_owned()),
            num_videos_text: Some("2 songs".to_owned()),
            items: vec![
                innertube_rs::PlaylistPanelVideoNode {
                    id: "first".to_owned(),
                    title: "First".to_owned(),
                    author: Some("Artist One".to_owned()),
                    duration: Some("3:10".to_owned()),
                    selected: false,
                },
                innertube_rs::PlaylistPanelVideoNode {
                    id: "current".to_owned(),
                    title: "Current".to_owned(),
                    author: Some("Artist Two".to_owned()),
                    duration: Some("4:20".to_owned()),
                    selected: true,
                },
            ],
        };

        let extras = vec![
            PanelExtras {
                artist: Some("Artist One".to_owned()),
                album: Some("Album One".to_owned()),
                art_url: Some("https://example.com/one.jpg".to_owned()),
            },
            PanelExtras {
                artist: Some("Artist Two".to_owned()),
                album: None,
                art_url: None,
            },
        ];

        let queue = up_next_queue(panel, "current", extras);
        assert_eq!(queue.current_index, 1);
        assert_eq!(queue.tracks.len(), 2);
        assert_eq!(queue.tracks[1].title, "Current");
        assert_eq!(queue.tracks[1].duration.as_deref(), Some("4:20"));
        assert_eq!(queue.tracks[0].artist, "Artist One");
        assert_eq!(queue.tracks[0].album.as_deref(), Some("Album One"));
        assert_eq!(
            queue.tracks[0].art_url.as_deref(),
            Some("https://example.com/one.jpg")
        );
    }

    #[test]
    fn builds_music_stats_url_with_live_client_identity() {
        let base = "https://s.youtube.com/api/stats/watchtime?cl=982807685&docid=dQw4w9WgXcQ&vm=signed&el=shorts";
        let url = build_music_stats_url(
            base,
            "1.20260915.14.00",
            &[
                ("cpn", "cpn-1234".to_owned()),
                ("vm", "replaced".to_owned()),
            ],
        )
        .expect("stats URL should be valid");

        assert!(url.starts_with("https://music.youtube.com/api/stats/watchtime?"));
        assert!(
            url.contains("&docid=dQw4w9WgXcQ"),
            "signed base params must survive"
        );
        assert!(
            url.contains("&vm=replaced"),
            "existing param should be overwritten in place"
        );
        assert!(
            url.contains("&el=shorts"),
            "base params should be preserved"
        );
        assert!(url.contains("&ver=2"), "ver should be added");
        assert!(url.contains("&c=web_remix"), "Music client should be set");
        assert!(
            url.contains("&cbrver=1.20260915.14.00"),
            "browser version should use the live Music version"
        );
        assert!(
            url.contains("&cver=1.20260915.14.00"),
            "client version should use the live Music version"
        );
        assert!(url.contains("&cpn=cpn-1234"), "cpn should be added");
        assert_eq!(
            url.chars().filter(|c| *c == '?').count(),
            1,
            "exactly one query separator"
        );
    }

    #[test]
    fn extracts_music_client_version_from_service_worker_data() {
        let data = r#")]}'\n[[[\"foo\",\"1.20260915.14.00\",\"bar\"]]]"#;

        assert_eq!(
            parse_music_client_version(data).as_deref(),
            Some("1.20260915.14.00")
        );
        assert_eq!(parse_music_client_version("no version here"), None);
    }

    #[test]
    fn extracts_playable_top_result_from_music_card() {
        let response = json!({
            "contents": {
                "musicCardShelfRenderer": {
                    "thumbnail": {
                        "musicThumbnailRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    { "url": "https://example.com/small.jpg" },
                                    { "url": "https://example.com/large.jpg" }
                                ]
                            }
                        }
                    },
                    "title": { "runs": [{ "text": "Ganda Mo" }] },
                    "subtitle": {
                        "runs": [
                            { "text": "Song" },
                            { "text": " • " },
                            {
                                "text": "Cue C",
                                "navigationEndpoint": {
                                    "browseEndpoint": {
                                        "browseId": "UCartist",
                                        "browseEndpointContextSupportedConfigs": {
                                            "browseEndpointContextMusicConfig": {
                                                "pageType": "MUSIC_PAGE_TYPE_ARTIST"
                                            }
                                        }
                                    }
                                }
                            },
                            { "text": " • " },
                            { "text": "4:46" }
                        ]
                    },
                    "onTap": { "watchEndpoint": { "videoId": "0RloTSfzlyo" } }
                }
            }
        });

        let top = parse_search_top_result(&response).expect("top result should parse");

        assert_eq!(top.kind, "Song");
        assert_eq!(top.title, "Ganda Mo");
        assert_eq!(top.detail, "Cue C");
        assert_eq!(top.video_id.as_deref(), Some("0RloTSfzlyo"));
        assert_eq!(
            top.art_url.as_deref(),
            Some("https://example.com/large.jpg")
        );
    }

    #[test]
    fn extracts_watch_tracking_from_player_response() {
        let info = innertube_rs::VideoInfo {
            player_response: innertube_rs::PlayerResponse {
                playability_status: innertube_rs::PlayabilityStatus {
                    status: "OK".to_owned(),
                    reason: None,
                    playable_in_embed: Some(true),
                },
                video_details: None,
                streaming_data: None,
                captions: None,
                playback_tracking: Some(innertube_rs::models::video::PlaybackTracking {
                    videostats_watchtime_url: Some(innertube_rs::models::video::TrackingUrl {
                        base_url: Some(
                            "https://s.youtube.com/api/stats/watchtime?key=abc".to_owned(),
                        ),
                    }),
                    videostats_playback_url: Some(innertube_rs::models::video::TrackingUrl {
                        base_url: Some(
                            "https://s.youtube.com/api/stats/playback?key=abc".to_owned(),
                        ),
                    }),
                }),
            },
            watch_next: None,
            cpn: "cpn-1234".to_owned(),
            po_token: None,
        };

        let tracking = extract_tracking(&info).expect("tracking should be extracted");
        assert_eq!(tracking.cpn, "cpn-1234");
        assert_eq!(
            tracking.playback_url.as_deref(),
            Some("https://s.youtube.com/api/stats/playback?key=abc")
        );
        assert_eq!(
            tracking.watchtime_url.as_deref(),
            Some("https://s.youtube.com/api/stats/watchtime?key=abc")
        );
    }

    #[test]
    fn returns_none_when_player_response_lacks_tracking() {
        let info = innertube_rs::VideoInfo {
            player_response: innertube_rs::PlayerResponse {
                playability_status: innertube_rs::PlayabilityStatus {
                    status: "OK".to_owned(),
                    reason: None,
                    playable_in_embed: Some(true),
                },
                video_details: None,
                streaming_data: None,
                captions: None,
                playback_tracking: None,
            },
            watch_next: None,
            cpn: "cpn-1234".to_owned(),
            po_token: None,
        };

        assert_eq!(extract_tracking(&info), None);
    }

    #[test]
    fn extracts_square_art_and_album_from_panel_item() {
        let panel = json!({
            "contents": [
                {
                    "playlistPanelVideoRenderer": {
                        "videoId": "abc",
                        "longBylineText": {
                            "runs": [
                                { "text": "Neon Sage" },
                                { "text": " • " },
                                {
                                    "text": "Dyosa",
                                    "navigationEndpoint": {
                                        "browseEndpoint": {
                                            "browseId": "MPREb_aUeEJBruyDC",
                                            "browseEndpointContextSupportedConfigs": {
                                                "browseEndpointContextMusicConfig": {
                                                    "pageType": "MUSIC_PAGE_TYPE_ALBUM"
                                                }
                                            }
                                        }
                                    }
                                },
                                { "text": " • " },
                                { "text": "2024" }
                            ]
                        },
                        "thumbnail": {
                            "musicThumbnailRenderer": {
                                "thumbnail": {
                                    "thumbnails": [
                                        { "url": "https://example.com/small.jpg", "width": 120, "height": 120 },
                                        { "url": "https://example.com/large.jpg", "width": 544, "height": 544 }
                                    ]
                                }
                            }
                        }
                    }
                }
            ]
        });

        let extras = parse_panel_extras(&panel);
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].artist.as_deref(), Some("Neon Sage"));
        assert_eq!(extras[0].album.as_deref(), Some("Dyosa"));
        assert_eq!(
            extras[0].art_url.as_deref(),
            Some("https://example.com/large.jpg")
        );
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn loads_search_playlist_and_final_track_automix() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let results = client
            .search("lofi hip hop playlist")
            .await
            .expect("playlist search should load");
        let playlist = results
            .sections
            .playlists
            .first()
            .expect("search should return a playlist");
        let playback = client
            .get_playlist(&playlist.browse_id)
            .await
            .expect("playlist should load");
        let final_track = playback.tracks.last().expect("playlist should have tracks");
        let automix = client
            .get_up_next(&final_track.video_id, None)
            .await
            .expect("final track Automix should load");

        assert!(!playback.tracks[0].video_id.is_empty());
        assert_eq!(
            automix.tracks[automix.current_index].video_id,
            final_track.video_id
        );
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn preserves_play_targets_from_personalized_home() {
        let credentials = crate::config::Credentials::load()
            .expect("credentials should load")
            .expect("saved credentials are required");
        let client = YTMusic::new(Some(credentials.cookie().to_owned()))
            .await
            .expect("authenticated client should initialize");
        let feed = client.get_home().await.expect("home feed should load");
        let target = feed
            .play_targets
            .iter()
            .find(|target| target.title == "Wala Man Sa'yo Ang Lahat")
            .expect("Listen Again card should retain its play endpoint");

        assert_eq!(target.video_id.as_deref(), Some("8i_VTKjtRkk"));
        assert_eq!(target.playlist_id.as_deref(), Some("RDAMVM8i_VTKjtRkk"));

        let queue = client
            .get_up_next(
                target.video_id.as_deref().expect("card should have a seed"),
                target.playlist_id.as_deref(),
            )
            .await
            .expect("card queue should load");
        assert_eq!(queue.tracks[queue.current_index].video_id, "8i_VTKjtRkk");
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn loads_automix_without_authentication() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let queue = client
            .get_up_next("dQw4w9WgXcQ", None)
            .await
            .expect("anonymous Automix should load");

        assert!(queue.tracks.len() > 1);
        assert_eq!(queue.tracks[queue.current_index].video_id, "dQw4w9WgXcQ");
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn includes_main_music_search_result() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let results = client.search("ganda mo").await.expect("search should load");
        let top = results.top_result.expect("main result should parse");

        assert_eq!(top.title, "Ganda Mo");
        assert_eq!(top.video_id.as_deref(), Some("0RloTSfzlyo"));
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn extracts_square_art_and_album_from_live_automix() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let queue = client
            .get_up_next("LG5E5zeeWng", None)
            .await
            .expect("anonymous Automix should load");

        let current = &queue.tracks[queue.current_index];
        assert_eq!(current.video_id, "LG5E5zeeWng");
        assert_eq!(current.artist, "Neon Sage");
        assert_eq!(current.album.as_deref(), Some("Dyosa"));
        let art_url = current
            .art_url
            .as_deref()
            .expect("music art should be present");
        assert!(
            art_url.contains("yt3.googleusercontent.com"),
            "expected square yt3 art, got {art_url}"
        );
        assert!(
            art_url.contains("=w"),
            "expected sized square art, got {art_url}"
        );
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn extracts_album_after_artist_navigation_run() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let queue = client
            .get_up_next("dHdMAdh4Xgc", None)
            .await
            .expect("anonymous Automix should load");

        let current = &queue.tracks[queue.current_index];
        assert_eq!(current.video_id, "dHdMAdh4Xgc");
        assert_eq!(current.artist, "Clean Bandit");
        assert_eq!(current.album.as_deref(), Some("New Eyes"));
    }
}
