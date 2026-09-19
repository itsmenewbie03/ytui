use super::{App, NAV_ITEMS, PLAYER_TABS, SPINNER_FRAMES};
use crate::app::model::{
    Focus, HomeEntry, NotificationMode, PlaybackStatus, PlaybackTrack, QueueSource, Screen,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{
        Block, BorderType, Borders, Clear, Gauge, LineGauge, List, ListItem, Padding, Paragraph,
        Wrap,
    },
};

impl App {
    pub(super) fn render(&mut self, frame: &mut Frame) {
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
        if NAV_ITEMS[self.nav.selected().unwrap_or_default()] == "Search" {
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
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>();
        render_bubble_tabs(frame, shelf_tabs, &labels, self.home_shelf);
        let shelf_window = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(BorderType::Rounded)
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
            .border_type(BorderType::Rounded)
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
            "Search YouTube Music...".to_owned()
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
                    .title(" Search ")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .padding(Padding::horizontal(1)),
            ),
            query_area,
        );
        if self.search_receiver.is_some() {
            let loading_area = Rect::new(
                results_area.x,
                results_area.y + results_area.height / 2,
                results_area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "{}  Searching YouTube Music...",
                    self.spinner_frame()
                ))
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::LightCyan)),
                loading_area,
            );
        } else if let Some(error) = &self.search_error {
            let error_area = Rect::new(
                results_area.x,
                results_area.y + results_area.height / 2,
                results_area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::Red)),
                error_area,
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
            let empty_area = Rect::new(
                results_area.x,
                results_area.y + results_area.height / 2,
                results_area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new("No results found.")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                empty_area,
            );
        } else {
            let hint_area = Rect::new(
                results_area.x,
                results_area.y + results_area.height.saturating_sub(2) / 2,
                results_area.width,
                2,
            );
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        "Find something to play",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Line::styled(
                        "Songs, videos, albums, artists, and playlists",
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
                .alignment(Alignment::Center),
                hint_area,
            );
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
