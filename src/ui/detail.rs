//! Detail panel — per-task deep-dive view.
//!
//! Two specialised layouts share the same outer chrome:
//!
//! - `process` (default) — multi-column per-process view: paths, full
//!   memory breakdown, scheduler, runtime activity, environ strip.
//! - `thread` — narrower per-task view that omits process-wide fields
//!   (memory, FDs, environ — those belong to the parent process) and
//!   focuses on per-thread scheduling and activity counters.
//!
//! Layout dispatch is driven by `ProcessDetail::is_thread`, computed from
//! `tgid != pid` in `process::collect_detail`.

mod process;
mod thread;

use crate::{app::App, format, process::ProcessDetail};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::merge::MergeStrategy,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, TitlePosition},
};

/// Outer height for the panel when showing a full process.
pub const PROCESS_HEIGHT: u16 = 13;

/// Outer height for the panel when showing a single thread.
pub const THREAD_HEIGHT: u16 = 10;

/// Pick the detail-panel height for the currently-selected detail target.
///
/// Threads get a shorter panel because they intentionally omit the
/// memory / FDs / environ rows that processes carry.
pub fn detail_height(app: &App) -> u16 {
    let Some(pid) = app.detail_pid() else {
        return PROCESS_HEIGHT;
    };
    // Prefer the freshly-fetched info; fall back to the row's is_thread
    // bit so the panel sizes correctly even before the first detail load.
    if let Some(info) = app.detail_info() {
        return if info.is_thread {
            THREAD_HEIGHT
        } else {
            PROCESS_HEIGHT
        };
    }
    let is_thread = app
        .flat_rows()
        .iter()
        .find(|r| r.pid() == pid)
        .map(|r| r.is_thread())
        .unwrap_or(false);
    if is_thread {
        THREAD_HEIGHT
    } else {
        PROCESS_HEIGHT
    }
}

pub fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let Some(pid) = app.detail_pid() else { return };

    let row = app.flat_rows().iter().find(|r| r.pid() == pid);
    let row_label = row
        .map(|r| r.cmdline().to_owned())
        .unwrap_or_else(|| pid.to_string());
    let row_state = row.map(|r| r.state());
    let row_is_thread = row.map(|r| r.is_thread()).unwrap_or(false);

    let block = outer_block(&row_label, pid, row_is_thread, row_state, app.detail_info());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(info) = app.detail_info() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " loading…",
                Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM),
            )),
            inner,
        );
        return;
    };

    if info.is_thread {
        thread::render(frame, info, inner);
    } else {
        process::render(frame, info, inner);
    }
}

/// Build the outer panel block: rounded border, name + state badge title
/// on top, a small shortcut hint on the bottom.
fn outer_block(
    label: &str,
    pid: crate::process::Pid,
    is_thread: bool,
    row_state: Option<char>,
    info: Option<&ProcessDetail>,
) -> Block<'static> {
    // Prefer the freshly-fetched detail state (truth) over the cached row
    // state (which may lag a tick on context switches).
    let state = info.map(|i| i.state).or(row_state);
    let kind = if is_thread { "thread" } else { "process" };

    let mut title_spans = vec![Span::styled(
        format!(" {} ", label),
        Style::new().fg(Color::White).bold(),
    )];
    title_spans.push(Span::styled(
        format!("[{}] ", pid),
        Style::new().fg(Color::DarkGray),
    ));
    title_spans.push(Span::styled(
        format!("{} ", kind),
        Style::new().fg(Color::DarkGray),
    ));
    if let Some(c) = state {
        title_spans.push(Span::styled(
            "· ".to_owned(),
            Style::new().fg(Color::DarkGray),
        ));
        title_spans.push(Span::styled(
            format!("● {} ", format::state_word(c)),
            Style::new().fg(state_color(c)).bold(),
        ));
    }

    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .merge_borders(MergeStrategy::Fuzzy)
        .title_position(TitlePosition::Top)
        .title(Line::from(title_spans))
        .title_bottom(
            Line::from(vec![
                Span::styled(" ⏎", Style::new().fg(Color::Cyan)),
                Span::styled(" close ", Style::new().fg(Color::White)),
            ])
            .right_aligned(),
        )
}

