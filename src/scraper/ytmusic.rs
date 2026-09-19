use innertube_rs::{
    FormatFilter, FormatType, GetVideoInfoOptions, Innertube, MusicHomeFeed, MusicSearchResults,
    QualityPreference,
};

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
}
