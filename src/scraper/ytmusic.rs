use innertube_rs::{
    FormatFilter, FormatType, GetVideoInfoOptions, Innertube, MusicHomeFeed, MusicSearchResults,
    QualityPreference,
};
use serde_json::{Value, json};

pub struct AudioStreamInfo {
    pub url: String,
    pub views: Option<u64>,
    pub likes: Option<u64>,
}

#[derive(Clone)]
pub struct YTMusic {
    yt: Innertube,
}

impl YTMusic {
    pub async fn new() -> innertube_rs::error::Result<Self> {
        let yt = Innertube::new().await?;
        Ok(Self { yt })
    }

    pub async fn get_home(&self) -> innertube_rs::error::Result<MusicHomeFeed> {
        self.yt.music().get_home().await
    }

    pub async fn search(&self, query: &str) -> innertube_rs::error::Result<MusicSearchResults> {
        self.yt.music().search(query, None).await
    }

    pub async fn get_audio_url(&self, video_id: &str) -> innertube_rs::error::Result<String> {
        let info = self
            .yt
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
            &self.yt.player.decipherer,
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
        let info_request = self.yt.get_basic_info(video_id, Some(&options));
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
            &self.yt.player.decipherer,
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
}
