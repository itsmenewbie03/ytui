use crate::scraper::ytmusic::{WatchTracking, YTMusicHomeFeed, YTMusicSearchResults};
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
        playlist_id: Option<String>,
        title: String,
        artist: String,
        album: Option<String>,
        art_url: Option<String>,
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

    pub(super) fn playback_track(&self) -> Option<PlaybackTrack> {
        let Self::Track {
            video_id,
            title,
            artist,
            album,
            art_url,
            ..
        } = self
        else {
            return None;
        };
        Some(PlaybackTrack {
            video_id: video_id.clone(),
            title: title.clone(),
            artist: artist.clone(),
            album: album.clone(),
            art_url: art_url.clone(),
            duration: None,
            views: None,
            likes: None,
        })
    }

    pub(super) fn playlist_id(&self) -> Option<&str> {
        match self {
            Self::Track { playlist_id, .. } => playlist_id.as_deref(),
            Self::Album { .. } | Self::Playlist { .. } => None,
        }
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
    pub(super) browse_id: Option<String>,
    pub(super) album: Option<String>,
    pub(super) art_url: Option<String>,
}

#[derive(Clone)]
pub(super) struct PlaybackTrack {
    pub(super) video_id: String,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: Option<String>,
    pub(super) art_url: Option<String>,
    pub(super) duration: Option<String>,
    pub(super) views: Option<u64>,
    pub(super) likes: Option<u64>,
}

#[derive(Default)]
pub(super) struct PlaybackState {
    pub(super) queue: Vec<PlaybackTrack>,
    pub(super) queue_index: Option<usize>,
    pub(super) queue_loading: bool,
    pub(super) queue_error: Option<String>,
    pub(super) track: Option<PlaybackTrack>,
    pub(super) status: PlaybackStatus,
    pub(super) position: f64,
    pub(super) duration: f64,
    pub(super) error: Option<String>,
    pub(super) diagnostics: Vec<String>,
    pub(super) stream_url: Option<String>,
    pub(super) stream_copied: bool,
    pub(super) watch_tracking: Option<WatchTracking>,
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
    pub(super) color: Option<Color>,
    pub(super) mode: NotificationMode,
    pub(super) expires_at: Instant,
}

pub(super) enum NotificationMode {
    Trim,
    Wrap,
}

