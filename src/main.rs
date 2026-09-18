use crate::player::{MpvPlayer, PlayerEvent, copy_to_clipboard};
use crate::scraper::ytmusic::YTMusic;
use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use innertube_rs::{MusicHomeFeed, MusicSearchResults};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{
        Block, BorderType, Borders, Clear, Gauge, LineGauge, List, ListItem, ListState, Padding,
        Paragraph, Wrap,
    },
};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;

pub mod player;
pub mod scraper;

const NAV_ITEMS: [&str; 2] = ["Home", "Search"];
const PLAYER_TABS: [&str; 4] = ["Lyrics", "Up Next", "Comments", "Related"];
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
type PlayRequestResult = innertube_rs::error::Result<(QueueSource, usize, String)>;

fn main() -> Result<()> {
    color_eyre::install()?;
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
            animation_started: Instant::now(),
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        loop {
            self.poll_initialization();
            self.poll_home();
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
                        KeyCode::Enter => self.submit_search(terminal)?,
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
                            self.previous_player_tab()
                        }
                        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                            self.next_player_tab()
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
                            self.previous_home_shelf()
                        }
                        KeyCode::Right | KeyCode::Char('l') if self.is_home_list() => {
                            self.next_home_shelf()
                        }
                        KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
                        KeyCode::Up | KeyCode::Char('k') if self.is_home_list() => {
                            self.previous_home()
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_home_list() => {
                            self.next_home()
                        }
                        KeyCode::Up | KeyCode::Char('k') if self.is_search_list() => {
                            self.previous_search()
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_search_list() => {
                            self.next_search()
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
                            self.focus = Focus::Content
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        match self.screen {
            Screen::Main => self.render_main(frame),
            Screen::Player => self.render_player_screen(frame),
        }
        self.render_notification(frame);
    }

    fn render_main(&mut self, frame: &mut Frame) {
        let mini_height = u16::from(self.playback.track.is_some()) * 4;
        let [body, mini_player, help] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(mini_height),
                Constraint::Length(1),
            ])
            .areas(frame.area());
        let [nav_area, workspace] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(20)])
            .areas(body);

        let nav = List::new(NAV_ITEMS.map(ListItem::new))
            .block(
                Block::default()
                    .title(" ytui ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .highlight_symbol("> ")
            .highlight_style(if self.focus == Focus::Nav {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            });
        frame.render_stateful_widget(nav, nav_area, &mut self.nav);

        self.render_main_content(frame, workspace);
        if mini_height > 0 {
            self.render_mini_player(frame, mini_player);
        }
        let help_text = if self.search_editing {
            " type query  Enter: search  Esc: cancel "
        } else if self.focus == Focus::Nav {
            " j/k: navigate  l/Enter: content  /: search  P: player  q: quit "
        } else if self.is_home_list() {
            " h/l: shelf  j/k: select  Enter: play  Esc: navigation  P: player "
        } else if self.is_search_list() {
            " j/k: select  Enter: play  /: search  Space: pause  P: player  Esc: navigation "
        } else {
            " Space: pause  [/] seek  n/p: track  P: player  Esc: navigation "
        };
        frame.render_widget(
            Paragraph::new(Line::from(help_text)).style(Style::default().fg(Color::DarkGray)),
            help,
        );
    }

    fn render_main_content(&mut self, frame: &mut Frame, area: Rect) {
        let section = NAV_ITEMS[self.nav.selected().unwrap_or_default()];
        if section == "Search" {
            self.render_search(frame, area);
        } else {
            self.render_home(frame, area);
        }
    }

    fn render_home(&mut self, frame: &mut Frame, area: Rect) {
        if self.home_shelves.is_empty() {
            let (message, loading) = if let Some(error) = &self.home_error {
                (error.clone(), false)
            } else if self.init_receiver.is_some() {
                ("Initializing YouTube Music...".to_owned(), true)
            } else if self.home_receiver.is_some() {
                ("Loading your home feed...".to_owned(), true)
            } else {
                (
                    "No playable tracks were found in the home feed.".to_owned(),
                    false,
                )
            };
            let message = if loading {
                format!("{}  {message}", self.spinner_frame())
            } else {
                message
            };
            let message_area = Rect::new(area.x, area.y + area.height / 2, area.width, 1);
            frame.render_widget(
                Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(if loading {
                        Color::LightCyan
                    } else {
                        Color::DarkGray
                    })),
                message_area,
            );
            return;
        }

        let [shelf_tabs, list_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .areas(area);
        let labels = self
            .home_shelves
            .iter()
            .map(|shelf| shelf.title.as_str())
            .collect::<Vec<_>>();
        render_bubble_tabs(frame, shelf_tabs, &labels, self.home_shelf);

        let shelf_window = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Cyan));
        let shelf_content = shelf_window.inner(list_area);
        frame.render_widget(shelf_window, list_area);

        let shelf = &self.home_shelves[self.home_shelf];
        if shelf.items.is_empty() {
            frame.render_widget(
                Paragraph::new("This shelf has no items.")
                    .style(Style::default().fg(Color::DarkGray)),
                shelf_content,
            );
            return;
        }

        let selected = self.home_state.selected();
        let items = shelf.items.iter().enumerate().map(|(index, item)| {
            let is_selected = selected == Some(index);
            let title_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            let detail_style = if is_selected {
                Style::default().fg(Color::LightCyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(vec![
                Line::styled(item.title().to_owned(), title_style),
                Line::styled(format!("{} - {}", item.kind(), item.detail()), detail_style),
            ])
        });
        let list = List::new(items)
            .highlight_symbol("│ ")
            .highlight_style(Style::default().fg(Color::Cyan))
            .repeat_highlight_symbol(true);
        frame.render_stateful_widget(list, shelf_content, &mut self.home_state);
    }

    fn render_mini_player(&self, frame: &mut Frame, area: Rect) {
        let Some(track) = &self.playback.track else {
            return;
        };
        let [progress_area, controls_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(3)])
            .areas(area);
        frame.render_widget(
            LineGauge::default()
                .ratio(self.playback_ratio())
                .filled_style(Style::default().fg(Color::Red))
                .unfilled_style(Style::default().fg(Color::DarkGray))
                .label(""),
            progress_area,
        );

        let strip_style = Style::default().fg(Color::Gray).bg(Color::Rgb(32, 33, 36));
        frame.render_widget(Block::default().style(strip_style), controls_area);
        let [transport_area, track_area, time_area, actions_area] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(18),
                Constraint::Min(28),
                Constraint::Length(16),
                Constraint::Length(25),
            ])
            .areas(controls_area);
        let play_icon = if matches!(self.playback.status, PlaybackStatus::Paused) {
            "▶"
        } else {
            "Ⅱ"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::styled(
                    format!("  ⏮    {play_icon}    ⏭"),
                    strip_style.add_modifier(Modifier::BOLD),
                ),
            ])
            .style(strip_style),
            transport_area,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    format!("♫  {}", track.title),
                    strip_style.add_modifier(Modifier::BOLD),
                ),
                Line::styled(track.artist.clone(), strip_style.fg(Color::DarkGray)),
                Line::styled(
                    self.playback.status.label(),
                    strip_style.fg(Color::DarkGray),
                ),
            ])
            .style(strip_style),
            track_area,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(format!(
                    "{} / {}",
                    format_time(self.playback.position),
                    format_time(self.playback.duration)
                )),
            ])
            .alignment(Alignment::Center)
            .style(strip_style.fg(Color::LightCyan)),
            time_area,
        );
        frame.render_widget(
            Paragraph::new(vec![Line::from(""), Line::from("↶ 10s    10s ↷    [P]")])
                .alignment(Alignment::Center)
                .style(strip_style),
            actions_area,
        );
    }

    fn render_player_screen(&self, frame: &mut Frame) {
        let [summary, tabs, content, help] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(6),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .areas(frame.area());
        self.render_player_summary(frame, summary);
        render_bubble_tabs(frame, tabs, &PLAYER_TABS, self.player_tab);

        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Cyan))
            .padding(Padding::proportional(1));
        let inner = block.inner(content);
        frame.render_widget(block, content);
        match self.player_tab {
            0 => frame.render_widget(Paragraph::new("No lyrics loaded."), inner),
            1 => self.render_up_next(frame, inner),
            2 => frame.render_widget(Paragraph::new("Comments are not loaded yet."), inner),
            3 => frame.render_widget(Paragraph::new("Related tracks are not loaded yet."), inner),
            _ => unreachable!(),
        }
        frame.render_widget(
            Paragraph::new(
                " h/l or Tab: section  Space: pause  [/] seek  n/p: track  Esc/P: back  q: quit ",
            )
            .style(Style::default().fg(Color::DarkGray)),
            help,
        );
    }

    fn render_player_summary(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Player ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(track) = &self.playback.track else {
            frame.render_widget(Paragraph::new("Nothing is playing."), inner);
            return;
        };
        let [details, progress, status] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(inner);
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    track.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    format!("{}  -  {}", track.artist, track.video_id),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            details,
        );
        frame.render_widget(self.progress_gauge(), progress);
        let status_line = self
            .playback
            .error
            .as_deref()
            .unwrap_or(self.playback.status.label());
        frame.render_widget(
            Paragraph::new(status_line)
                .style(if self.playback.error.is_some() {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                })
                .wrap(Wrap { trim: true }),
            status,
        );
    }

    fn progress_gauge(&self) -> Gauge<'static> {
        Gauge::default()
            .ratio(self.playback_ratio())
            .gauge_style(Style::default().fg(Color::Cyan))
            .label(format!(
                "{} / {}",
                format_time(self.playback.position),
                format_time(self.playback.duration)
            ))
    }

    fn playback_ratio(&self) -> f64 {
        if self.playback.duration > 0.0 {
            (self.playback.position / self.playback.duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn spinner_frame(&self) -> &'static str {
        let frame = (self.animation_started.elapsed().as_millis() / 100) as usize;
        SPINNER_FRAMES[frame % SPINNER_FRAMES.len()]
    }

    fn render_up_next(&self, frame: &mut Frame, area: Rect) {
        let Some(source) = self.playback.source else {
            frame.render_widget(Paragraph::new("The queue is empty."), area);
            return;
        };
        let current = self.playback.current_index.unwrap_or_default();
        let items = match source {
            QueueSource::Home(shelf) => self
                .home_shelves
                .get(shelf)
                .into_iter()
                .flat_map(|shelf| shelf.items.iter().skip(current + 1))
                .filter_map(HomeEntry::playback_track)
                .collect::<Vec<_>>(),
            QueueSource::Search => self
                .search_items
                .iter()
                .skip(current + 1)
                .filter_map(|item| {
                    item.video_id.as_ref().map(|video_id| PlaybackTrack {
                        video_id: video_id.clone(),
                        title: item.title.clone(),
                        artist: item.detail.clone(),
                    })
                })
                .collect(),
        };
        if items.is_empty() {
            frame.render_widget(Paragraph::new("Nothing else is queued."), area);
            return;
        }
        frame.render_widget(
            List::new(items.into_iter().map(|track| {
                ListItem::new(vec![
                    Line::styled(track.title, Style::default().add_modifier(Modifier::BOLD)),
                    Line::styled(track.artist, Style::default().fg(Color::DarkGray)),
                ])
            })),
            area,
        );
    }

    fn render_search(&mut self, frame: &mut Frame, area: Rect) {
        let [query_area, results_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .areas(area);
        let query = if self.search_query.is_empty() && !self.search_editing {
            "Press Enter to type a query".to_owned()
        } else if self.search_editing {
            format!("{}|", self.search_query)
        } else {
            self.search_query.clone()
        };
        let query_style = if self.search_editing {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(query).style(query_style).block(
                Block::default()
                    .title(" Query ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .padding(Padding::horizontal(1)),
            ),
            query_area,
        );

        if let Some(error) = &self.search_error {
            frame.render_widget(
                Paragraph::new(error.as_str()).style(Style::default().fg(Color::Red)),
                results_area,
            );
        } else if !self.search_items.is_empty() {
            let [count_area, list_area] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .areas(results_area);
            frame.render_widget(
                Paragraph::new(format!("{} results", self.search_items.len()))
                    .alignment(Alignment::Right)
                    .style(Style::default().fg(Color::DarkGray)),
                count_area,
            );

            let selected = self.search_state.selected();
            let items = self.search_items.iter().enumerate().map(|(index, item)| {
                let is_selected = selected == Some(index);
                let title_style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                let detail_style = if is_selected {
                    Style::default().fg(Color::LightCyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                ListItem::new(vec![
                    Line::styled(item.title.clone(), title_style),
                    Line::styled(format!("{} - {}", item.kind, item.detail), detail_style),
                ])
            });
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("│ ")
                    .highlight_style(Style::default().fg(Color::Cyan))
                    .repeat_highlight_symbol(true),
                list_area,
                &mut self.search_state,
            );
        } else if self.search_complete {
            frame.render_widget(Paragraph::new("No results found."), results_area);
        } else {
            frame.render_widget(
                Paragraph::new("Search songs, videos, albums, artists, and playlists.")
                    .style(Style::default().fg(Color::DarkGray)),
                results_area,
            );
        }
    }

    fn submit_search(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let query = self.search_query.trim().to_owned();
        if query.is_empty() {
            return Ok(());
        }

        if self.ytmusic.is_none() {
            let message = "YouTube Music is still initializing. Try again shortly.";
            self.search_error = Some(message.to_owned());
            self.notification = Some(Notification::warning("Not ready", message));
            return Ok(());
        }

        self.search_editing = false;
        self.search_error = Some("Searching...".to_owned());
        terminal.draw(|frame| self.render(frame))?;

        let ytmusic = self.ytmusic.as_ref().expect("client checked above");
        match self.runtime.block_on(ytmusic.search(&query)) {
            Ok(results) => {
                self.search_items = search_items(results);
                self.search_state
                    .select((!self.search_items.is_empty()).then_some(0));
                self.search_complete = true;
                self.search_error = None;
            }
            Err(error) => {
                self.search_items.clear();
                self.search_state.select(None);
                self.search_complete = true;
                self.search_error = Some(format!("Search failed: {error}"));
            }
        }

        Ok(())
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
                if self
                    .home_shelves
                    .first()
                    .is_some_and(|shelf| !shelf.items.is_empty())
                {
                    self.home_state.select(Some(0));
                } else {
                    self.home_state.select(None);
                }
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
                let result = MpvPlayer::start(&url);

                match result {
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
            let Some(event) = event else {
                break;
            };

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

    fn render_notification(&self, frame: &mut Frame) {
        let Some(notification) = &self.notification else {
            return;
        };

        let area = frame.area();
        if area.width < 12 || area.height < 3 {
            return;
        }

        let max_width = area.width.saturating_sub(2).min(52);
        let preferred_width = notification
            .message
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or_default()
            .max(notification.title.chars().count() + 2)
            + 4;
        let width = preferred_width
            .min(usize::from(max_width))
            .max(usize::from(max_width.min(12))) as u16;
        let content_width = usize::from(width.saturating_sub(4).max(1));
        let mut lines = match notification.mode {
            NotificationMode::Trim => {
                vec![trim_with_ellipsis(&notification.message, content_width)]
            }
            NotificationMode::Wrap => wrap_text(&notification.message, content_width),
        };
        let vertical_margin = u16::from(area.height >= 5);
        let max_popup_height = area.height.saturating_sub(vertical_margin * 2);
        let max_lines = usize::from(max_popup_height.saturating_sub(2).max(1));
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                *last = trim_with_ellipsis(&format!("{last}..."), content_width);
            }
        }
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX) + 2;
        let popup = Rect::new(
            area.right().saturating_sub(width + 1),
            area.y.saturating_add(vertical_margin),
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).block(
                Block::default()
                    .title(format!(" {} ", notification.title))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(notification.color))
                    .padding(Padding::horizontal(1)),
            ),
            popup,
        );
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
            .map_or(0, |shelf| shelf.items.len());
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
            .is_some_and(|shelf| !shelf.items.is_empty());
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
                .get(shelf)
                .and_then(|shelf| shelf.items.get(index))
                .and_then(HomeEntry::playback_track),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Nav,
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Main,
    Player,
}

enum HomeEntry {
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
    fn title(&self) -> &str {
        match self {
            Self::Track { title, .. }
            | Self::Album { title, .. }
            | Self::Playlist { title, .. } => title,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Track { .. } => "Song",
            Self::Album { .. } => "Album",
            Self::Playlist { .. } => "Playlist",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Track { artist, .. } | Self::Album { artist, .. } => artist,
            Self::Playlist { author, .. } => author,
        }
    }

    fn is_playable(&self) -> bool {
        matches!(self, Self::Track { .. })
    }

    fn playback_track(&self) -> Option<PlaybackTrack> {
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

    fn browse_message(&self) -> String {
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

struct HomeShelf {
    title: String,
    items: Vec<HomeEntry>,
}

struct SearchItem {
    kind: &'static str,
    title: String,
    detail: String,
    video_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueSource {
    Home(usize),
    Search,
}

struct PlaybackTrack {
    video_id: String,
    title: String,
    artist: String,
}

#[derive(Default)]
struct PlaybackState {
    source: Option<QueueSource>,
    current_index: Option<usize>,
    track: Option<PlaybackTrack>,
    status: PlaybackStatus,
    position: f64,
    duration: f64,
    error: Option<String>,
    diagnostics: Vec<String>,
    stream_url: Option<String>,
    stream_copied: bool,
}

#[derive(Default)]
enum PlaybackStatus {
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
    fn label(&self) -> &'static str {
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

struct Notification {
    title: &'static str,
    message: String,
    color: Color,
    mode: NotificationMode,
    expires_at: Instant,
}

enum NotificationMode {
    Trim,
    Wrap,
}

impl Notification {
    fn info(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Cyan, NotificationMode::Trim)
    }

    fn success(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Green, NotificationMode::Trim)
    }

    fn warning(title: &'static str, message: impl Into<String>) -> Self {
        Self::new(title, message, Color::Yellow, NotificationMode::Trim)
    }

    fn error(title: &'static str, message: impl Into<String>) -> Self {
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

fn trim_with_ellipsis(message: &str, max_width: usize) -> String {
    let characters = message.chars().collect::<Vec<_>>();
    if characters.len() <= max_width {
        return message.to_owned();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    characters[..max_width - 3]
        .iter()
        .chain(['.', '.', '.'].iter())
        .collect()
}

fn wrap_text(message: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in message.lines() {
        let characters = paragraph.chars().collect::<Vec<_>>();
        if characters.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut start = 0;
        while start < characters.len() {
            let mut end = (start + max_width).min(characters.len());
            if end < characters.len()
                && let Some(space) = characters[start..end].iter().rposition(|char| *char == ' ')
                && space > 0
            {
                end = start + space;
            }

            lines.push(characters[start..end].iter().collect());
            start = end;
            while start < characters.len() && characters[start] == ' ' {
                start += 1;
            }
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn home_shelves(feed: MusicHomeFeed) -> Vec<HomeShelf> {
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

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn render_bubble_tabs(frame: &mut Frame, area: Rect, labels: &[&str], selected: usize) {
    if labels.is_empty() || area.width < 4 || area.height < 3 {
        return;
    }

    let widths = labels
        .iter()
        .map(|label| (label.chars().count() as u16 + 4).min(area.width))
        .collect::<Vec<_>>();
    let mut start = 0;
    while start < selected && widths[start..=selected].iter().copied().sum::<u16>() > area.width {
        start += 1;
    }
    let mut end = start;
    let mut used = 0;
    while end < labels.len() && used + widths[end] <= area.width {
        used += widths[end];
        end += 1;
    }

    let mut x = area.x;
    let bottom = area.y + 2;
    for (visible_index, index) in (start..end).enumerate() {
        let is_selected = index == selected;
        let mut label = labels[index].to_owned();
        if visible_index == 0 && start > 0 {
            label.insert_str(0, "< ");
        }
        if visible_index + 1 == end - start && end < labels.len() {
            label.push_str(" >");
        }
        let width = widths[index].min(area.right().saturating_sub(x));
        let tab_area = Rect::new(x, area.y, width, 3);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        frame.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(style)
                .block(block),
            tab_area,
        );

        let left = if visible_index == 0 {
            if is_selected { "│" } else { "├" }
        } else if is_selected {
            "┘"
        } else {
            "┴"
        };
        let reaches_right = x + width == area.right();
        let right = if reaches_right {
            if is_selected { "│" } else { "┤" }
        } else if is_selected {
            "└"
        } else {
            "┴"
        };
        let line = if is_selected { " " } else { "─" };
        let border_style = Style::default().fg(Color::Cyan);
        let buffer = frame.buffer_mut();
        buffer[(x, bottom)].set_symbol(left).set_style(border_style);
        for column in x + 1..x + width - 1 {
            buffer[(column, bottom)]
                .set_symbol(line)
                .set_style(border_style);
        }
        buffer[(x + width - 1, bottom)]
            .set_symbol(right)
            .set_style(border_style);
        x += width;
    }

    if x < area.right() {
        let border_style = Style::default().fg(Color::Cyan);
        let buffer = frame.buffer_mut();
        for column in x..area.right() - 1 {
            buffer[(column, bottom)]
                .set_symbol("─")
                .set_style(border_style);
        }
        buffer[(area.right() - 1, bottom)]
            .set_symbol("┐")
            .set_style(border_style);
    }
}

fn search_items(results: MusicSearchResults) -> Vec<SearchItem> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_notifications_with_ellipsis() {
        assert_eq!(trim_with_ellipsis("abcdefgh", 6), "abc...");
        assert_eq!(trim_with_ellipsis("short", 6), "short");
    }

    #[test]
    fn wraps_notifications_on_word_boundaries() {
        assert_eq!(wrap_text("one two three", 7), ["one", "two", "three"]);
    }
}
