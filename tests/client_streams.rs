use innertube_rs::{
    FormatFilter, FormatType, GetVideoInfoOptions, Innertube, QualityPreference,
    constants::SUPPORTED_CLIENTS,
};
use std::process::{Command, Stdio};

const VIDEO_ID: &str = "lbaVbgbeKaI";
#[tokio::test]
#[ignore = "live network and mpv compatibility probe"]
async fn reports_working_stream_clients() -> Result<(), Box<dyn std::error::Error>> {
    let yt = Innertube::new().await?;
    let filter = FormatFilter {
        format_type: FormatType::AudioOnly,
        quality: QualityPreference::Highest,
        container: None,
    };
    let mut working = Vec::new();

    for client in SUPPORTED_CLIENTS {
        let info = match yt
            .get_basic_info(
                VIDEO_ID,
                Some(&GetVideoInfoOptions {
                    client: Some((*client).to_owned()),
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(info) => info,
            Err(error) => {
                println!("{client:<12} RESOLVE FAILED: {error}");
                continue;
            }
        };
        let url = match info.get_stream_url(&filter, &yt.player.decipherer) {
            Ok(url) => url,
            Err(error) => {
                println!("{client:<12} FORMAT FAILED: {error}");
                continue;
            }
        };

        if mpv_can_play(&url)? {
            println!("{client:<12} WORKS");
            working.push(*client);
        } else {
            println!("{client:<12} HTTP/PLAYBACK FAILED");
        }
    }

    println!("Working clients: {}", working.join(", "));
    assert!(
        !working.is_empty(),
        "no tested client produced a playable stream"
    );
    Ok(())
}

fn mpv_can_play(url: &str) -> std::io::Result<bool> {
    Command::new("mpv")
        .args([
            "--no-config",
            "--no-video",
            "--ao=null",
            "--length=1",
            "--network-timeout=10",
            "--really-quiet",
        ])
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
}
