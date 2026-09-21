use innertube_rs::{
    FormatFilter, FormatType, GetVideoInfoOptions, Innertube, MusicHomeFeed, MusicSearchResults,
    QualityPreference, SessionOptions,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct AudioStreamInfo {
    pub url: String,
    pub views: Option<u64>,
    pub likes: Option<u64>,
}

pub struct AccountIdentity {
    pub display_name: String,
    pub username: Option<String>,
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

struct PanelExtras {
    artist: Option<String>,
    album: Option<String>,
    art_url: Option<String>,
}

#[derive(Clone)]
pub struct YTMusic {
    yt: Innertube,
    playback: Innertube,
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
        Ok(Self { yt, playback })
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

    pub async fn get_home(&self) -> innertube_rs::error::Result<MusicHomeFeed> {
        self.yt.music().get_home().await
    }

    pub async fn search(&self, query: &str) -> innertube_rs::error::Result<MusicSearchResults> {
        self.yt.music().search(query, None).await
    }

    pub async fn get_up_next(&self, video_id: &str) -> innertube_rs::error::Result<UpNextQueue> {
        let response = self
            .yt
            .session
            .post_innertube_client(
                "YTMUSIC",
                "/next",
                json!({
                    "videoId": video_id,
                    "playlistId": format!("RDAMVM{video_id}"),
                    "enablePersistentPlaylistPanel": true,
                    "isAudioOnly": true,
                    "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
                }),
            )
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
    ) -> innertube_rs::error::Result<AudioStreamInfo> {
        let options = GetVideoInfoOptions {
            client: Some("VISIONOS".to_owned()),
            ..Default::default()
        };
        let info_request = self.playback.get_basic_info(video_id, Some(&options));
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
            &self.playback.player.decipherer,
        )?;
        Ok(AudioStreamInfo {
            url,
            views,
            likes: likes.unwrap_or_default(),
        })
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
    async fn loads_automix_without_authentication() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let queue = client
            .get_up_next("dQw4w9WgXcQ")
            .await
            .expect("anonymous Automix should load");

        assert!(queue.tracks.len() > 1);
        assert_eq!(queue.tracks[queue.current_index].video_id, "dQw4w9WgXcQ");
    }

    #[tokio::test]
    #[ignore = "live YouTube Music compatibility probe"]
    async fn extracts_square_art_and_album_from_live_automix() {
        let client = YTMusic::new(None)
            .await
            .expect("anonymous client should initialize");
        let queue = client
            .get_up_next("LG5E5zeeWng")
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
            .get_up_next("dHdMAdh4Xgc")
            .await
            .expect("anonymous Automix should load");

        let current = &queue.tracks[queue.current_index];
        assert_eq!(current.video_id, "dHdMAdh4Xgc");
        assert_eq!(current.artist, "Clean Bandit");
        assert_eq!(current.album.as_deref(), Some("New Eyes"));
    }
}
