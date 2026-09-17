use crate::scraper::ytmusic::YTMusic;
use color_eyre::eyre::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use innertube_rs::{MusicHomeFeed, MusicSearchResults};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph},
};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tokio::runtime::Runtime;

pub mod scraper;

const NAV_ITEMS: [&str; 2] = ["Home", "Search"];
const TABS: [&str; 3] = ["List", "Lyrics", "Player"];

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
    tab: usize,
    runtime: Runtime,
    ytmusic: Option<YTMusic>,
    init_receiver: Option<Receiver<innertube_rs::error::Result<YTMusic>>>,
    home_receiver: Option<Receiver<innertube_rs::error::Result<MusicHomeFeed>>>,
    home_items: Vec<HomeItem>,
    home_state: ListState,
    home_error: Option<String>,
    notification: Option<Notification>,
    search_query: String,
    search_editing: bool,
    search_results: Option<MusicSearchResults>,
    search_error: Option<String>,
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
            tab: 0,
            runtime,
            ytmusic: None,
            init_receiver: Some(init_receiver),
            home_receiver: None,
            home_items: Vec::new(),
            home_state: ListState::default(),
            home_error: None,
            notification: None,
            search_query: String::new(),
            search_editing: false,
            search_results: None,
            search_error: None,
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        loop {
            self.poll_initialization();
            self.poll_home();
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
                } else if self.focus == Focus::Content {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                            self.focus = Focus::Nav
                        }
                        KeyCode::Up | KeyCode::Char('k') if self.is_home_list() => {
                            self.previous_home()
                        }
                        KeyCode::Down | KeyCode::Char('j') if self.is_home_list() => {
                            self.next_home()
                        }
                        KeyCode::Enter if self.is_home_list() => self.select_home_item(),
                        KeyCode::Enter if self.is_search_list() => self.search_editing = true,
                        KeyCode::BackTab => self.previous_tab(),
                        KeyCode::Tab => self.next_tab(),
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
                        KeyCode::BackTab => self.previous_tab(),
                        KeyCode::Tab => self.next_tab(),
                        KeyCode::Char('/') => {
                            self.nav.select(Some(1));
                            self.tab = 0;
                            self.focus = Focus::Content;
                            self.search_editing = true;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let [body, help] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .areas(frame.area());
        let [nav_area, workspace] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(20), Constraint::Min(20)])
            .areas(body);
        let [tabs_area, content_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .areas(workspace);

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

        let tabs_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        let tabs_inner = tabs_block.inner(tabs_area);
        frame.render_widget(tabs_block, tabs_area);

        let tab_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(33),
                Constraint::Percentage(34),
                Constraint::Percentage(33),
            ])
            .split(tabs_inner);
        for (index, label) in TABS.iter().enumerate() {
            let style = if index == self.tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(*label)
                    .alignment(Alignment::Center)
                    .style(style),
                tab_areas[index],
            );
        }

        self.render_tab(frame, content_area);
        let help_text = if self.search_editing {
            " type query  Enter: search  Esc: cancel "
        } else if self.focus == Focus::Nav {
            " j/k: navigate  l/right/Enter: content  Tab: switch tab  /: search  q: quit "
        } else if self.is_home_list() {
            " j/k: select  Enter: play  h/left/Esc: navigation  Tab: switch tab "
        } else {
            " Enter: select  h/left/Esc: navigation  Tab: switch tab  q: quit "
        };
        frame.render_widget(
            Paragraph::new(Line::from(help_text)).style(Style::default().fg(Color::DarkGray)),
            help,
        );
        self.render_notification(frame);
    }

    fn render_tab(&mut self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let section = NAV_ITEMS[self.nav.selected().unwrap_or_default()];
        let block = Block::default()
            .title(format!(" {section} / {} ", TABS[self.tab]))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(if self.focus == Focus::Content {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            })
            .padding(Padding::proportional(1));

        match self.tab {
            0 if section == "Search" => self.render_search(frame, area, block),
            0 => self.render_home(frame, area, block),
            1 => frame.render_widget(Paragraph::new("No lyrics loaded.").block(block), area),
            2 => frame.render_widget(Paragraph::new("Nothing is playing.").block(block), area),
            _ => unreachable!(),
        }
    }

    fn render_home(&mut self, frame: &mut Frame, area: Rect, block: Block) {
        if self.home_items.is_empty() {
            let message = if let Some(error) = &self.home_error {
                error.as_str()
            } else if self.init_receiver.is_some() {
                "Initializing YouTube Music..."
            } else if self.home_receiver.is_some() {
                "Loading your home feed..."
            } else {
                "No playable tracks were found in the home feed."
            };
            frame.render_widget(Paragraph::new(message).block(block), area);
            return;
        }

        let items = self.home_items.iter().map(|item| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", item.shelf),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    item.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", item.artist),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        });
        let list = List::new(items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        frame.render_stateful_widget(list, area, &mut self.home_state);
    }

    fn render_search(&self, frame: &mut Frame, area: ratatui::layout::Rect, block: Block) {
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [query_area, results_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .areas(inner);
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
        } else if let Some(results) = &self.search_results {
            frame.render_widget(List::new(search_items(results)), results_area);
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
                self.search_results = Some(results);
                self.search_error = None;
            }
            Err(error) => {
                self.search_results = None;
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
                self.home_items = home_items(feed);
                if !self.home_items.is_empty() {
                    self.home_state.select(Some(0));
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
        self.nav.selected() == Some(1) && self.tab == 0
    }

    fn is_home_list(&self) -> bool {
        self.nav.selected() == Some(0) && self.tab == 0
    }

    fn previous_home(&mut self) {
        let selected = self.home_state.selected().unwrap_or_default();
        self.home_state.select(Some(selected.saturating_sub(1)));
    }

    fn next_home(&mut self) {
        if self.home_items.is_empty() {
            return;
        }

        let selected = self.home_state.selected().unwrap_or_default();
        self.home_state
            .select(Some((selected + 1).min(self.home_items.len() - 1)));
    }

    fn select_home_item(&mut self) {
        let Some(item) = self
            .home_state
            .selected()
            .and_then(|index| self.home_items.get(index))
        else {
            return;
        };

        self.notification = Some(Notification::info(
            "Now playing",
            format!("Playing {} by {}", item.title, item.artist),
        ));
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

    fn previous_tab(&mut self) {
        self.tab = self.tab.checked_sub(1).unwrap_or(TABS.len() - 1);
    }

    fn next_tab(&mut self) {
        self.tab = (self.tab + 1) % TABS.len();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Nav,
    Content,
}

struct HomeItem {
    shelf: String,
    title: String,
    artist: String,
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

fn home_items(feed: MusicHomeFeed) -> Vec<HomeItem> {
    feed.shelves
        .into_iter()
        .flat_map(|shelf| {
            shelf.tracks.into_iter().map(move |track| {
                let artist = track
                    .artists
                    .into_iter()
                    .map(|artist| artist.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                HomeItem {
                    shelf: shelf.title.clone(),
                    title: track.title,
                    artist: if artist.is_empty() {
                        "Unknown artist".to_owned()
                    } else {
                        artist
                    },
                }
            })
        })
        .collect()
}

fn search_items(results: &MusicSearchResults) -> Vec<ListItem<'static>> {
    let tracks = results
        .songs
        .iter()
        .map(|track| ("Song", track))
        .chain(results.videos.iter().map(|track| ("Video", track)));
    let tracks = tracks.map(|(kind, track)| {
        let artists = track
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        result_item(kind, &track.title, &artists)
    });
    let albums = results.albums.iter().map(|album| {
        result_item(
            "Album",
            &album.title,
            album.artist.as_deref().unwrap_or("Unknown artist"),
        )
    });
    let artists = results.artists.iter().map(|artist| {
        result_item(
            "Artist",
            &artist.name,
            artist.subscribers.as_deref().unwrap_or(""),
        )
    });
    let playlists = results.playlists.iter().map(|playlist| {
        result_item(
            "Playlist",
            &playlist.title,
            playlist.author.as_deref().unwrap_or("YouTube Music"),
        )
    });

    tracks
        .chain(albums)
        .chain(artists)
        .chain(playlists)
        .collect()
}

fn result_item(kind: &str, title: &str, detail: &str) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(format!("[{kind}] "), Style::default().fg(Color::Cyan)),
        Span::styled(
            title.to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {detail}"), Style::default().fg(Color::DarkGray)),
    ]))
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
