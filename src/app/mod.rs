mod model;
mod ui;

use self::model::{
    Focus, HomeShelf, Notification, PlaybackState, PlaybackStatus, PlaybackTrack, Screen,
    SearchItem, home_shelves, search_items,
};
use crate::config::{Config, Credentials};
use crate::player::{MpvPlayer, PlayerEvent, copy_to_clipboard};
use crate::scraper::ytmusic::{AccountIdentity, AudioStreamInfo, UpNextQueue, YTMusic};
use color_eyre::eyre::{Context, Result};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind},
    execute,
};
use innertube_rs::{MusicHomeFeed, MusicSearchResults};
use ratatui::{DefaultTerminal, style::Color, widgets::ListState};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;

const NAV_ITEMS: [&str; 3] = ["Home", "Search", "Settings"];
const PLAYER_TABS: [&str; 4] = ["Lyrics", "Up Next", "Comments", "Related"];
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const DEFAULT_ACCENT: usize = 10;
const ACCENT_COLORS: [AccentColor; 14] = [
    AccentColor::new("Rosewater", 0xf5, 0xe0, 0xdc),
    AccentColor::new("Flamingo", 0xf2, 0xcd, 0xcd),
    AccentColor::new("Pink", 0xf5, 0xc2, 0xe7),
    AccentColor::new("Mauve", 0xcb, 0xa6, 0xf7),
    AccentColor::new("Red", 0xf3, 0x8b, 0xa8),
    AccentColor::new("Maroon", 0xeb, 0xa0, 0xac),
    AccentColor::new("Peach", 0xfa, 0xb3, 0x87),
    AccentColor::new("Yellow", 0xf9, 0xe2, 0xaf),
    AccentColor::new("Green", 0xa6, 0xe3, 0xa1),
    AccentColor::new("Teal", 0x94, 0xe2, 0xd5),
    AccentColor::new("Sky", 0x89, 0xdc, 0xeb),
    AccentColor::new("Sapphire", 0x74, 0xc7, 0xec),
    AccentColor::new("Blue", 0x89, 0xb4, 0xfa),
    AccentColor::new("Lavender", 0xb4, 0xbe, 0xfe),
];
type PlayRequestResult = innertube_rs::error::Result<AudioStreamInfo>;
type UpNextRequestResult = innertube_rs::error::Result<UpNextQueue>;
type SearchRequestResult = innertube_rs::error::Result<MusicSearchResults>;
type InitRequestResult = innertube_rs::error::Result<(YTMusic, Option<AccountIdentity>)>;
type AuthRequestResult = innertube_rs::error::Result<(YTMusic, AccountIdentity, Credentials)>;

#[derive(Clone, Copy)]
struct AccentColor {
    name: &'static str,
    red: u8,
    green: u8,
    blue: u8,
}

impl AccentColor {
    const fn new(name: &'static str, red: u8, green: u8, blue: u8) -> Self {
        Self {
            name,
            red,
            green,
            blue,
        }
    }

