use innertube_rs::{FormatFilter, FormatType, GetVideoInfoOptions, Innertube, QualityPreference};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let video_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "dQw4w9WgXcQ".to_owned());
    let client = std::env::args().nth(2);
    let yt = Innertube::new().await?;

    let info = if let Some(client) = client {
        println!("Client: {client}");
        yt.get_basic_info(
            &video_id,
            Some(&GetVideoInfoOptions {
                client: Some(client),
                ..Default::default()
            }),
        )
        .await?
    } else {
        yt.get_basic_info(&video_id, None).await?
    };
    if let Some(details) = &info.player_response.video_details {
        println!("Title: {}", details.title);
        println!("Author: {}", details.author);
        println!("Duration: {}s", details.length_seconds);
    }

    let filter = FormatFilter {
        format_type: FormatType::AudioOnly,
        quality: QualityPreference::Highest,
        container: None,
    };

    let stream_url = info.get_stream_url(&filter, &yt.player.decipherer)?;
    println!("Stream URL: {stream_url}");

    Ok(())
}
