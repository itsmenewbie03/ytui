mod model;
mod ui;

use self::model::{
    Focus, HomeShelf, Notification, PlaybackState, PlaybackStatus, PlaybackTrack, QueueSource,
    Screen, SearchItem, home_shelves, search_items,
};
use crate::player::{MpvPlayer, PlayerEvent, copy_to_clipboard};
use crate::scraper::ytmusic::YTMusic;
use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use innertube_rs::{MusicHomeFeed, MusicSearchResults};
use ratatui::{DefaultTerminal, widgets::ListState};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;

const NAV_ITEMS: [&str; 2] = ["Home", "Search"];
const PLAYER_TABS: [&str; 4] = ["Lyrics", "Up Next", "Comments", "Related"];
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
type PlayRequestResult = innertube_rs::error::Result<(QueueSource, usize, String)>;
type SearchRequestResult = innertube_rs::error::Result<MusicSearchResults>;

pub fn run() -> Result<()> {
    let runtime = Runtime::new().wrap_err("failed to create async runtime")?;
    let (init_sender, init_receiver) = mpsc::channel();
    runtime.spawn(async move {
        let _ = init_sender.send(YTMusic::new().await);
    });

    let mut app = App::new(runtime, init_receiver);
    ratatui::run(|terminal| app.run(terminal))?;
    Ok(())
}

struct App {
    nav: ListState,
    focus: Focus,
    screen: Screen,
    player_tab: usize,
    runtime: Runtime,
    ytmusic: Option<YTMusic>,
    init_receiver: Option<Receiver<innertube_rs::error::Result<YTMusic>>>,
    home_receiver: Option<Receiver<innertube_rs::error::Result<MusicHomeFeed>>>,
    home_shelves: Vec<HomeShelf>,
    home_shelf: usize,
    home_state: ListState,
    home_error: Option<String>,
    play_receiver: Option<Receiver<PlayRequestResult>>,
    player: Option<MpvPlayer>,
    playback: PlaybackState,
    notification: Option<Notification>,
    search_query: String,
    search_editing: bool,
    search_items: Vec<SearchItem>,
    search_state: ListState,
    search_complete: bool,
    search_error: Option<String>,
    search_receiver: Option<Receiver<SearchRequestResult>>,
    animation_started: Instant,
}