    const fn color(self) -> Color {
        Color::Rgb(self.red, self.green, self.blue)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CookieInputKind {
    NetscapeFile,
    Header,
}

pub fn run() -> Result<()> {
    let runtime = Runtime::new().wrap_err("failed to create async runtime")?;
    let (config, config_warning) = match Config::load() {
        Ok(config) => (config, None),
        Err(error) => (Config::default(), Some(error)),
    };
    let (credentials, credentials_warning) = match Credentials::load() {
        Ok(credentials) => (credentials, None),
        Err(error) => (None, Some(error)),
    };
    let has_credentials = credentials.is_some();
    let cookie = credentials.map(|credentials| credentials.cookie().to_owned());
    let (init_sender, init_receiver) = mpsc::channel();
    runtime.spawn(async move {
        let _ = init_sender.send(initialize_client(cookie).await);
    });

    let warning = [config_warning, credentials_warning]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ");
    let mut app = App::new(
        runtime,
        init_receiver,
        config,
        has_credentials,
        (!warning.is_empty()).then_some(warning),
    );
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    let run_result = ratatui::run(|terminal| app.run(terminal));
    let paste_result = execute!(std::io::stdout(), DisableBracketedPaste);
    run_result?;
    paste_result?;
    Ok(())
}

async fn initialize_client(cookie: Option<String>) -> InitRequestResult {
    let authenticated = cookie.is_some();
    let ytmusic = YTMusic::new(cookie).await?;
    let identity = if authenticated {
        Some(ytmusic.account_identity().await?)
    } else {
        None
    };
    Ok((ytmusic, identity))
}

struct App {
    nav: ListState,
    focus: Focus,
    screen: Screen,
    player_tab: usize,
    runtime: Runtime,
    config: Config,
    ytmusic: Option<YTMusic>,
    init_receiver: Option<Receiver<InitRequestResult>>,
    init_with_cookie: bool,
    auth_receiver: Option<Receiver<AuthRequestResult>>,
    cookie_input: Option<String>,
    cookie_input_kind: CookieInputKind,
    has_credentials: bool,
    account_identity: Option<AccountIdentity>,
    home_receiver: Option<Receiver<innertube_rs::error::Result<MusicHomeFeed>>>,
    home_shelves: Vec<HomeShelf>,
    home_shelf: usize,
    home_state: ListState,
    home_error: Option<String>,
    play_receiver: Option<Receiver<PlayRequestResult>>,
    up_next_receiver: Option<Receiver<UpNextRequestResult>>,
    up_next_state: ListState,
    player: Option<MpvPlayer>,
    playback: PlaybackState,
    notification: Option<Notification>,
    quit_confirmation: bool,
    search_query: String,
    search_editing: bool,
    search_items: Vec<SearchItem>,
    search_state: ListState,
    search_complete: bool,
    search_error: Option<String>,
    search_receiver: Option<Receiver<SearchRequestResult>>,
    settings_state: ListState,
    settings_row: usize,
    animation_started: Instant,
}

impl App {
    fn new(
        runtime: Runtime,
        init_receiver: Receiver<InitRequestResult>,
        config: Config,
        has_credentials: bool,
        config_warning: Option<String>,
    ) -> Self {
        let mut nav = ListState::default();
        nav.select(Some(0));
        let accent_index = ACCENT_COLORS
            .iter()
            .position(|accent| accent.name.eq_ignore_ascii_case(&config.accent));
        let mut config_warning = config_warning;
        if accent_index.is_none() && config_warning.is_none() {
            config_warning = Some(format!("unknown accent color: {}", config.accent));
        }
        let mut settings_state = ListState::default();
        settings_state.select(Some(accent_index.unwrap_or(DEFAULT_ACCENT)));

        Self {
            nav,
            focus: Focus::Nav,
            screen: Screen::Main,
            player_tab: 0,
            runtime,
            config,
            ytmusic: None,
            init_receiver: Some(init_receiver),
            init_with_cookie: has_credentials,
            auth_receiver: None,
            cookie_input: None,
            cookie_input_kind: CookieInputKind::NetscapeFile,
            has_credentials,
            account_identity: None,
            home_receiver: None,
            home_shelves: Vec::new(),
            home_shelf: 0,
            home_state: ListState::default(),
            home_error: None,
            play_receiver: None,
            up_next_receiver: None,
            up_next_state: ListState::default(),
            player: None,
            playback: PlaybackState::default(),
            notification: config_warning
                .map(|error| Notification::warning("Config not loaded", error)),
            quit_confirmation: false,
            search_query: String::new(),
            search_editing: false,
            search_items: Vec::new(),
            search_state: ListState::default(),
            search_complete: false,
            search_error: None,
            search_receiver: None,
            settings_state,
            settings_row: 0,
            animation_started: Instant::now(),
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        loop {
            self.poll_initialization();
            self.poll_authentication();
            self.poll_home();
            self.poll_search();
            self.poll_play_request();
            self.poll_up_next();
            self.poll_player_events();
            self.expire_notification();
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))? {
                let event = event::read()?;
                if let Event::Paste(text) = &event
                    && self.cookie_input.is_some()
                {
                    self.append_cookie_input(text);
                    continue;
                }
                let Event::Key(key) = event else { continue };
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if self.quit_confirmation {
                    match key.code {
                        KeyCode::Enter | KeyCode::Char('y') => return Ok(()),
                        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {
                            self.quit_confirmation = false;
                        }
                        _ => {}
                    }
                } else if self.cookie_input.is_some() {
                    match key.code {
                        KeyCode::Enter => self.submit_cookie(),
                        KeyCode::Esc => self.cookie_input = None,
                        KeyCode::Backspace => {
                            if let Some(input) = &mut self.cookie_input {
                                input.pop();
                            }
                        }
                        KeyCode::Char(character) => {
                            if let Some(input) = &mut self.cookie_input
                                && input.len() + character.len_utf8() <= 16 * 1024
                            {
                                input.push(character);
                            }
                        }
                        _ => {}
                    }
                } else if self.search_editing {
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
                        KeyCode::Char('q') => self.quit_confirmation = true,
                        KeyCode::Esc | KeyCode::Char('P') => self.screen = Screen::Main,
                        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                            self.previous_player_tab();
                        }
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                            self.next_player_tab();
                        }
                        KeyCode::Up | KeyCode::Char('k') if self.player_tab == 1 => {
                            self.previous_up_next();
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.player_tab == 1 => {
                            self.next_up_next();
                        }
                        KeyCode::Enter if self.player_tab == 1 => self.play_selected_up_next(),
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
                        KeyCode::Char('q') => self.quit_confirmation = true,
                        KeyCode::Esc => self.focus = Focus::Nav,
                        KeyCode::Left | KeyCode::Char('h') if self.is_home_list() => {
                            self.previous_home_shelf();
                        }
                        KeyCode::Right | KeyCode::Char('l') if self.is_home_list() => {
                            self.next_home_shelf();
                        }
                        KeyCode::Left | KeyCode::Char('h') if self.is_settings() => {
                            if self.settings_row == 0 {
                                self.previous_accent();
                            } else {
                                self.focus = Focus::Nav;
                            }
                        }
                        KeyCode::Right | KeyCode::Char('l')
                            if self.is_settings() && self.settings_row == 0 =>
                        {
                            self.next_accent();
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
                        KeyCode::Up | KeyCode::Char('k') if self.is_settings() => {
                            self.settings_row = self.settings_row.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_search_list() => {
                            self.next_search();
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_settings() => {
                            self.settings_row = (self.settings_row + 1).min(1);
                        }
                        KeyCode::Enter if self.is_home_list() => self.select_home_item(),
                        KeyCode::Enter if self.is_search_list() => self.select_search_item(),
                        KeyCode::Enter if self.is_settings() && self.settings_row == 1 => {
                            self.open_cookie_input(CookieInputKind::NetscapeFile);
                        }
                        KeyCode::Char('c') if self.is_settings() && self.settings_row == 1 => {
                            self.open_cookie_input(CookieInputKind::Header);
                        }
                        KeyCode::Char('d') if self.is_settings() && self.settings_row == 1 => {
                            self.remove_cookie();
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.quit_confirmation = true,
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
            Ok(Ok((ytmusic, identity))) => {
                self.init_receiver = None;
                self.init_with_cookie = false;
                self.has_credentials = identity.is_some() || self.has_credentials;
                self.account_identity = identity;
                self.activate_client(ytmusic);
                self.notification = Some(Notification::success(
                    "Connected",
                    if self.account_identity.is_some() {
                        "Signed in to YouTube Music."
                    } else {
                        "YouTube Music initialized successfully."
                    },
                ));
            }
            Ok(Err(error)) => {
                self.init_receiver = None;
                if self.init_with_cookie {
                    self.start_initialization(None);
                    self.notification = Some(Notification::warning(
                        "Cookie not accepted",
                        format!("{error}. Starting an anonymous session."),
                    ));
                } else {
                    self.home_error = Some(format!("Initialization failed: {error}"));
                    self.notification = Some(Notification::error(
                        "Initialization failed",
                        error.to_string(),
                    ));
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.init_receiver = None;
                if self.init_with_cookie {
                    self.start_initialization(None);
                    self.notification = Some(Notification::warning(
                        "Cookie validation stopped",
                        "Starting an anonymous session.",
                    ));
                } else {
                    self.home_error = Some("The background task stopped unexpectedly.".to_owned());
                    self.notification = Some(Notification::error(
                        "Initialization failed",
                        "The background task stopped unexpectedly.",
                    ));
                }
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn poll_authentication(&mut self) {
        let Some(receiver) = &self.auth_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok((ytmusic, identity, credentials))) => {
                self.auth_receiver = None;
                if let Err(error) = credentials.save() {
                    self.notification = Some(Notification::error("Cookie not saved", error));
                    return;
                }
                self.has_credentials = true;
                self.account_identity = Some(identity);
                self.activate_client(ytmusic);
                self.notification = Some(Notification::success(
                    "Signed in",
                    "Your YouTube Music account is connected.",
                ));
            }
            Ok(Err(error)) => {
                self.auth_receiver = None;
                self.notification = Some(Notification::error(
                    "Cookie not accepted",
                    error.to_string(),
                ));
            }
            Err(TryRecvError::Disconnected) => {
                self.auth_receiver = None;
                self.notification = Some(Notification::error(
                    "Sign-in failed",
                    "The validation task stopped unexpectedly.",
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn activate_client(&mut self, ytmusic: YTMusic) {
        let home_client = ytmusic.clone();
        let (sender, receiver) = mpsc::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(home_client.get_home().await);
        });
        self.ytmusic = Some(ytmusic);
        self.home_shelves.clear();
        self.home_shelf = 0;
        self.home_state.select(None);
        self.home_error = None;
        self.home_receiver = Some(receiver);
        self.search_receiver = None;
        self.play_receiver = None;
        self.up_next_receiver = None;
    }

    fn start_initialization(&mut self, cookie: Option<String>) {
        let has_cookie = cookie.is_some();
        let (sender, receiver) = mpsc::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(initialize_client(cookie).await);
        });
        self.ytmusic = None;
        self.init_receiver = Some(receiver);
        self.init_with_cookie = has_cookie;
        self.home_receiver = None;
        self.home_shelves.clear();
        self.home_state.select(None);
        self.home_error = None;
        self.search_receiver = None;
        self.play_receiver = None;
        self.up_next_receiver = None;
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
            Ok(Ok(stream)) => {
                self.play_receiver = None;
                self.player = None;
                self.playback.stream_url = Some(stream.url.clone());
                if let Some(track) = &mut self.playback.track {
                    track.views = stream.views;
                    track.likes = stream.likes;
                }
                match MpvPlayer::start(&stream.url) {
                    Ok(player) => {
                        self.player = Some(player);
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

    fn poll_up_next(&mut self) {
        let Some(receiver) = &self.up_next_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(queue)) => {
                self.up_next_receiver = None;
                self.playback.queue_loading = false;
                self.playback.queue_error = None;
                let current_track = self.playback.track.clone();
                let mut tracks = queue
                    .tracks
                    .into_iter()
                    .map(|track| PlaybackTrack {
                        video_id: track.video_id,
                        title: track.title,
                        artist: track.artist,
                        duration: track.duration,
                        views: None,
                        likes: None,
                    })
                    .collect::<Vec<_>>();
                let current_index = current_track.as_ref().and_then(|current| {
                    tracks
                        .get(queue.current_index)
                        .filter(|track| track.video_id == current.video_id)
                        .map(|_| queue.current_index)
                        .or_else(|| {
                            tracks
                                .iter()
                                .position(|track| track.video_id == current.video_id)
                        })
                });
                let current_index = match (current_index, current_track) {
                    (Some(index), _) => index,
                    (None, Some(current)) => {
                        tracks.insert(0, current);
                        0
                    }
                    (None, None) => queue.current_index.min(tracks.len().saturating_sub(1)),
                };
                self.playback.queue = tracks;
                self.playback.queue_index =
                    (!self.playback.queue.is_empty()).then_some(current_index);
                self.up_next_state.select(self.playback.queue_index);
            }
            Ok(Err(error)) => {
                self.up_next_receiver = None;
                self.playback.queue_loading = false;
                self.playback.queue_error = Some(format!("Could not load Automix: {error}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.up_next_receiver = None;
                self.playback.queue_loading = false;
                self.playback.queue_error =
                    Some("The Automix task stopped unexpectedly.".to_owned());
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

    fn is_settings(&self) -> bool {
        self.nav.selected() == Some(2)
    }

    fn accent_color(&self) -> Color {
        ACCENT_COLORS[self.settings_state.selected().unwrap_or(DEFAULT_ACCENT)].color()
    }

    fn previous_accent(&mut self) {
        let selected = self.settings_state.selected().unwrap_or(DEFAULT_ACCENT);
        self.select_accent(selected.saturating_sub(1));
    }

    fn next_accent(&mut self) {
        let selected = self.settings_state.selected().unwrap_or(DEFAULT_ACCENT);
        self.select_accent((selected + 1).min(ACCENT_COLORS.len() - 1));
    }

    fn select_accent(&mut self, index: usize) {
        self.settings_state.select(Some(index));
        self.config.accent = ACCENT_COLORS[index].name.to_owned();
        if let Err(error) = self.config.save() {
            self.notification = Some(Notification::warning("Config not saved", error));
        }
    }

    fn open_cookie_input(&mut self, kind: CookieInputKind) {
        if self.auth_receiver.is_some() {
            self.notification = Some(Notification::info(
                "Validating cookie",
                "Wait for the current sign-in attempt to finish.",
            ));
            return;
        }
        self.cookie_input_kind = kind;
        self.cookie_input = Some(String::new());
    }

    fn append_cookie_input(&mut self, text: &str) {
        let Some(input) = &mut self.cookie_input else {
            return;
        };
        let remaining = (16_usize * 1024).saturating_sub(input.len());
        input.extend(text.chars().take(remaining));
    }

    fn submit_cookie(&mut self) {
        let Some(input) = self.cookie_input.take() else {
            return;
        };
        let credentials = match self.cookie_input_kind {
            CookieInputKind::NetscapeFile => Credentials::from_netscape_file(&input),
            CookieInputKind::Header => Credentials::new(&input),
        };
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(error) => {
                self.notification = Some(Notification::warning("Invalid credentials", error));
                return;
            }
        };
        let cookie = credentials.cookie().to_owned();
        let (sender, receiver) = mpsc::channel();
        self.runtime.spawn(async move {
            let result = async {
                let ytmusic = YTMusic::new(Some(cookie)).await?;
                let identity = ytmusic.account_identity().await?;
                Ok((ytmusic, identity, credentials))
            }
            .await;
            let _ = sender.send(result);
        });
        self.auth_receiver = Some(receiver);
        self.notification = Some(Notification::info(
            "Validating cookie",
            "Checking your YouTube Music account...",
        ));
    }

    fn remove_cookie(&mut self) {
        if !self.has_credentials && self.account_identity.is_none() {
            self.notification = Some(Notification::info(
                "Not signed in",
                "There is no saved cookie to remove.",
            ));
            return;
        }
        if let Err(error) = Credentials::remove() {
            self.notification = Some(Notification::error("Cookie not removed", error));
            return;
        }
        self.auth_receiver = None;
        self.cookie_input = None;
        self.has_credentials = false;
        self.account_identity = None;
        self.start_initialization(None);
        self.notification = Some(Notification::success(
            "Signed out",
            "The local browser cookie was removed.",
        ));
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
        if let Some(track) = entry.playback_track() {
            self.request_play(track, true);
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
        if let Some(video_id) = &self.search_items[index].video_id {
            let track = PlaybackTrack {
                video_id: video_id.clone(),
                title: self.search_items[index].title.clone(),
                artist: self.search_items[index].detail.clone(),
                duration: None,
                views: None,
                likes: None,
            };
            self.request_play(track, true);
        } else {
            self.notification = Some(Notification::warning(
                "Not playable",
                "Select a song or video result.",
            ));
        }
    }

    fn request_play(&mut self, track: PlaybackTrack, load_queue: bool) {
        let Some(ytmusic) = self.ytmusic.clone() else {
            self.notification = Some(Notification::warning(
                "Not ready",
                "YouTube Music is still initializing.",
            ));
            return;
        };
        let video_id = track.video_id.clone();
        let title = track.title.clone();
        let (sender, receiver) = mpsc::channel();
        let stream_video_id = video_id.clone();
        self.runtime.spawn(async move {
            let result = ytmusic.get_audio_stream(&stream_video_id).await;
            let _ = sender.send(result);
        });
        if load_queue {
            let Some(queue_client) = self.ytmusic.clone() else {
                return;
            };
            let (sender, receiver) = mpsc::channel();
            self.runtime.spawn(async move {
                let _ = sender.send(queue_client.get_up_next(&video_id).await);
            });
            self.up_next_receiver = Some(receiver);
            self.playback.queue = vec![track.clone()];
            self.playback.queue_index = Some(0);
            self.playback.queue_loading = true;
            self.playback.queue_error = None;
            self.up_next_state.select(Some(0));
        }
        self.play_receiver = Some(receiver);
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
        let Some(current) = self.playback.queue_index else {
            return;
        };
        let next = current + 1;
        if next < self.playback.queue.len() {
            self.play_queue_index(next);
        } else {
            self.playback.status = PlaybackStatus::Stopped;
            self.notification = Some(Notification::info("Queue finished", "No next track."));
        }
    }

    fn previous_track(&mut self) {
        let Some(current) = self.playback.queue_index else {
            return;
        };
        if current > 0 {
            self.play_queue_index(current - 1);
        }
    }

    fn play_queue_index(&mut self, index: usize) {
        let Some(track) = self.playback.queue.get(index).cloned() else {
            return;
        };
        self.playback.queue_index = Some(index);
        self.up_next_state.select(Some(index));
        self.request_play(track, false);
    }

    fn previous_up_next(&mut self) {
        let selected = self.up_next_state.selected().unwrap_or_default();
        self.up_next_state.select(Some(selected.saturating_sub(1)));
    }

    fn next_up_next(&mut self) {
        if self.playback.queue.is_empty() {
            return;
        }
        let selected = self.up_next_state.selected().unwrap_or_default();
        self.up_next_state
            .select(Some((selected + 1).min(self.playback.queue.len() - 1)));
    }

    fn play_selected_up_next(&mut self) {
        if let Some(index) = self.up_next_state.selected() {
            self.play_queue_index(index);
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