impl Notification {
    pub(super) fn info(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, None, NotificationMode::Trim)
    }

    pub(super) fn success(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Some(Color::Green), NotificationMode::Trim)
    }

    pub(super) fn warning(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Some(Color::Yellow), NotificationMode::Trim)
    }

    pub(super) fn error(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Some(Color::Red), NotificationMode::Wrap)
    }

    fn new(
        title: &'static str,
        message: impl Into<String>,
        color: Option<Color>,
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

pub(super) fn home_shelves(feed: YTMusicHomeFeed) -> Vec<HomeShelf> {
    let mut shelves = feed
        .feed
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
                        playlist_id: None,
                        title: track.title,
                        artist: if artist.is_empty() {
                            "Unknown artist".to_owned()
                        } else {
                            artist
                        },
                        album: track.album.map(|album| album.title),
                        art_url: track.thumbnail,
                    }
                })
                .collect::<Vec<_>>();
            items.extend(shelf.albums.into_iter().map(|album| HomeEntry::Album {
                browse_id: album.browse_id,
                title: album.title,
                artist: album.artist.unwrap_or_else(|| "Unknown artist".to_owned()),
            }));
            items.extend(shelf.playlists.into_iter().map(|playlist| {
                let play_target = feed.play_targets.iter().find(|target| {
                    target.item_id == playlist.browse_id && target.title == playlist.title
                });
                if let Some(target) = play_target
                    && target.video_id.as_deref() == Some(playlist.browse_id.as_str())
                    && let Some(video_id) = &target.video_id
                {
                    let artist = playlist
                        .author
                        .as_deref()
                        .and_then(|author| {
                            author
                                .strip_prefix("Song • ")
                                .or_else(|| author.strip_prefix("Video • "))
                        })
                        .or(playlist.author.as_deref())
                        .unwrap_or("Unknown artist")
                        .to_owned();
                    HomeEntry::Track {
                        video_id: video_id.clone(),
                        playlist_id: target.playlist_id.clone(),
                        title: playlist.title,
                        artist,
                        album: None,
                        art_url: playlist.thumbnail,
                    }
                } else {
                    HomeEntry::Playlist {
                        browse_id: playlist.browse_id,
                        title: playlist.title,
                        author: playlist
                            .author
                            .unwrap_or_else(|| "YouTube Music".to_owned()),
                    }
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

pub(super) fn search_items(results: YTMusicSearchResults) -> Vec<SearchItem> {
    let top_result = results.top_result;
    let results = results.sections;
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
                browse_id: None,
                album: track.album.map(|album| album.title),
                art_url: track.thumbnail,
            }
        })
        .collect::<Vec<_>>();
    items.extend(results.albums.into_iter().map(|album| SearchItem {
        kind: "Album",
        title: album.title,
        detail: album.artist.unwrap_or_else(|| "Unknown artist".to_owned()),
        video_id: None,
        browse_id: Some(album.browse_id),
        album: None,
        art_url: None,
    }));
    items.extend(results.artists.into_iter().map(|artist| SearchItem {
        kind: "Artist",
        title: artist.name,
        detail: artist.subscribers.unwrap_or_default(),
        video_id: None,
        browse_id: Some(artist.browse_id),
        album: None,
        art_url: None,
    }));
    items.extend(results.playlists.into_iter().map(|playlist| {
        SearchItem {
            kind: "Playlist",
            title: playlist.title,
            detail: playlist
                .author
                .unwrap_or_else(|| "YouTube Music".to_owned()),
            video_id: None,
            browse_id: Some(playlist.browse_id),
            album: None,
            art_url: None,
        }
    }));
    if let Some(top) = top_result {
        let kind = match top.kind.as_str() {
            "Song" => "Song",
            "Video" => "Video",
            "Album" => "Album",
            "Artist" => "Artist",
            "Playlist" => "Playlist",
            _ => "Top result",
        };
        items.retain(|item| match (&top.video_id, &item.video_id) {
            (Some(top_id), Some(item_id)) => top_id != item_id,
            _ => item.title != top.title || item.kind != kind,
        });
        items.insert(
            0,
            SearchItem {
                kind,
                title: top.title,
                detail: top.detail,
                video_id: top.video_id,
                browse_id: top.browse_id,
                album: None,
                art_url: top.art_url,
            },
        );
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::ytmusic::{HomePlayTarget, MusicSearchTopResult};
    use innertube_rs::{
        MusicHomeFeed, MusicPlaylistItem, MusicSearchResults, MusicShelf, MusicTrackItem,
    };

    #[test]
    fn turns_playable_listen_again_card_into_track() {
        let shelves = home_shelves(YTMusicHomeFeed {
            feed: MusicHomeFeed {
                shelves: vec![MusicShelf {
                    title: "Listen again".to_owned(),
                    playlists: vec![MusicPlaylistItem {
                        browse_id: "8i_VTKjtRkk".to_owned(),
                        title: "Wala Man Sa'yo Ang Lahat".to_owned(),
                        author: Some("Song • Myrus".to_owned()),
                        thumbnail: Some("https://example.com/art.jpg".to_owned()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            play_targets: vec![HomePlayTarget {
                item_id: "8i_VTKjtRkk".to_owned(),
                title: "Wala Man Sa'yo Ang Lahat".to_owned(),
                video_id: Some("8i_VTKjtRkk".to_owned()),
                playlist_id: Some("RDAMVM8i_VTKjtRkk".to_owned()),
            }],
        });

        let entry = &shelves[0].items[0];
        assert_eq!(entry.kind(), "Song");
        assert_eq!(entry.detail(), "Myrus");
        assert_eq!(entry.playlist_id(), Some("RDAMVM8i_VTKjtRkk"));
        assert_eq!(
            entry.playback_track().map(|track| track.video_id),
            Some("8i_VTKjtRkk".to_owned())
        );
    }

    #[test]
    fn puts_top_search_result_first_without_duplicates() {
        let results = YTMusicSearchResults {
            top_result: Some(MusicSearchTopResult {
                kind: "Song".to_owned(),
                title: "Ganda Mo".to_owned(),
                detail: "Cue C".to_owned(),
                video_id: Some("top-video".to_owned()),
                browse_id: None,
                art_url: Some("https://example.com/top.jpg".to_owned()),
            }),
            sections: MusicSearchResults {
                songs: vec![
                    MusicTrackItem {
                        video_id: "other-video".to_owned(),
                        title: "Other result".to_owned(),
                        ..Default::default()
                    },
                    MusicTrackItem {
                        video_id: "top-video".to_owned(),
                        title: "Ganda Mo".to_owned(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        };

        let items = search_items(results);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Ganda Mo");
        assert_eq!(items[0].video_id.as_deref(), Some("top-video"));
        assert_eq!(items[1].title, "Other result");
    }
}
