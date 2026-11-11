//! rstask-tui — minimal kanban board over the shared core.
//!
//! Skeleton scope: four workflow columns (queue / work / review / done),
//! refresh on `r`, quit on `q`. Tree/board/wip-limit views come later.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;
use rstask_core::{Config, Store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::load(None)?;
    let store = Arc::new(Store::open(&cfg.storage.db_path).await?);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, store.clone()).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: Arc<Store>,
) -> anyhow::Result<()> {
    let mut board = store.board_state().await?;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                    Constraint::Percentage(25),
                ])
                .split(f.area());

            let colors = [Color::Gray, Color::Blue, Color::Yellow, Color::Green];
            for (((status, tasks), area), color) in
                board.iter().zip(chunks.iter()).zip(colors.iter())
            {
                let items: Vec<ListItem> = tasks
                    .iter()
                    .map(|t| {
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                format!("{:<8} ", &t.id[..8.min(t.id.len())]),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::raw(&t.title),
                        ]))
                    })
                    .collect();
                let widget = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(*color))
                            .title(format!(" {status} ({}) ", tasks.len())),
                    )
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD));
                f.render_widget(widget, *area);
            }

            let hint = Paragraph::new("q quit · r refresh");
            f.render_widget(hint, f.area());
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => {
                        board = store.board_state().await?;
                    }
                    _ => {}
                }
            }
        }
    }
}
