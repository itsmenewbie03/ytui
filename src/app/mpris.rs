use super::model::{PlaybackState, PlaybackStatus};
use mpris_server::{Metadata, PlaybackStatus as MprisPlaybackStatus, Player, Time, TrackId};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::{runtime, sync::mpsc as tokio_mpsc, task::LocalSet};

const ARTWORK_DIR: &str = "/tmp/ytui";

#[derive(Debug)]
pub(super) enum MprisCommand {
    Next,
    Previous,
    Pause,
    PlayPause,
    Stop,
    Play,
    Seek(i64),
    SetPosition { track_id: String, position: i64 },
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MprisTrack {
    id: String,
    video_id: String,
    title: String,
    artist: String,
    album: Option<String>,
    length: i64,
    art_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MprisSnapshot {
    status: MprisPlaybackStatus,
    track: Option<MprisTrack>,
    position: i64,
    can_go_next: bool,
    can_go_previous: bool,
    can_play: bool,
    can_pause: bool,
    can_seek: bool,
}

impl MprisSnapshot {
    pub(super) fn from_playback(playback: &PlaybackState, has_player: bool) -> Self {
        let track = playback.track.as_ref().map(|track| MprisTrack {
            id: track_id(&track.video_id),
            video_id: track.video_id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            length: seconds_to_micros(playback.duration),
            art_url: track.art_url.clone(),
        });
        let queue_index = playback.queue_index;
        Self {
            status: match playback.status {
                PlaybackStatus::Resolving | PlaybackStatus::Loading | PlaybackStatus::Playing => {
                    MprisPlaybackStatus::Playing
                }
                PlaybackStatus::Paused => MprisPlaybackStatus::Paused,
                PlaybackStatus::Idle | PlaybackStatus::Stopped | PlaybackStatus::Error => {
                    MprisPlaybackStatus::Stopped
                }
            },
            position: seconds_to_micros(playback.position),
            can_go_next: queue_index.is_some_and(|index| index + 1 < playback.queue.len()),
            can_go_previous: queue_index.is_some_and(|index| index > 0),
            can_play: track.is_some(),
            can_pause: has_player,
            can_seek: has_player && playback.duration > 0.0,
            track,
        }
    }

    pub(super) fn track_id(&self) -> Option<&str> {
        self.track.as_ref().map(|track| track.id.as_str())
    }
}

enum MprisUpdate {
    Snapshot(MprisSnapshot),
    Seeked(i64),
}

pub(super) struct MprisService {
    updates: Option<tokio_mpsc::UnboundedSender<MprisUpdate>>,
    commands: Receiver<MprisCommand>,
    thread: Option<JoinHandle<()>>,
    stopped: Receiver<()>,
}

impl MprisService {
    pub(super) fn start() -> Self {
        let (update_sender, update_receiver) = tokio_mpsc::unbounded_channel();
        let (command_sender, command_receiver) = mpsc::channel();
        let (stopped_sender, stopped_receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            run_service(update_receiver, command_sender);
            let _ = stopped_sender.send(());
        });
        Self {
            updates: Some(update_sender),
            commands: command_receiver,
            thread: Some(thread),
            stopped: stopped_receiver,
        }
    }

    pub(super) fn publish(&self, snapshot: MprisSnapshot) {
        if let Some(updates) = &self.updates {
            let _ = updates.send(MprisUpdate::Snapshot(snapshot));
        }
    }

    pub(super) fn seeked(&self, position: f64) {
        if let Some(updates) = &self.updates {
            let _ = updates.send(MprisUpdate::Seeked(seconds_to_micros(position)));
        }
    }

    pub(super) fn try_recv(&self) -> Result<MprisCommand, TryRecvError> {
        self.commands.try_recv()
    }
}

impl Drop for MprisService {
    fn drop(&mut self) {
        self.updates.take();
        let Some(thread) = self.thread.take() else {
            return;
        };
        match self.stopped.recv_timeout(Duration::from_millis(250)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn run_service(
    updates: tokio_mpsc::UnboundedReceiver<MprisUpdate>,
    commands: mpsc::Sender<MprisCommand>,
) {
    let Ok(runtime) = runtime::Builder::new_current_thread().enable_all().build() else {
        return;
    };
    LocalSet::new().block_on(&runtime, serve(updates, commands));
}

async fn serve(
    mut updates: tokio_mpsc::UnboundedReceiver<MprisUpdate>,
    commands: mpsc::Sender<MprisCommand>,
) {
    let Ok(player) = Player::builder(&format!("ytui.instance{}", std::process::id()))
        .identity("ytui")
        .can_quit(true)
        .can_control(true)
        .build()
        .await
    else {
        return;
    };
    connect_commands(&player, commands);
    tokio::task::spawn_local(player.run());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok();
    let mut previous: Option<MprisSnapshot> = None;
    while let Some(update) = updates.recv().await {
        let result = match update {
            MprisUpdate::Snapshot(mut snapshot) => {
                let artwork_changed = previous.as_ref().and_then(|previous| {
                    previous
                        .track
                        .as_ref()
                        .and_then(|track| track.art_url.as_deref())
                }) != snapshot
                    .track
                    .as_ref()
                    .and_then(|track| track.art_url.as_deref());
                if artwork_changed
                    && let Some(url) = snapshot
                        .track
                        .as_mut()
                        .and_then(|track| track.art_url.as_mut())
                    && let Some(client) = &client
                    && let Some(local) = cache_artwork(client, url).await
                {
                    *url = local;
                }
                let result = apply_snapshot(&player, &snapshot, previous.as_ref()).await;
                previous = Some(snapshot);
                result
            }
            MprisUpdate::Seeked(position) => {
                let position = Time::from_micros(position);
                player.set_position(position);
                player.seeked(position).await
            }
        };
        if result.is_err() {
            return;
        }
    }
}

fn connect_commands(player: &Player, commands: mpsc::Sender<MprisCommand>) {
    let sender = commands.clone();
    player.connect_next(move |_| {
        let _ = sender.send(MprisCommand::Next);
    });
    let sender = commands.clone();
    player.connect_previous(move |_| {
        let _ = sender.send(MprisCommand::Previous);
    });
    let sender = commands.clone();
    player.connect_pause(move |_| {
        let _ = sender.send(MprisCommand::Pause);
    });
    let sender = commands.clone();
    player.connect_play_pause(move |_| {
        let _ = sender.send(MprisCommand::PlayPause);
    });
    let sender = commands.clone();
    player.connect_stop(move |_| {
        let _ = sender.send(MprisCommand::Stop);
    });
    let sender = commands.clone();
    player.connect_play(move |_| {
        let _ = sender.send(MprisCommand::Play);
    });
    let sender = commands.clone();
    player.connect_seek(move |_, offset| {
        let _ = sender.send(MprisCommand::Seek(offset.as_micros()));
    });
    let sender = commands.clone();
    player.connect_set_position(move |_, track_id, position| {
        let _ = sender.send(MprisCommand::SetPosition {
            track_id: track_id.to_string(),
            position: position.as_micros(),
        });
    });
    player.connect_quit(move |_| {
        let _ = commands.send(MprisCommand::Quit);
    });
}

async fn apply_snapshot(
    player: &Player,
    snapshot: &MprisSnapshot,
    previous: Option<&MprisSnapshot>,
) -> mpris_server::zbus::Result<()> {
    if previous.is_none_or(|previous| previous.status != snapshot.status) {
        player.set_playback_status(snapshot.status).await?;
    }
    if previous.is_none_or(|previous| previous.track != snapshot.track) {
        player
            .set_metadata(metadata(snapshot.track.as_ref()))
            .await?;
    }
    if previous.is_none_or(|previous| previous.can_go_next != snapshot.can_go_next) {
        player.set_can_go_next(snapshot.can_go_next).await?;
    }
    if previous.is_none_or(|previous| previous.can_go_previous != snapshot.can_go_previous) {
        player.set_can_go_previous(snapshot.can_go_previous).await?;
    }
    if previous.is_none_or(|previous| previous.can_play != snapshot.can_play) {
        player.set_can_play(snapshot.can_play).await?;
    }
    if previous.is_none_or(|previous| previous.can_pause != snapshot.can_pause) {
        player.set_can_pause(snapshot.can_pause).await?;
    }
    if previous.is_none_or(|previous| previous.can_seek != snapshot.can_seek) {
        player.set_can_seek(snapshot.can_seek).await?;
    }
    player.set_position(Time::from_micros(snapshot.position));
    Ok(())
}

fn metadata(track: Option<&MprisTrack>) -> Metadata {
    let Some(track) = track else {
        return Metadata::builder().trackid(TrackId::NO_TRACK).build();
    };
    let track_id = TrackId::try_from(track.id.as_str()).unwrap_or(TrackId::NO_TRACK);
    let mut metadata = Metadata::builder()
        .trackid(track_id)
        .title(track.title.clone())
        .artist([track.artist.clone()])
        .url(format!(
            "https://music.youtube.com/watch?v={}",
            track.video_id
        ));
    if let Some(album) = &track.album {
        metadata = metadata.album(album.clone());
    }
    if let Some(art_url) = &track.art_url {
        metadata = metadata.art_url(art_url.clone());
    }
    if track.length > 0 {
        metadata = metadata.length(Time::from_micros(track.length));
    }
    metadata.build()
}

fn seconds_to_micros(seconds: f64) -> i64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * 1_000_000.0).min(i64::MAX as f64) as i64
}

async fn cache_artwork(client: &reqwest::Client, url: &str) -> Option<String> {
    if url.starts_with("file://") {
        return Some(url.to_owned());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    let path = artwork_cache_path(url);
    if path.exists() {
        return Some(file_url(&path));
    }
    let response = client.get(url).send().await.ok()?;
    let bytes = response.bytes().await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let _ = std::fs::create_dir_all(ARTWORK_DIR);
    if tokio::fs::write(&path, bytes).await.is_err() {
        return None;
    }
    Some(file_url(&path))
}

fn artwork_cache_path(url: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    PathBuf::from(ARTWORK_DIR).join(format!("{:016x}.jpg", hasher.finish()))
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn track_id(video_id: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = "/io/github/itsmenewbie03/ytui/track/".to_owned();
    for byte in video_id.bytes() {
        if byte.is_ascii_alphanumeric() {
            id.push(char::from(byte));
        } else {
            id.push('_');
            id.push(char::from(HEX[usize::from(byte >> 4)]));
            id.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    if video_id.is_empty() {
        id.push_str("unknown");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::PlaybackTrack;
    use std::time::Duration;

    #[test]
    fn creates_valid_collision_resistant_track_paths() {
        let dashed = track_id("abc-def");
        let underscored = track_id("abc_def");

        assert!(TrackId::try_from(dashed.as_str()).is_ok());
        assert!(TrackId::try_from(underscored.as_str()).is_ok());
        assert_ne!(dashed, underscored);
    }

    #[test]
    fn normalizes_invalid_playback_times() {
        assert_eq!(seconds_to_micros(f64::NAN), 0);
        assert_eq!(seconds_to_micros(-1.0), 0);
        assert_eq!(seconds_to_micros(1.25), 1_250_000);
    }

    #[test]
    fn exposes_playback_metadata_and_queue_capabilities() {
        let first = PlaybackTrack {
            video_id: "first-id".to_owned(),
            title: "First".to_owned(),
            artist: "Artist".to_owned(),
            album: Some("Album".to_owned()),
            art_url: Some("https://example.com/art.jpg".to_owned()),
            duration: Some("3:00".to_owned()),
            views: None,
            likes: None,
        };
        let second = PlaybackTrack {
            video_id: "second-id".to_owned(),
            title: "Second".to_owned(),
            artist: "Artist".to_owned(),
            album: None,
            art_url: None,
            duration: Some("4:00".to_owned()),
            views: None,
            likes: None,
        };
        let playback = PlaybackState {
            queue: vec![first.clone(), second],
            queue_index: Some(0),
            track: Some(first),
            status: PlaybackStatus::Playing,
            position: 1.25,
            duration: 180.0,
            ..Default::default()
        };

        let snapshot = MprisSnapshot::from_playback(&playback, true);

        assert_eq!(snapshot.status, MprisPlaybackStatus::Playing);
        assert_eq!(snapshot.position, 1_250_000);
        assert!(snapshot.can_go_next);
        assert!(!snapshot.can_go_previous);
        assert!(snapshot.can_pause);
        assert!(snapshot.can_seek);
        assert_eq!(
            snapshot.track.as_ref().map(|track| track.length),
            Some(180_000_000)
        );
        assert_eq!(
            snapshot
                .track
                .as_ref()
                .and_then(|track| track.art_url.as_deref()),
            Some("https://example.com/art.jpg")
        );
    }

    #[test]
    fn maps_artwork_to_a_local_cache_file() {
        let url = "https://example.com/art.jpg";
        let path = artwork_cache_path(url);

        assert!(path.starts_with(ARTWORK_DIR));
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("jpg")
        );
        assert_eq!(artwork_cache_path(url), path);
        assert_ne!(artwork_cache_path("https://example.com/other.jpg"), path);
        assert!(file_url(&path).starts_with("file:///tmp/ytui/"));
    }

    #[test]
    #[ignore = "requires dbus-run-session and playerctl"]
    fn registers_with_a_session_bus() {
        let service = MprisService::start();
        let art_url = "https://example.com/art.jpg".to_owned();
        let cached_path = artwork_cache_path(&art_url);
        std::fs::create_dir_all(ARTWORK_DIR).expect("cache directory should be creatable");
        std::fs::write(&cached_path, b"fake-artwork").expect("seeded artwork should be writable");
        let expected_file_url = file_url(&cached_path);
        let playback = PlaybackState {
            track: Some(PlaybackTrack {
                video_id: "test-id".to_owned(),
                title: "Test Track".to_owned(),
                artist: "Test Artist".to_owned(),
                album: Some("Test Album".to_owned()),
                art_url: Some(art_url),
                duration: Some("1:00".to_owned()),
                views: None,
                likes: None,
            }),
            status: PlaybackStatus::Paused,
            duration: 60.0,
            ..Default::default()
        };
        service.publish(MprisSnapshot::from_playback(&playback, true));
        let player_name = (0..20).find_map(|_| {
            let output = std::process::Command::new("playerctl")
                .arg("--list-all")
                .output()
                .expect("playerctl should be available");
            let players = String::from_utf8_lossy(&output.stdout);
            if let Some(player) = players.lines().find(|player| player.starts_with("ytui")) {
                return Some(player.to_owned());
            }
            std::thread::sleep(Duration::from_millis(50));
            None
        });
        let player_name = player_name.expect("ytui MPRIS service should register");

        let artwork_present = (0..20).any(|_| {
            let output = std::process::Command::new("playerctl")
                .args(["--player", &player_name, "metadata", "mpris:artUrl"])
                .output()
                .expect("playerctl should read metadata");
            let metadata = String::from_utf8_lossy(&output.stdout);
            if metadata.trim() == expected_file_url {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            false
        });
        assert!(artwork_present, "ytui should publish a local mpris:artUrl");

        let album_present = (0..20).any(|_| {
            let output = std::process::Command::new("playerctl")
                .args(["--player", &player_name, "metadata", "xesam:album"])
                .output()
                .expect("playerctl should read metadata");
            let metadata = String::from_utf8_lossy(&output.stdout);
            if metadata.trim() == "Test Album" {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            false
        });
        assert!(album_present, "ytui should publish xesam:album");

        let command_sent = (0..20).any(|_| {
            let status = std::process::Command::new("playerctl")
                .args(["--player", &player_name, "play-pause"])
                .status()
                .expect("playerctl should send a command");
            if status.success() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
            false
        });
        assert!(command_sent, "playerctl should accept the command");
        for _ in 0..20 {
            if matches!(service.try_recv(), Ok(MprisCommand::PlayPause)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("ytui did not receive the MPRIS command");
    }
}
