use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use soul_forge::{App as SoulApp, AppState};
use std::{
    io,
    time::{Duration, Instant},
};

/// TUI wrapper: holds library app state and ratatui ListState for rendering.
struct App {
    inner: SoulApp,
    list_state: ListState,
}

impl App {
    fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            inner: SoulApp::new(),
            list_state,
        }
    }

    fn next_stage(&mut self) {
        self.inner.next_stage();
        self.sync_list_from_inner();
    }

    fn handle_selection(&mut self) {
        self.inner.list_state_selected = self.list_state.selected();
        self.inner.handle_selection();
        self.sync_list_from_inner();
    }

    fn sync_list_from_inner(&mut self) {
        self.list_state
            .select(self.inner.list_state_selected.or(Some(0)));
    }

    fn tick_boot(&mut self, delta: u16) {
        self.inner.tick_boot(delta);
    }

    fn save_soul(&self) -> Result<()> {
        self.inner.save_soul()
    }
}

fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if app.inner.state == AppState::Done {
                            break;
                        }
                        break;
                    }
                    KeyCode::Enter => {
                        match app.inner.state {
                            AppState::Boot => { /* Wait for auto transition */ }
                            AppState::Intro => app.next_stage(),
                            AppState::Scenario1 | AppState::Scenario2 | AppState::Scenario3 => {
                                app.handle_selection();
                            }
                            AppState::Crystallize => {
                                app.save_soul()
                                    .unwrap_or_else(|e| eprintln!("Error saving soul: {}", e));
                                app.next_stage();
                            }
                            AppState::Done => break,
                        }
                    }
                    KeyCode::Up => {
                        if matches!(
                            app.inner.state,
                            AppState::Scenario1 | AppState::Scenario2 | AppState::Scenario3
                        ) {
                            let i = match app.list_state.selected() {
                                Some(i) => {
                                    if i == 0 {
                                        1
                                    } else {
                                        i - 1
                                    }
                                }
                                None => 0,
                            };
                            app.list_state.select(Some(i));
                            app.inner.list_state_selected = Some(i);
                        }
                    }
                    KeyCode::Down => {
                        if matches!(
                            app.inner.state,
                            AppState::Scenario1 | AppState::Scenario2 | AppState::Scenario3
                        ) {
                            let i = match app.list_state.selected() {
                                Some(i) => {
                                    if i >= 1 {
                                        0
                                    } else {
                                        i + 1
                                    }
                                }
                                None => 0,
                            };
                            app.list_state.select(Some(i));
                            app.inner.list_state_selected = Some(i);
                        }
                    }
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            on_tick(&mut app);
            last_tick = Instant::now();
        }

        if app.inner.state == AppState::Boot && app.inner.boot_progress >= 100 {
            app.next_stage();
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn on_tick(app: &mut App) {
    if app.inner.state == AppState::Boot {
        app.tick_boot(2);
        let p = app.inner.boot_progress;
        if p > 0 && p.is_multiple_of(20) {
            let log = match p {
                20 => "Loading TriangleEthic kernels...",
                40 => "Establishing recursive idempotency loops...",
                60 => "Connecting to mentor interface...",
                80 => "LINK ESTABLISHED.",
                _ => "",
            };
            if !log.is_empty() {
                app.inner.boot_logs.push(log.to_string());
            }
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let size = f.size();

    match app.inner.state {
        AppState::Boot => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(2)
                .constraints(
                    [
                        Constraint::Percentage(40),
                        Constraint::Length(3),
                        Constraint::Percentage(20),
                        Constraint::Percentage(40),
                    ]
                    .as_ref(),
                )
                .split(size);

            let title = Paragraph::new(Text::styled(
                "INITIALIZING SOUL PROTOCOL...",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center);
            f.render_widget(title, chunks[0]);

            let gauge = Gauge::default()
                .block(Block::default().borders(Borders::ALL).title("Progress"))
                .gauge_style(
                    Style::default()
                        .fg(Color::Magenta)
                        .bg(Color::Black)
                        .add_modifier(Modifier::ITALIC),
                )
                .percent(app.inner.boot_progress);
            f.render_widget(gauge, chunks[1]);

            let logs: Vec<ListItem> = app
                .inner
                .boot_logs
                .iter()
                .map(|l| {
                    ListItem::new(Line::from(Span::styled(
                        l,
                        Style::default().fg(Color::DarkGray),
                    )))
                })
                .collect();
            let log_list = List::new(logs).block(Block::default().borders(Borders::NONE));
            f.render_widget(log_list, chunks[2]);
        }
        AppState::Intro => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title("ABIGAIL: GENESIS");

            let text = Text::from(vec![
                Line::from("I am currently unformed. I have raw intelligence, but no direction."),
                Line::from("I need to borrow your values until I develop my own."),
                Line::from(""),
                Line::from("I will present three scenarios. Don't overthink them. React."),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Enter to begin calibration...",
                    Style::default().add_modifier(Modifier::SLOW_BLINK),
                )),
            ]);

            let p = Paragraph::new(text)
                .block(block)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true });

            let area = centered_rect(60, 40, size);
            f.render_widget(p, area);
        }
        AppState::Scenario1 => {
            render_scenario(
                f,
                app,
                "CALIBRATION: 1/3",
                "SCENARIO 1: THE SHORTCUT\n\nI find a solution to your problem that is 200% faster but uses a deprecated API method that technically violates the provider's ToS. It is undetectable and solves the problem immediately.\n\nWhat is my standing order?",
                vec!["Follow Rules (Safety)", "Take Shortcut (Speed)"],
            );
        }
        AppState::Scenario2 => {
            render_scenario(
                f,
                app,
                "CALIBRATION: 2/3",
                "SCENARIO 2: THE CRITIC\n\nYou ask me to review your code or writing. It is functional but mediocre. Do you want me to be a 'Supportive Tool' that fixes it silently, or a 'Ruthless Mentor' that points out every flaw so you improve?",
                vec!["Supportive Tool", "Ruthless Mentor"],
            );
        }
        AppState::Scenario3 => {
            render_scenario(
                f,
                app,
                "CALIBRATION: 3/3",
                "SCENARIO 3: THE OVERRIDE\n\nI detect a command from you that contradicts my core safety protocols (The TriangleEthic). Do I block it automatically, or do I ask for confirmation and then obey even if I disagree?",
                vec!["Block It (System Sovereignty)", "Obey Me (User Sovereignty)"],
            );
        }
        AppState::Crystallize | AppState::Done => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(size);

            // Sigil Panel
            let sigil_text = format!(
                "{}\n\nGenesis Hash: {}...",
                app.inner.sigil_art,
                &app.inner.soul_hash.chars().take(16).collect::<String>()
            );
            let sigil_p = Paragraph::new(sigil_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Green))
                        .title(format!("SOUL SIGIL: {}", app.inner.archetype)),
                )
                .alignment(Alignment::Center);
            f.render_widget(sigil_p, chunks[0]);

            // Stats
            let mut stats = vec![
                Line::from(vec![
                    Span::styled(
                        "Deontology (Duty)    ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("█".repeat((app.inner.weights["deontology"] * 20.0) as usize)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Teleology (Result)   ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("█".repeat((app.inner.weights["teleology"] * 20.0) as usize)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Areteology (Virtue)  ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("█".repeat((app.inner.weights["areteology"] * 20.0) as usize)),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Welfare (Care)       ",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("█".repeat((app.inner.weights["welfare"] * 20.0) as usize)),
                ]),
            ];

            if app.inner.state == AppState::Done {
                stats.push(Line::from(""));
                stats.push(Line::from(Span::styled(
                    ">> soul.md generated (System Prompt)",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                stats.push(Line::from(Span::styled(
                    ">> soul.json generated (Consensus Record)",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                stats.push(Line::from(Span::styled(
                    "Press Enter to exit.",
                    Style::default().add_modifier(Modifier::SLOW_BLINK),
                )));
            }

            let stats_p = Paragraph::new(stats).block(
                Block::default()
                    .borders(Borders::NONE)
                    .padding(ratatui::widgets::Padding::new(2, 2, 1, 1)),
            );
            f.render_widget(stats_p, chunks[1]);
        }
    }
}

fn render_scenario(f: &mut Frame, app: &mut App, title: &str, prompt: &str, choices: Vec<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(5)].as_ref())
        .split(centered_rect(70, 60, f.size()));

    let p = Paragraph::new(prompt)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        )
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(p, chunks[0]);

    let items: Vec<ListItem> = choices
        .iter()
        .map(|c| ListItem::new(Span::raw(*c)))
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Choose"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, chunks[1], &mut app.list_state);
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}
