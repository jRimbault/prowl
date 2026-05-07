//! Process detail layout.
//!
//! Three side-by-side bordered columns (Identity / Memory / Scheduler),
//! a one-line "Activity" summary that emphasises live, debugger-flavoured
//! signals (wchan, page faults, context switches), and the existing env
//! strip pinned to the bottom.
//!
//! Field selection is biased toward signals a developer monitoring their
//! own process actually wants to glance at: the wchan name shows where
//! the kernel parked the task; FD usage is paired with its NOFILE soft
//! limit; major page faults indicate disk-backed memory pressure; the
//! cgroup path identifies systemd slices and container scopes.

use super::{
    DASH, detail_line, detail_line_styled, fd_usage_color, format_count, format_kb,
    oom_score_color, render_column,
};
use crate::{format, process::ProcessDetail};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect, Spacing},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

const COL_GUTTER: usize = 11;

pub fn render(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    if area.height == 0 {
        return;
    }

    // Vertical layout: three columns of context block, one activity row,
    // one environ strip.  The activity row carries the live debug-flavour
    // counters that change every tick — keeping them on a single row
    // keeps them visually distinct from the more static column blocks.
    let [columns_area, activity_area, environ_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let [col_identity, col_memory, col_scheduler] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .spacing(Spacing::Overlap(1))
    .areas(columns_area);

    render_identity(frame, info, col_identity);
    render_memory(frame, info, col_memory);
    render_scheduler(frame, info, col_scheduler);
    render_activity(frame, info, activity_area);
    render_environ_strip(frame, &info.environ, environ_area);
}

fn render_identity(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    let exe = info.exe.as_deref().unwrap_or(DASH);
    let cwd = info.cwd.as_deref().unwrap_or(DASH);
    let cgroup = info.cgroup.as_deref().unwrap_or(DASH);

    render_column(
        frame,
        area,
        "Identity",
        vec![
            detail_line("Exe:", COL_GUTTER, exe),
            detail_line("CWD:", COL_GUTTER, cwd),
            detail_line("Cgroup:", COL_GUTTER, cgroup),
            detail_line(
                "Threads:",
                COL_GUTTER,
                info.thread_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| DASH.to_owned()),
            ),
        ],
    );
}

fn render_memory(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    render_column(
        frame,
        area,
        "Memory",
        vec![
            detail_line("Virtual:", COL_GUTTER, format_kb(info.vm_size_kb)),
            detail_line("Resident:", COL_GUTTER, format_kb(info.vm_rss_kb)),
            detail_line("Peak RSS:", COL_GUTTER, format_kb(info.vm_hwm_kb)),
            detail_line("Heap:", COL_GUTTER, format_kb(info.vm_data_kb)),
            detail_line("Stack:", COL_GUTTER, format_kb(info.vm_stack_kb)),
            detail_line("Swap:", COL_GUTTER, format_kb(info.vm_swap_kb)),
        ],
    );
}

fn render_scheduler(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    let policy = info
        .policy
        .map(|p| p.label())
        .unwrap_or_else(|| DASH.to_owned());
    let nice_prio = format!("{} / {}", info.nice, info.priority);
    let last_cpu = info
        .last_cpu
        .map(|c| c.to_string())
        .unwrap_or_else(|| DASH.to_owned());

    let fd_text = match info.fd_soft_limit {
        Some(limit) => format!("{} / {}", info.fd_count, limit),
        None => format!("{} / ∞", info.fd_count),
    };
    let fd_color = fd_usage_color(info.fd_count, info.fd_soft_limit);

    let oom_text = match (info.oom_score, info.oom_score_adj) {
        (Some(score), Some(adj)) => format!("{} ({:+})", score, adj),
        (Some(score), None) => score.to_string(),
        _ => DASH.to_owned(),
    };
    let oom_color = info.oom_score.map(oom_score_color).unwrap_or(Color::White);

    render_column(
        frame,
        area,
        "Scheduler",
        vec![
            detail_line("Policy:", COL_GUTTER, policy),
            detail_line("Nice/Pri:", COL_GUTTER, nice_prio),
            detail_line("Last CPU:", COL_GUTTER, last_cpu),
            detail_line_styled("FDs:", COL_GUTTER, fd_text, Style::new().fg(fd_color)),
            detail_line_styled("OOM:", COL_GUTTER, oom_text, Style::new().fg(oom_color)),
        ],
    );
}

/// One-line activity strip: wchan + page faults + context switches.
///
/// These fields change tick-to-tick.  Keeping them on a single line makes
/// the live churn legible without polluting the static column blocks.
fn render_activity(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let wchan = info.wchan.as_deref().unwrap_or("running");
    let wchan_style = if info.wchan.is_some() {
        Style::new().fg(Color::Cyan).bold()
    } else {
        Style::new().fg(Color::Green).bold()
    };

    let major_style = if info.major_faults > 0 {
        Style::new().fg(Color::Yellow).bold()
    } else {
        Style::new().fg(Color::White).bold()
    };

    let cpu_total = info.user_cpu_time.saturating_add(info.system_cpu_time);

    let spans = vec![
        Span::styled(" Wchan: ", Style::new().fg(Color::White)),
        Span::styled(wchan.to_owned(), wchan_style),
        sep(),
        Span::styled("Faults min/maj: ", Style::new().fg(Color::White)),
        Span::styled(
            format_count(info.minor_faults),
            Style::new().fg(Color::White).bold(),
        ),
        Span::styled("/", Style::new().fg(Color::DarkGray)),
        Span::styled(format_count(info.major_faults), major_style),
        sep(),
        Span::styled("Ctx v/n: ", Style::new().fg(Color::White)),
        Span::styled(
            format!(
                "{}/{}",
                info.voluntary_ctxt_switches
                    .map(format_count)
                    .unwrap_or_else(|| DASH.to_owned()),
                info.nonvoluntary_ctxt_switches
                    .map(format_count)
                    .unwrap_or_else(|| DASH.to_owned()),
            ),
            Style::new().fg(Color::White).bold(),
        ),
        sep(),
        Span::styled("CPU u+s: ", Style::new().fg(Color::White)),
        Span::styled(
            format::format_duration(cpu_total),
            Style::new().fg(Color::White).bold(),
        ),
    ];

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let mut used: usize = 6;
    for (k, v) in &ordered {
        let val = if v.len() > max_val_len {
            format!("{}…", &v[..max_val_len])
        } else {
            v.clone()
        };
        let entry = format!("{}={}", k, val);
        let sep_str = if spans.len() > 1 { "  " } else { "" };
        let needed = sep_str.len() + entry.len();
        if used + needed > area.width as usize {
            break;
        }
        if !sep_str.is_empty() {
            spans.push(Span::styled(
                sep_str.to_owned(),
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

/// Vertical separator span used to visually break the activity row.
fn sep() -> Span<'static> {
    Span::styled("  │  ", Style::new().fg(Color::DarkGray))
}

#[cfg(test)]
mod tests {
    use crate::test_support;
    use insta::assert_snapshot;

    #[test]
    fn renders_process_layout() {
        let info = test_support::make_process_detail();
        let frame = test_support::render_widget(100, 11, |frame, area| {
            super::render(frame, &info, area);
        });
        assert_snapshot!(frame);
    }
}
