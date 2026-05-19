//! Thread detail layout.
//!
//! Threads share most of their context with the parent process (memory,
//! file descriptors, environ, cgroup), so showing those fields again at
//! the thread scope only adds noise.  This layout instead foregrounds
//! the per-task signals that vary thread-by-thread:
//!
//! - **Identity** — thread group it belongs to, raw nice/priority.
//! - **Scheduler** — policy, RT priority, last CPU it ran on.
//! - **Activity** — wchan (where the kernel parked it), CPU split into
//!   user / system time, page faults, and context switches.

use super::{DASH, detail_line, detail_line_styled, format_count, render_column};
use crate::{format, process::ProcessDetail};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect, Spacing},
    style::{Color, Style},
    widgets::Paragraph,
};

const COL_GUTTER: usize = 11;

pub fn render(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    if area.height == 0 {
        return;
    }

    let [col_identity, col_scheduler, col_activity] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .spacing(Spacing::Overlap(1))
    .areas(area);

    render_identity(frame, info, col_identity);
    render_scheduler(frame, info, col_scheduler);
    render_activity(frame, info, col_activity);
}

fn render_identity(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    render_column(
        frame,
        area,
        "Identity",
        vec![
            detail_line("TID:", COL_GUTTER, info.pid.to_string()),
            detail_line("TGID:", COL_GUTTER, info.tgid.to_string()),
            detail_line("Nice:", COL_GUTTER, info.nice.to_string()),
            detail_line("Priority:", COL_GUTTER, info.priority.to_string()),
        ],
    );
}

fn render_scheduler(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    let policy = info
        .policy
        .map(|p| p.label())
        .unwrap_or_else(|| DASH.to_owned());
    let rt_prio = info
        .rt_priority
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .unwrap_or_else(|| DASH.to_owned());
    let last_cpu = info
        .last_cpu
        .map(|c| c.to_string())
        .unwrap_or_else(|| DASH.to_owned());

    render_column(
        frame,
        area,
        "Scheduler",
        vec![
            detail_line("Policy:", COL_GUTTER, policy),
            detail_line("RT prio:", COL_GUTTER, rt_prio),
            detail_line("Last CPU:", COL_GUTTER, last_cpu),
            detail_line(
                "User CPU:",
                COL_GUTTER,
                format::format_duration(info.user_cpu_time),
            ),
            detail_line(
                "Sys CPU:",
                COL_GUTTER,
                format::format_duration(info.system_cpu_time),
            ),
        ],
    );
}

fn render_activity(frame: &mut Frame, info: &ProcessDetail, area: Rect) {
    // Wchan — the headline field for thread debugging.
    let wchan = info.wchan.as_deref().unwrap_or("running");
    let wchan_style = if info.wchan.is_some() {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(format::GREEN)
    };

    // Major-fault accent: any non-zero count is worth eyeing.
    let major_style = if info.major_faults > 0 {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().fg(Color::White)
    };

    let lines = vec![
        detail_line_styled("Wchan:", COL_GUTTER, wchan, wchan_style),
        detail_line("MinFlt:", COL_GUTTER, format_count(info.minor_faults)),
        detail_line_styled(
            "MajFlt:",
            COL_GUTTER,
            format_count(info.major_faults),
            major_style,
        ),
        detail_line(
            "Ctx vol:",
            COL_GUTTER,
            info.voluntary_ctxt_switches
                .map(format_count)
                .unwrap_or_else(|| DASH.to_owned()),
        ),
        detail_line(
            "Ctx nonvol:",
            COL_GUTTER,
            info.nonvoluntary_ctxt_switches
                .map(format_count)
                .unwrap_or_else(|| DASH.to_owned()),
        ),
    ];

    // Render directly without an outer block when the area is too short
    // to host a bordered column block, so users on small terminals still
    // see the fields.
    if area.height >= 4 {
        render_column(frame, area, "Activity", lines);
    } else {
        frame.render_widget(Paragraph::new(lines), area);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support;
    use insta::assert_snapshot;

    #[test]
    fn renders_thread_layout() {
        let info = test_support::make_thread_detail();
        let frame = test_support::render_widget(100, 8, |frame, area| {
            super::render(frame, &info, area);
        });
        assert_snapshot!(frame);
    }
}
