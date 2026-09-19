use innertube_rs::{MusicHomeFeed, MusicSearchResults};
use ratatui::style::Color;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Nav,
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Screen {
    Main,
    Player,
}

pub(super) enum HomeEntry {
    Track {
        video_id: String,
        title: String,
        artist: String,
    },
    Album {
        browse_id: String,
        title: String,
        artist: String,
    },
    Playlist {
        browse_id: String,
        title: String,
        author: String,
    },
}

impl HomeEntry {
    pub(super) fn title(&self) -> &str {
        match self {
            Self::Track { title, .. }
            | Self::Album { title, .. }
            | Self::Playlist { title, .. } => title,
        }
    }

    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Track { .. } => "Song",
            Self::Album { .. } => "Album",
            Self::Playlist { .. } => "Playlist",
        }
    }

    pub(super) fn detail(&self) -> &str {
        match self {
            Self::Track { artist, .. } | Self::Album { artist, .. } => artist,
            Self::Playlist { author, .. } => author,
        }
    }

    pub(super) fn is_playable(&self) -> bool {
        matches!(self, Self::Track { .. })
    }

    pub(super) fn playback_track(&self) -> Option<PlaybackTrack> {
        let Self::Track {
            video_id,
            title,
            artist,
        } = self
        else {
            return None;
        };
        Some(PlaybackTrack {
            video_id: video_id.clone(),
            title: title.clone(),
            artist: artist.clone(),
        })
    }

    pub(super) fn browse_message(&self) -> String {
        match self {
            Self::Album {
                browse_id, title, ..
            } => format!("Album browsing planned: {title} ({browse_id})"),
            Self::Playlist {
                browse_id, title, ..
            } => format!("Playlist browsing planned: {title} ({browse_id})"),
            Self::Track { .. } => "Track is playable.".to_owned(),
        }
    }
}

pub(super) struct HomeShelf {
    pub(super) title: String,
    pub(super) items: Vec<HomeEntry>,
}

pub(super) struct SearchItem {
    pub(super) kind: &'static str,
    pub(super) title: String,
    pub(super) detail: String,
    pub(super) video_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum QueueSource {
    Home(usize),
    Search,
}

pub(super) struct PlaybackTrack {
    pub(super) video_id: String,
    pub(super) title: String,
    pub(super) artist: String,
}

#[derive(Default)]
pub(super) struct PlaybackState {
    pub(super) source: Option<QueueSource>,
    pub(super) current_index: Option<usize>,
    pub(super) track: Option<PlaybackTrack>,
    pub(super) status: PlaybackStatus,
    pub(super) position: f64,
    pub(super) duration: f64,
    pub(super) error: Option<String>,
    pub(super) diagnostics: Vec<String>,
    pub(super) stream_url: Option<String>,
    pub(super) stream_copied: bool,
}

#[derive(Default)]
pub(super) enum PlaybackStatus {
    #[default]
    Idle,
    Resolving,
    Loading,
    Playing,
    Paused,
    Stopped,
    Error,
}

impl PlaybackStatus {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Resolving => "Resolving stream...",
            Self::Loading => "Loading in mpv...",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
            Self::Error => "Playback error",
        }
    }
}

pub(super) struct Notification {
    pub(super) title: &'static str,
    pub(super) message: String,
    pub(super) color: Color,
    pub(super) mode: NotificationMode,
    pub(super) expires_at: Instant,
}

pub(super) enum NotificationMode {
    Trim,
    Wrap,
}

impl Notification {
    pub(super) fn info(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Cyan, NotificationMode::Trim)
    }

    pub(super) fn success(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Green, NotificationMode::Trim)
    }

    pub(super) fn warning(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Yellow, NotificationMode::Trim)
    }

    pub(super) fn error(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Red, NotificationMode::Wrap)
    }

    fn new(
        title: &'static str,
        message: impl Into<String>,
        color: Color,
        mode: NotificationMode,
    ) -> Self {
        Self {
            title,
            message: message.into(),
            color,
            mode,
            expires_at: Instant::now() + Duration::from_secs(5),
        }
    }
}

pub(super) fn home_shelves(feed: MusicHomeFeed) -> Vec<HomeShelf> {
    let mut shelves = feed
        .shelves
        .into_iter()
        .map(|shelf| {
            let mut items = shelf
                .tracks
                .into_iter()
                .map(|track| {
                    let artist = track
                        .artists
                        .into_iter()
                        .map(|artist| artist.name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    HomeEntry::Track {
                        video_id: track.video_id,
                        title: track.title,
                        artist: if artist.is_empty() {
                            "Unknown artist".to_owned()
                        } else {
                            artist
                        },
                    }
                })
                .collect::<Vec<_>>();
            items.extend(shelf.albums.into_iter().map(|album| HomeEntry::Album {
                browse_id: album.browse_id,
                title: album.title,
                artist: album.artist.unwrap_or_else(|| "Unknown artist".to_owned()),
            }));
            items.extend(shelf.playlists.into_iter().map(|playlist| {
                HomeEntry::Playlist {
                    browse_id: playlist.browse_id,
                    title: playlist.title,
                    author: playlist
                        .author
                        .unwrap_or_else(|| "YouTube Music".to_owned()),
                }
            }));
            HomeShelf {
                title: shelf.title,
                items,
            }
        })
        .collect::<Vec<_>>();
    if let Some(index) = shelves
        .iter()
        .position(|shelf| shelf.title.eq_ignore_ascii_case("Quick picks"))
    {
        let quick_picks = shelves.remove(index);
        shelves.insert(0, quick_picks);
    }
    shelves
}

pub(super) fn search_items(results: MusicSearchResults) -> Vec<SearchItem> {
    let tracks = results
        .songs
        .into_iter()
        .map(|track| ("Song", track))
        .chain(results.videos.into_iter().map(|track| ("Video", track)));
    let mut items = tracks
        .map(|(kind, track)| {
            let artists = track
                .artists
                .into_iter()
                .map(|artist| artist.name)
                .collect::<Vec<_>>()
                .join(", ");
            SearchItem {
                kind,
                title: track.title,
                detail: if artists.is_empty() {
                    "Unknown artist".to_owned()
                } else {
                    artists
                },
                video_id: Some(track.video_id),
            }
        })
        .collect::<Vec<_>>();
    items.extend(results.albums.into_iter().map(|album| SearchItem {
        kind: "Album",
        title: album.title,
        detail: album.artist.unwrap_or_else(|| "Unknown artist".to_owned()),
        video_id: None,
    }));
    items.extend(results.artists.into_iter().map(|artist| SearchItem {
        kind: "Artist",
        title: artist.name,
        detail: artist.subscribers.unwrap_or_default(),
        video_id: None,
    }));
    items.extend(results.playlists.into_iter().map(|playlist| {
        SearchItem {
            kind: "Playlist",
            title: playlist.title,
            detail: playlist
                .author
                .unwrap_or_else(|| "YouTube Music".to_owned()),
            video_id: None,
        }
    }));
    items
}