/// Map a kernel state character to a display colour.  Aligns with the
/// htop/btop convention: green = active, yellow = sleeping, magenta/red
/// = abnormal / pending cleanup.
fn state_color(state: char) -> Color {
    match state {
        'R' => format::GREEN,
        'S' | 'I' => Color::Yellow,
        'D' | 'K' | 'W' => Color::Magenta,
        'T' | 't' => Color::Cyan,
        'Z' | 'X' | 'x' => Color::Red,
        _ => Color::White,
    }
}

// --- Shared helpers used by the per-layout submodules. ---

/// Render a titled bordered column block holding a stack of lines.
pub(super) fn render_column(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .merge_borders(MergeStrategy::Fuzzy)
        .border_style(Style::new().fg(Color::DarkGray))
        .title_position(TitlePosition::Top)
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(Color::White).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Build a label + value line, with the label right-padded to a column
/// gutter width so colons line up across rows.
pub(super) fn detail_line(label: &str, gutter: usize, value: impl Into<String>) -> Line<'static> {
    let padded = format!("{:<gutter$}", label, gutter = gutter);
    Line::from(vec![
        Span::styled(padded, Style::new().fg(Color::White)),
        Span::styled(value.into(), Style::new().fg(Color::White).bold()),
    ])
}

/// Build a label + styled value line — for fields where the value carries
/// extra meaning (warnings, traffic-light thresholds, accent colour).
pub(super) fn detail_line_styled(
    label: &str,
    gutter: usize,
    value: impl Into<String>,
    value_style: Style,
) -> Line<'static> {
    let padded = format!("{:<gutter$}", label, gutter = gutter);
    Line::from(vec![
        Span::styled(padded, Style::new().fg(Color::White)),
        Span::styled(value.into(), value_style.add_modifier(Modifier::BOLD)),
    ])
}

/// Default placeholder for missing optional values.
pub(super) const DASH: &str = "—";

/// Format an `Option<kilobytes>` as a human-readable byte count, or `—`.
pub(super) fn format_kb(kb: Option<u64>) -> String {
    kb.map(|n| format::format_bytes(n.saturating_mul(1024)))
        .unwrap_or_else(|| DASH.to_owned())
}

/// Compact decimal count: `1.2k` / `3.4M` / `1.0G`, mirroring `format_bytes`'s
/// scale labels but without the byte unit.  Used for fault counters where a
/// raw integer is hard to scan at a glance.
pub(super) fn format_count(n: u64) -> String {
    const K: u64 = 1_000;
    const M: u64 = K * 1_000;
    const G: u64 = M * 1_000;
    match n {
        x if x < K => x.to_string(),
        x if x < M => format!("{:.1}k", x as f64 / K as f64),
        x if x < G => format!("{:.1}M", x as f64 / M as f64),
        x => format!("{:.1}G", x as f64 / G as f64),
    }
}

/// Choose a colour for an FD-usage ratio: green / yellow / red as the
/// process approaches its NOFILE soft limit.
pub(super) fn fd_usage_color(used: usize, soft_limit: Option<u64>) -> Color {
    let Some(limit) = soft_limit.filter(|n| *n > 0) else {
        return Color::White;
    };
    let ratio = used as f64 / limit as f64;
    if ratio >= 0.8 {
        Color::Red
    } else if ratio >= 0.5 {
        Color::Yellow
    } else {
        format::GREEN
    }
}

/// OOM score colour: low = neutral, mid = yellow, high = red.
/// `oom_score` is 0..=1000 in current kernels; 500+ is concerning.
pub(super) fn oom_score_color(score: u16) -> Color {
    match score {
        0..=200 => format::GREEN,
        201..=500 => Color::White,
        501..=800 => Color::Yellow,
        _ => Color::Red,
    }
}
