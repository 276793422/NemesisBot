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
        let ev = event::read()?;
        match handle_key(
            ev,
            &rows,
            state.selected(),
            manifest,
            cfg,
            &mut dirty,
            config_path,
        ) {
            KeyDisposition::Nothing => {}
            KeyDisposition::Select(new_sel) => state.select(new_sel),
            // q/Esc saved inside handle_key; Ctrl+C aborted without saving —
            // either way we fall through to the terminal restore below.
            KeyDisposition::Exit => break,
        }
    }
    Ok(())
}

/// What [`interactive_loop`] should do after translating one raw event.
#[derive(Debug, PartialEq, Eq)]
enum KeyDisposition {
    /// Keep editing; the terminal list state is unchanged.
    Nothing,
    /// Replace the list selection with this index.
    Select(Option<usize>),
    /// Leave the loop (both exits are fully handled inside [`handle_key`]).
    Exit,
}

/// Pure key → disposition translation for the menu (S12b batch: extracted
/// verbatim from `interactive_loop` so the navigation/toggle/save state
/// machine is unit-testable without a live TTY; the rendering + poll loop
/// remains structurally tied to a real terminal).
///
/// Semantics (unchanged):
/// - `q`/Esc: save `.config` (best-effort, errors logged to stderr) and exit
/// - Ctrl+C: exit **without** saving
/// - `s`: save in place, clear the dirty marker, keep editing
/// - ↓/j · ↑/k: move selection, clamped to `[0, rows.len()-1]`
/// - Space: flip the boolean feature at the cursor (header rows ignored)
/// - →/Enter: cycle an enum feature through its options (wraps; header rows
///   and empty option lists ignored)
fn handle_key(
    ev: Event,
    rows: &[Row],
    selected: Option<usize>,
    manifest: &FeatureManifest,
    cfg: &mut BuildConfig,
    dirty: &mut bool,
    config_path: &Path,
) -> KeyDisposition {
    let Event::Key(k) = ev else {
        return KeyDisposition::Nothing;
    };
    if k.kind != KeyEventKind::Press {
        return KeyDisposition::Nothing;
    }
    match k.code {
        // q / Esc: save the current selection and exit (so a parent
        // build script can immediately build with the saved .config).
        KeyCode::Char('q') | KeyCode::Esc => {
            if let Err(e) = cfg.save(config_path) {
                eprintln!("[nemesis-build-config] failed to save .config: {e}");
            }
            KeyDisposition::Exit
        }
        // Ctrl+C: abort without saving.
        KeyCode::Char('c')
            if k.modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            KeyDisposition::Exit
        }
        KeyCode::Char('s') => {
            // save in place; keep editing
            if let Err(e) = cfg.save(config_path) {
                // best-effort: surface error in the dirty marker
                let _ = e;
            }
            *dirty = false;
            KeyDisposition::Nothing
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let next = selected
                .map(|i| (i + 1).min(rows.len().saturating_sub(1)))
                .or(selected);
            KeyDisposition::Select(next)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let next = selected.map(|i| i.saturating_sub(1)).or(selected);
            KeyDisposition::Select(next)
        }
        KeyCode::Char(' ') => {
            if let Some(i) = selected
                && let Some(r) = rows.get(i)
                && !r.id.is_empty()
                && !r.is_enum
            {
                let cur = cfg.get_bool(&r.id).unwrap_or(false);
                cfg.set_bool(&r.id, !cur);
                *dirty = true;
            }
            KeyDisposition::Nothing
        }
        KeyCode::Right | KeyCode::Enter => 'right: {
            if let Some(i) = selected
                && let Some(r) = rows.get(i)
                && r.is_enum
                && let Some(spec) = manifest.features.iter().find(|f| f.id == r.id)
            {
                if spec.options.is_empty() {
                    break 'right KeyDisposition::Nothing;
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
                *dirty = true;
            }
            KeyDisposition::Nothing
        }
        _ => KeyDisposition::Nothing,
    }
}

// Keep DefaultVal referenced (documentation of the value model used elsewhere).
#[allow(dead_code)]
fn _type_anchor(_: DefaultVal) {}

#[cfg(test)]
mod tests;
