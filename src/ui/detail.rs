//! Detail panel — per-process deep-dive view.
//!
//! Rendered as a bordered block at the bottom of the screen when the user
//! presses Enter on a row in the tree table.  All data comes from
//! `App::detail_info`, which is populated on-demand via `process::collect_detail`
//! only while the panel is visible.

use crate::{app::App, format, process::ProcessDetail};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect, Spacing},
    style::{Color, Modifier, Style},
    symbols::merge::MergeStrategy,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, TitlePosition},
};

pub fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let pid = match app.detail_pid() {
        Some(p) => p,
        None => return,
    };

    // Find the row name for the title while detail_info may still be loading.
    let row_name = app
        .flat_rows()
        .iter()
        .find(|r| r.pid() == pid)
        .map(|r| r.cmdline().to_owned())
        .unwrap_or_else(|| pid.to_string());

    let title = format!(" {} [{}] ", row_name, pid);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .merge_borders(MergeStrategy::Fuzzy)
        .title_position(TitlePosition::Top)
        .title(Span::styled(title, Style::new().fg(Color::White).bold()))
        .title_bottom(Line::from(vec![
            Span::styled(" ⏎", Style::new().fg(Color::Cyan)),
            Span::styled(" close ", Style::new().fg(Color::White)),
        ]));

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

    render_detail_inner(frame, info, inner);
}

/// Render the body of the detail panel once the data is available.
fn render_detail_inner(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    if area.height == 0 {
        return;
    }

    // Top area: info columns + environ strip at the bottom.
    let [cols_area, environ_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

    let [col1, col2, col3] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .spacing(Spacing::Overlap(1))
    .areas(cols_area);

    // Column 1: paths
    let exe_str = info.exe.as_deref().unwrap_or("—");
    let cwd_str = info.cwd.as_deref().unwrap_or("—");
    render_column(
        frame,
        col1,
        "Paths",
        vec![detail_line("Exe: ", exe_str), detail_line("CWD: ", cwd_str)],
    );

    // Column 2: scheduling + file descriptors
    render_column(
        frame,
        col2,
        "Scheduler",
        vec![
            detail_line("Nice: ", &info.nice.to_string()),
            detail_line("Priority: ", &info.priority.to_string()),
            detail_line("FDs: ", &info.fd_count.to_string()),
        ],
    );

    // Column 3: memory + context switches
    let vm_peak = info
        .vm_peak_kb
        .map(|kb| format::format_bytes(kb * 1024))
        .unwrap_or_else(|| "—".to_owned());
    let vm_rss = info
        .vm_rss_kb
        .map(|kb| format::format_bytes(kb * 1024))
        .unwrap_or_else(|| "—".to_owned());
    let ctx_vol = info
        .voluntary_ctxt_switches
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_owned());
    let ctx_nonvol = info
        .nonvoluntary_ctxt_switches
        .map(|n| n.to_string())
        .unwrap_or_else(|| "—".to_owned());
    render_column(
        frame,
        col3,
        "Memory / Ctx",
        vec![
            detail_line("VM Peak: ", &vm_peak),
            detail_line("VM RSS: ", &vm_rss),
            detail_line("Vol ctx: ", &ctx_vol),
            detail_line("Nonvol: ", &ctx_nonvol),
        ],
    );

    // Environ strip: show first few KEY=value pairs on one line.
    render_environ_strip(frame, &info.environ, environ_area);
}

/// Render a single titled column block.
fn render_column(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
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

/// Build a label + value line for a detail column.
fn detail_line(label: &str, contents: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_owned(), Style::new().fg(Color::White)),
        Span::styled(contents.to_owned(), Style::new().fg(Color::White).bold()),
    ])
}

/// Render a condensed KEY=value environ strip.
///
/// Shows as many environment variables as fit on one line. Truncates long
/// values and stops before overflowing the column width.
fn render_environ_strip(frame: &mut Frame, environ: &[(String, String)], area: Rect) {
    if area.width < 10 || environ.is_empty() {
        return;
    }

    // Prefer a handful of well-known, high-signal variables first.
    const PREFERRED: &[&str] = &["SHELL", "HOME", "USER", "LANG", "PATH", "TERM"];

    let mut ordered: Vec<&(String, String)> = PREFERRED
        .iter()
        .filter_map(|key| environ.iter().find(|(k, _)| k.as_str() == *key))
        .collect();
    for pair in environ {
        if !ordered.contains(&pair) {
            ordered.push(pair);
        }
    }

    let max_val_len: usize = 30;
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        " env: ".to_owned(),
        Style::new().fg(Color::White),
    )];
    let mut used: usize = 6; // " env: "
    for (k, v) in &ordered {
        let val = if v.len() > max_val_len {
            format!("{}…", &v[..max_val_len])
        } else {
            v.clone()
        };
        let entry = format!("{}={}", k, val);
        let sep = if spans.len() > 1 { "  " } else { "" };
        let needed = sep.len() + entry.len();
        if used + needed > area.width as usize {
            break;
        }
        if !sep.is_empty() {
            spans.push(Span::styled(
                sep.to_owned(),
                Style::new().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            format!("{}=", k),
            Style::new().fg(Color::White),
        ));
        spans.push(Span::styled(val, Style::new().fg(Color::White).bold()));
        used += needed;
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
