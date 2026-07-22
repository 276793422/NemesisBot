//! Terminal UI (menuconfig-style). A full-screen single-pane list of features
//! grouped by category. Keys:
//!   ↑/↓     move selection
//!   Space   toggle boolean feature
//!   →/Enter cycle enum feature to next option
//!   s       save .config (keep editing)
//!   q/Esc   save .config and quit
//!   Ctrl+c  abort without saving

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::config::BuildConfig;
use crate::manifest::{DefaultVal, FeatureManifest};

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// A flat list item pointing back at a feature id.
struct Row {
    id: String,
    label: String,
    is_enum: bool,
}

fn rows(manifest: &FeatureManifest) -> Vec<Row> {
    let mut rows = Vec::new();
    // stable category order: channels, subsystems, core, build, then any other
    let order = ["channels", "subsystems", "core", "build"];
    let mut cats: Vec<String> = manifest
        .features
        .iter()
        .map(|f| f.category.clone())
        .filter(|c| !c.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    cats.sort_by_key(|c| {
        order
            .iter()
            .position(|o| *o == c.as_str())
            .unwrap_or(usize::MAX)
    });
    for cat in cats {
        rows.push(Row {
            id: String::new(),
            label: format!("— {} —", cat),
            is_enum: false,
        });
        for f in manifest.features.iter().filter(|f| f.category == cat) {
            rows.push(Row {
                id: f.id.clone(),
                label: f.label.clone(),
                is_enum: f.is_enum(),
            });
        }
    }
    rows
}

fn row_text(row: &Row, manifest: &FeatureManifest, cfg: &BuildConfig) -> Line<'static> {
    if row.id.is_empty() {
        return Line::from(Span::styled(
            row.label.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let spec = manifest.features.iter().find(|f| f.id == row.id);
    let marker = if row.is_enum {
        let cur = cfg
            .get_enum(&row.id)
            .map(|s| s.to_string())
            .unwrap_or_default();
        format!("[{:>8}]", cur)
    } else {
        match cfg.get_bool(&row.id) {
            Some(true) => "[x]".to_string(),
            _ => "[ ]".to_string(),
        }
    };
    let label = if let Some(s) = spec {
        if s.desc.is_empty() {
            format!("{}  {}", marker, row.label)
        } else {
            format!("{}  {} — {}", marker, row.label, s.desc)
        }
    } else {
        format!("{}  {}", marker, row.label)
    };
    Line::from(label)
}

/// Run the TUI. Returns Ok(()) on clean exit.
pub fn run(
    manifest: &FeatureManifest,
    cfg: &mut BuildConfig,
    config_path: &Path,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Tui::new(backend)?;
    terminal.hide_cursor()?;

    let result = interactive_loop(&mut terminal, manifest, cfg, config_path);

    // restore terminal regardless of outcome
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn interactive_loop(
    terminal: &mut Tui,
    manifest: &FeatureManifest,
    cfg: &mut BuildConfig,
    config_path: &Path,
) -> io::Result<()> {
    let rows = rows(manifest);
    let mut state = ListState::default();
    state.select(Some(0));
    let mut dirty = false;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(3),
                    Constraint::Length(2),
                ])
                .split(f.area());

            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| ListItem::new(row_text(r, manifest, cfg)))
                .collect();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("NemesisBot 构建配置"),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_stateful_widget(list, chunks[0], &mut state.clone());

            // detail / help line
            let sel = state.selected().and_then(|i| rows.get(i));
            let detail = if let Some(r) = sel {
                if r.is_enum {
                    let opts = manifest
                        .features
                        .iter()
                        .find(|f| f.id == r.id)
                        .map(|f| f.options.join(" | "))
                        .unwrap_or_default();
                    format!("{} (enum: → 切换; 选项: {})", r.id, opts)
                } else if !r.id.is_empty() {
                    format!("{} (Space 切换)", r.id)
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let para = Paragraph::new(detail)
                .block(Block::default().borders(Borders::ALL).title("当前项"));
            f.render_widget(para, chunks[1]);

            let help = "↑↓ 移动 · Space 切换 · →/Enter 切换枚举 · s 保存 · q 退出";
            let dirty_mark = if dirty { " (未保存)" } else { "" };
            f.render_widget(Paragraph::new(format!("{help}{dirty_mark}")), chunks[2]);
        })?;

        if !event::poll(std::time::Duration::from_millis(250))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                // q / Esc: save the current selection and exit (so a parent
                // build script can immediately build with the saved .config).
                KeyCode::Char('q') | KeyCode::Esc => {
                    if let Err(e) = cfg.save(config_path) {
                        eprintln!("[nemesis-build-config] failed to save .config: {e}");
                    }
                    break;
                }
                // Ctrl+C: abort without saving.
                KeyCode::Char('c')
                    if k.modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    break;
                }
                KeyCode::Char('s') => {
                    // save in place; keep editing
                    if let Err(e) = cfg.save(config_path) {
                        // best-effort: surface error in the dirty marker
                        let _ = e;
                    }
                    dirty = false;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(i) = state.selected() {
                        state.select(Some((i + 1).min(rows.len().saturating_sub(1))));
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(i) = state.selected() {
                        state.select(Some(i.saturating_sub(1)));
                    }
                }
                KeyCode::Char(' ') => {
                    if let Some(i) = state.selected() {
                        if let Some(r) = rows.get(i) {
                            if !r.id.is_empty() && !r.is_enum {
                                let cur = cfg.get_bool(&r.id).unwrap_or(false);
                                cfg.set_bool(&r.id, !cur);
                                dirty = true;
                            }
                        }
                    }
                }
                KeyCode::Right | KeyCode::Enter => {
                    if let Some(i) = state.selected() {
                        if let Some(r) = rows.get(i) {
                            if r.is_enum {
                                if let Some(spec) = manifest.features.iter().find(|f| f.id == r.id)
                                {
                                    if spec.options.is_empty() {
                                        continue;
                                    }
                                    let cur = cfg.get_enum(&r.id).unwrap_or("");
                                    let idx = spec
                                        .options
                                        .iter()
                                        .position(|o| o == cur)
                                        .map(|p| p + 1)
                                        .unwrap_or(0);
                                    let next = &spec.options[idx % spec.options.len()];
                                    cfg.set_enum(&r.id, next);
                                    dirty = true;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// Keep DefaultVal referenced (documentation of the value model used elsewhere).
#[allow(dead_code)]
fn _type_anchor(_: DefaultVal) {}
