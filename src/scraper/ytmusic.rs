use innertube_rs::{Innertube, MusicHomeFeed, MusicSearchResults};

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
}