impl App {
    fn new(
        runtime: Runtime,
        init_receiver: Receiver<innertube_rs::error::Result<YTMusic>>,
    ) -> Self {
        let mut nav = ListState::default();
        nav.select(Some(0));

        Self {
            nav,
            focus: Focus::Nav,
            screen: Screen::Main,
            player_tab: 0,
            runtime,
            ytmusic: None,
            init_receiver: Some(init_receiver),
            home_receiver: None,
            home_shelves: Vec::new(),
            home_shelf: 0,
            home_state: ListState::default(),
            home_error: None,
            play_receiver: None,
            player: None,
            playback: PlaybackState::default(),
            notification: None,
            search_query: String::new(),
            search_editing: false,
            search_items: Vec::new(),
            search_state: ListState::default(),
            search_complete: false,
            search_error: None,
            search_receiver: None,
            animation_started: Instant::now(),
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        loop {
            self.poll_initialization();
            self.poll_home();
            self.poll_search();
            self.poll_play_request();
            self.poll_player_events();
            self.expire_notification();
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                if self.search_editing {
                    match key.code {
                        KeyCode::Enter => self.submit_search(),
                        KeyCode::Esc => self.search_editing = false,
                        KeyCode::Backspace => {
                            self.search_query.pop();
                        }
                        KeyCode::Char(character) => self.search_query.push(character),
                        _ => {}
                    }
                } else if self.screen == Screen::Player {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Char('P') => self.screen = Screen::Main,
                        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                            self.previous_player_tab();
                        }
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                            self.next_player_tab();
                        }
                        KeyCode::Char(' ') => self.toggle_pause(),
                        KeyCode::Char('[') => self.seek(-10),
                        KeyCode::Char(']') => self.seek(10),
                        KeyCode::Char('n') => self.next_track(),
                        KeyCode::Char('p') => self.previous_track(),
                        _ => {}
                    }
                } else if key.code == KeyCode::Char('P') {
                    self.screen = Screen::Player;
                } else if key.code == KeyCode::Char(' ') {
                    self.toggle_pause();
                } else if key.code == KeyCode::Char('[') {
                    self.seek(-10);
                } else if key.code == KeyCode::Char(']') {
                    self.seek(10);
                } else if key.code == KeyCode::Char('n') {
                    self.next_track();
                } else if key.code == KeyCode::Char('p') {
                    self.previous_track();
                } else if key.code == KeyCode::Char('/') {
                    self.nav.select(Some(1));
                    self.focus = Focus::Content;
                    self.search_editing = true;
                } else if self.focus == Focus::Content {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc => self.focus = Focus::Nav,
                        KeyCode::Left | KeyCode::Char('h') if self.is_home_list() => {
                            self.previous_home_shelf();
                        }
                        KeyCode::Right | KeyCode::Char('l') if self.is_home_list() => {
                            self.next_home_shelf();
                        }
                        KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
                        KeyCode::Up | KeyCode::Char('k') if self.is_home_list() => {
                            self.previous_home();
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_home_list() => {
                            self.next_home();
                        }
                        KeyCode::Up | KeyCode::Char('k') if self.is_search_list() => {
                            self.previous_search();
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_search_list() => {
                            self.next_search();
                        }
                        KeyCode::Enter if self.is_home_list() => self.select_home_item(),
                        KeyCode::Enter if self.is_search_list() => self.select_search_item(),
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Up | KeyCode::Char('k') => self.previous_nav(),
                        KeyCode::Down | KeyCode::Char('j') => self.next_nav(),
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                            self.focus = Focus::Content;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn submit_search(&mut self) {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        let Some(ytmusic) = self.ytmusic.clone() else {
            let message = "YouTube Music is still initializing. Try again shortly.";
            self.search_error = Some(message.to_owned());
            self.notification = Some(Notification::warning("Not ready", message));
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(ytmusic.search(&query).await);
        });
        self.search_editing = false;
        self.search_complete = false;
        self.search_error = None;
        self.search_receiver = Some(receiver);
    }

    fn poll_search(&mut self) {
        let Some(receiver) = &self.search_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(results)) => {
                self.search_items = search_items(results);
                self.search_state
                    .select((!self.search_items.is_empty()).then_some(0));
                self.search_complete = true;
                self.search_error = None;
                self.search_receiver = None;
            }
            Ok(Err(error)) => {
                self.search_items.clear();
                self.search_state.select(None);
                self.search_complete = true;
                self.search_error = Some(format!("Search failed: {error}"));
                self.search_receiver = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.search_items.clear();
                self.search_state.select(None);
                self.search_complete = true;
                self.search_error = Some("The search task stopped unexpectedly.".to_owned());
                self.search_receiver = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_initialization(&mut self) {
        let Some(receiver) = &self.init_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(ytmusic)) => {
                let home_client = ytmusic.clone();
                let (sender, receiver) = mpsc::channel();
                self.runtime.spawn(async move {
                    let _ = sender.send(home_client.get_home().await);
                });
                self.ytmusic = Some(ytmusic);
                self.init_receiver = None;
                self.home_receiver = Some(receiver);
                self.notification = Some(Notification::success(
                    "Connected",
                    "YouTube Music initialized successfully.",
                ));
            }
            Ok(Err(error)) => {
                self.init_receiver = None;
                self.home_error = Some(format!("Initialization failed: {error}"));
                self.notification = Some(Notification::error(
                    "Initialization failed",
                    error.to_string(),
                ));
            }
            Err(TryRecvError::Disconnected) => {
                self.init_receiver = None;
                self.home_error = Some("The background task stopped unexpectedly.".to_owned());
                self.notification = Some(Notification::error(
                    "Initialization failed",
                    "The background task stopped unexpectedly.",
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_home(&mut self) {
        let Some(receiver) = &self.home_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(feed)) => {
                self.home_shelves = home_shelves(feed);
                self.home_shelf = 0;
                self.home_state.select(
                    self.home_shelves
                        .first()
                        .is_some_and(|shelf| !shelf.items.is_empty())
                        .then_some(0),
                );
                self.home_receiver = None;
            }
            Ok(Err(error)) => {
                let message = format!("Failed to load home feed: {error}");
                self.home_error = Some(message.clone());
                self.home_receiver = None;
                self.notification = Some(Notification::error("Home feed failed", message));
            }
            Err(TryRecvError::Disconnected) => {
                let message = "The home feed task stopped unexpectedly.".to_owned();
                self.home_error = Some(message.clone());
                self.home_receiver = None;
                self.notification = Some(Notification::error("Home feed failed", message));
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_play_request(&mut self) {
        let Some(receiver) = &self.play_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((source, index, url))) => {
                self.play_receiver = None;
                self.player = None;
                self.playback.stream_url = Some(url.clone());
                match MpvPlayer::start(&url) {
                    Ok(player) => {
                        self.player = Some(player);
                        self.playback.source = Some(source);
                        self.playback.current_index = Some(index);
                        self.playback.position = 0.0;
                        self.playback.duration = 0.0;
                        self.playback.status = PlaybackStatus::Loading;
                    }
                    Err(error) => self.playback_error(format!("Could not start mpv: {error}")),
                }
            }
            Ok(Err(error)) => {
                self.play_receiver = None;
                self.playback_error(format!("Could not resolve audio stream: {error}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.play_receiver = None;
                self.playback_error("The playback task stopped unexpectedly.".to_owned());
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_player_events(&mut self) {
        loop {
            let event = self
                .player
                .as_ref()
                .and_then(|player| player.try_recv().ok());
            let Some(event) = event else { break };
            match event {
                PlayerEvent::FileLoaded => self.mark_playing(),
                PlayerEvent::Position(position) => {
                    self.playback.position = position;
                    if matches!(self.playback.status, PlaybackStatus::Loading) {
                        self.mark_playing();
                    }
                }
                PlayerEvent::Duration(duration) => self.playback.duration = duration,
                PlayerEvent::Paused(paused)
                    if !matches!(
                        self.playback.status,
                        PlaybackStatus::Resolving | PlaybackStatus::Loading
                    ) =>
                {
                    self.playback.status = if paused {
                        PlaybackStatus::Paused
                    } else {
                        PlaybackStatus::Playing
                    };
                }
                PlayerEvent::Paused(_) => {}
                PlayerEvent::EndOfFile => self.next_track(),
                PlayerEvent::Diagnostic(message) => self.append_playback_diagnostic(message),
                PlayerEvent::Error(error) => self.playback_error(format!("mpv error: {error}")),
            }
        }
    }

    fn expire_notification(&mut self) {
        if self
            .notification
            .as_ref()
            .is_some_and(|notification| Instant::now() >= notification.expires_at)
        {
            self.notification = None;
        }
    }

    fn is_search_list(&self) -> bool {
        self.nav.selected() == Some(1)
    }
    fn is_home_list(&self) -> bool {
        self.nav.selected() == Some(0)
    }

    fn previous_home(&mut self) {
        let selected = self.home_state.selected().unwrap_or_default();
        self.home_state.select(Some(selected.saturating_sub(1)));
    }

    fn next_home(&mut self) {
        let track_count = self
            .home_shelves
            .get(self.home_shelf)
            .map_or(0, |s| s.items.len());
        if track_count == 0 {
            return;
        }
        let selected = self.home_state.selected().unwrap_or_default();
        self.home_state
            .select(Some((selected + 1).min(track_count - 1)));
    }

    fn previous_home_shelf(&mut self) {
        self.home_shelf = self.home_shelf.saturating_sub(1);
        self.reset_home_selection();
    }

    fn next_home_shelf(&mut self) {
        if self.home_shelves.is_empty() {
            return;
        }
        self.home_shelf = (self.home_shelf + 1).min(self.home_shelves.len() - 1);
        self.reset_home_selection();
    }

    fn reset_home_selection(&mut self) {
        let has_tracks = self
            .home_shelves
            .get(self.home_shelf)
            .is_some_and(|s| !s.items.is_empty());
        self.home_state.select(has_tracks.then_some(0));
    }

    fn previous_search(&mut self) {
        let selected = self.search_state.selected().unwrap_or_default();
        self.search_state.select(Some(selected.saturating_sub(1)));
    }

    fn next_search(&mut self) {
        if self.search_items.is_empty() {
            return;
        }
        let selected = self.search_state.selected().unwrap_or_default();
        self.search_state
            .select(Some((selected + 1).min(self.search_items.len() - 1)));
    }

    fn select_home_item(&mut self) {
        let Some(index) = self.home_state.selected() else {
            return;
        };
        let entry = &self.home_shelves[self.home_shelf].items[index];
        if entry.is_playable() {
            self.request_play(QueueSource::Home(self.home_shelf), index);
        } else {
            self.notification = Some(Notification::info("Browse", entry.browse_message()));
        }
    }

    fn select_search_item(&mut self) {
        if self.search_items.is_empty() {
            self.search_editing = true;
            return;
        }
        let Some(index) = self.search_state.selected() else {
            return;
        };
        if self.search_items[index].video_id.is_some() {
            self.request_play(QueueSource::Search, index);
        } else {
            self.notification = Some(Notification::warning(
                "Not playable",
                "Select a song or video result.",
            ));
        }
    }

    fn request_play(&mut self, source: QueueSource, index: usize) {
        let Some(ytmusic) = self.ytmusic.clone() else {
            self.notification = Some(Notification::warning(
                "Not ready",
                "YouTube Music is still initializing.",
            ));
            return;
        };
        let Some(track) = self.track(source, index) else {
            return;
        };
        let video_id = track.video_id.clone();
        let title = track.title.clone();
        let (sender, receiver) = mpsc::channel();
        self.runtime.spawn(async move {
            let result = ytmusic
                .get_audio_url(&video_id)
                .await
                .map(|url| (source, index, url));
            let _ = sender.send(result);
        });
        self.play_receiver = Some(receiver);
        self.playback.source = Some(source);
        self.playback.current_index = Some(index);
        self.playback.track = Some(track);
        self.playback.position = 0.0;
        self.playback.duration = 0.0;
        self.playback.status = PlaybackStatus::Resolving;
        self.playback.error = None;
        self.playback.diagnostics.clear();
        self.playback.stream_url = None;
        self.playback.stream_copied = false;
        self.notification = Some(Notification::info("Loading", format!("Resolving {title}")));
    }

    fn toggle_pause(&mut self) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        if let Err(error) = player.toggle_pause() {
            self.playback_error(format!("Could not toggle pause: {error}"));
        }
    }

    fn seek(&mut self, seconds: i64) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        if let Err(error) = player.seek(seconds) {
            self.playback_error(format!("Could not seek: {error}"));
        }
    }

    fn next_track(&mut self) {
        let Some(source) = self.playback.source else {
            return;
        };
        let Some(current) = self.playback.current_index else {
            return;
        };
        if let Some(next) = self.next_playable_index(source, current) {
            self.select_queue_index(source, next);
            self.request_play(source, next);
        } else {
            self.playback.status = PlaybackStatus::Stopped;
            self.notification = Some(Notification::info("Queue finished", "No next track."));
        }
    }

    fn previous_track(&mut self) {
        let Some(source) = self.playback.source else {
            return;
        };
        let Some(current) = self.playback.current_index else {
            return;
        };
        if let Some(previous) = self.previous_playable_index(source, current) {
            self.select_queue_index(source, previous);
            self.request_play(source, previous);
        }
    }

    fn track(&self, source: QueueSource, index: usize) -> Option<PlaybackTrack> {
        match source {
            QueueSource::Home(shelf) => self
                .home_shelves
                .get(shelf)?
                .items
                .get(index)?
                .playback_track(),
            QueueSource::Search => self.search_items.get(index).and_then(|item| {
                item.video_id.as_ref().map(|video_id| PlaybackTrack {
                    video_id: video_id.clone(),
                    title: item.title.clone(),
                    artist: item.detail.clone(),
                })
            }),
        }
    }

    fn next_playable_index(&self, source: QueueSource, current: usize) -> Option<usize> {
        match source {
            QueueSource::Home(shelf) => self.home_shelves.get(shelf).and_then(|shelf| {
                (current + 1..shelf.items.len()).find(|index| shelf.items[*index].is_playable())
            }),
            QueueSource::Search => (current + 1..self.search_items.len())
                .find(|index| self.search_items[*index].video_id.is_some()),
        }
    }

    fn previous_playable_index(&self, source: QueueSource, current: usize) -> Option<usize> {
        match source {
            QueueSource::Home(shelf) => self.home_shelves.get(shelf).and_then(|shelf| {
                (0..current)
                    .rev()
                    .find(|index| shelf.items[*index].is_playable())
            }),
            QueueSource::Search => (0..current)
                .rev()
                .find(|index| self.search_items[*index].video_id.is_some()),
        }
    }

    fn select_queue_index(&mut self, source: QueueSource, index: usize) {
        match source {
            QueueSource::Home(shelf) => {
                self.home_shelf = shelf;
                self.home_state.select(Some(index));
            }
            QueueSource::Search => self.search_state.select(Some(index)),
        }
    }

    fn playback_error(&mut self, message: String) {
        self.playback.status = PlaybackStatus::Error;
        let mut full_message = if self.playback.diagnostics.is_empty() {
            message
        } else {
            format!("{}\n{}", self.playback.diagnostics.join("\n"), message)
        };
        if !self.playback.stream_copied
            && let Some(url) = &self.playback.stream_url
        {
            let clipboard_message = match copy_to_clipboard(url) {
                Ok(()) => "Signed stream URL copied to clipboard.".to_owned(),
                Err(error) => format!("Could not copy stream URL: {error}"),
            };
            self.playback.stream_copied = true;
            full_message.push('\n');
            full_message.push_str(&clipboard_message);
        }
        self.playback.error = Some(full_message.clone());
        self.notification = Some(Notification::error("Playback failed", full_message));
    }

    fn mark_playing(&mut self) {
        self.playback.status = PlaybackStatus::Playing;
        if let Some(track) = &self.playback.track {
            self.notification = Some(Notification::info(
                "Now playing",
                format!("Playing {} by {}", track.title, track.artist),
            ));
        }
    }

    fn append_playback_diagnostic(&mut self, message: String) {
        if self.playback.diagnostics.last() == Some(&message) {
            return;
        }
        self.playback.diagnostics.push(message.clone());
        if let Some(error) = &mut self.playback.error {
            error.push('\n');
            error.push_str(&message);
        }
    }

    fn previous_nav(&mut self) {
        let selected = self.nav.selected().unwrap_or_default();
        self.nav.select(Some(selected.saturating_sub(1)));
    }

    fn next_nav(&mut self) {
        let selected = self.nav.selected().unwrap_or_default();
        self.nav
            .select(Some((selected + 1).min(NAV_ITEMS.len() - 1)));
    }

    fn previous_player_tab(&mut self) {
        self.player_tab = self
            .player_tab
            .checked_sub(1)
            .unwrap_or(PLAYER_TABS.len() - 1);
    }

    fn next_player_tab(&mut self) {
        self.player_tab = (self.player_tab + 1) % PLAYER_TABS.len();
    }
}
