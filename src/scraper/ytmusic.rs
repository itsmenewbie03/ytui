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
}
