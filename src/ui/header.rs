//! Header panel — btop-style detail view for the root process.
//!
//! Layout (inner area, 7 rows for a Length(9) outer block):
//!
//!   rows 0-5     │ [CPU graph panel]  │  [info column blocks]
//!   row 6        │ full command line
//!
//! The CPU panel renders a tall multi-row braille graph (6 rows = 24 vertical
//! levels) with a narrow label column showing the current percentage and the
//! vertical "C P U" label.  The info panel organises values into four bordered
//! column blocks (Process, Time, I/O, Tree) — each block's title provides
//! context so individual labels can be short and unambiguous.

use crate::{
    app::{App, History},
    format,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect, Spacing},
    style::{Color, Style},
    symbols::merge::MergeStrategy,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, TitlePosition},
};

pub fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " {} ",
        if app.name().is_empty() {
            "prowl"
        } else {
            app.name()
        }
    );
    let Some(root) = app.root() else {
        frame.render_widget(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::DarkGray))
                .title(Span::styled(title, Style::new().fg(Color::White).bold())),
            area,
        );
        return;
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .merge_borders(MergeStrategy::Exact)
        .title(Span::styled(
            format!(" {} ", root.name()),
            Style::new().fg(Color::White).bold(),
        ))
        .title(polling_rate_title(app.interval()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // let [main_area, cmd_row] =
    //     Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    // CPU graph panel (left) | info column blocks (right).
    let [cpu_panel, info_panel] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(inner);

    render_cpu_panel(
        frame,
        cpu_panel,
        app.cpu_history(),
        root.cpu_pct(),
        app.cpu_count(),
    );
    render_info_panel(frame, info_panel, root, app.mem_history());
}

/// Render the left CPU panel: narrow label column + tall braille graph.
///
/// Row 0 shows the current percentage; rows 1-3 show the "C", "P", "U" label
/// letters; remaining rows are blank.  The graph fills all rows from the bottom
/// upward so the trace reads as a filled area chart.
fn render_cpu_panel(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    cpu_pct: crate::format::Percent,
    cpu_count: usize,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    const LABEL_W: u16 = 7;
    let [label_col, graph_col] =
        Layout::horizontal([Constraint::Length(LABEL_W), Constraint::Fill(1)]).areas(area);

    let rows = graph_col.height as usize;
    // Lock the graph's y-axis to total CPU capacity (cpu_count * 100 %) so the
    // visual height represents a stable fraction of the machine rather than
    // auto-rescaling whenever the recent local maximum changes.
    let max_pct = cpu_count as f64 * 100.0;
    let graph_rows =
        format::braille_graph_multi(history.iter(), graph_col.width as usize, rows, max_pct);
    let cpu_color = cpu_pct.color_scaled(max_pct);
    const ROW_LABELS: [&str; 5] = ["", "", "C", "P", "U"];

    for r in 0..rows {
        let y = area.top() + r as u16;
        if y >= area.bottom() {
            break;
        }

        let label_text = if r == 0 {
            format!("{:>5.1}% ", cpu_pct.value())
        } else {
            format!("{:<7}", ROW_LABELS.get(r).copied().unwrap_or(""))
        };
        let label_style = if r == 0 {
            Style::new().fg(cpu_color).bold()
        } else {
            Style::new().fg(Color::White)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(label_text, label_style)),
            Rect::new(label_col.left(), y, label_col.width, 1),
        );

        if let Some(row_str) = graph_rows.get(r) {
            frame.render_widget(
                Paragraph::new(Span::styled(row_str.as_str(), Style::new().fg(cpu_color))),
                Rect::new(graph_col.left(), y, graph_col.width, 1),
            );
        }
    }
}

/// Render the right info panel as four bordered column blocks + a memory
/// sparkline row beneath them.
fn render_info_panel(
    frame: &mut Frame,
    area: Rect,
    root: &crate::process::Node,
    mem_history: &History,
) {
    if area.height < 2 {
        return;
    }

    let [columns_area, mem_row] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

    let [col1, col2, col3, col4] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .spacing(Spacing::Overlap(1))
    .areas(columns_area);

    render_column(
        frame,
        col1,
        "Process",
        vec![
            info_line("Status: ", 8, format::state_word(root.state())),
            info_line("Parent: ", 8, root.parent_name()),
            info_line("User: ", 8, root.user()),
        ],
    );

    render_column(
        frame,
        col2,
        "Time",
        vec![
            info_line("Elapsed: ", 9, format::format_duration(root.elapsed())),
            info_line("CPU: ", 9, format::format_duration(root.cpu_time())),
            info_line(
                "Subproc: ",
                9,
                format::format_duration(root.subprocess_cpu_time()),
            ),
        ],
    );

    render_column(
        frame,
        col3,
        "I/O",
        vec![
            info_line("Read: ", 7, format::format_bytes(root.io().read())),
            info_line("Write: ", 7, format::format_bytes(root.io().write())),
        ],
    );

    render_column(
        frame,
        col4,
        "Tree",
        vec![
            info_line("Threads: ", 10, root.thread_count().to_string()),
            info_line("Children: ", 10, root.subprocess_count().to_string()),
        ],
    );

    // Memory sparkline spanning full width beneath the column blocks.
    render_sparkline_row(
        frame,
        mem_row,
        &format!("  Memory {:>6.1}%  ", root.mem_pct().value()),
        mem_history,
        Color::Green,
        &format!("  {}", format::format_bytes(root.mem_rss_bytes())),
    );
}

/// Render a single column group as a bordered block with a title.
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

/// Build a single labelled field line for a column block.
///
/// `name_width` is the maximum label width within this column so colons
/// align vertically across rows.
fn info_line(name: &str, name_width: usize, contents: impl Into<String>) -> Line<'static> {
    let padded = format!("{:>width$}", name, width = name_width);
    Line::from(vec![label(&padded), value(&contents.into())])
}

/// Render a 1-row braille graph between a left label and a right label.
fn render_sparkline_row(
    frame: &mut Frame,
    area: Rect,
    left_label: &str,
    history: &History,
    color: Color,
    right_label: &str,
) {
    let right_width = right_label.len() as u16;
    let left_width = left_label.len() as u16;

    let [left, spark, right] = Layout::horizontal([
        Constraint::Length(left_width),
        Constraint::Fill(1),
        Constraint::Length(right_width),
    ])
    .areas(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            left_label.to_owned(),
            Style::new().fg(Color::White),
        )),
        left,
    );

    // Decouple format from app: pass the iterator of Percent directly.
    let graph = format::braille_graph(history.iter(), spark.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(graph, Style::new().fg(color))),
        spark,
    );

    if !right_label.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                right_label.to_owned(),
                Style::new().fg(Color::White).bold(),
            )),
            right,
        );
    }
}

fn label(s: &str) -> Span<'static> {
    Span::styled(s.to_owned(), Style::new().fg(Color::White))
}

fn value(s: &str) -> Span<'static> {
    Span::styled(s.to_owned(), Style::new().fg(Color::White).bold())
}

/// btop-style polling-rate indicator placed in the header block's top-right
/// title slot.  The `-` and `+` glyphs hint at their `+`/`-` key bindings.
fn polling_rate_title(interval: std::time::Duration) -> Line<'static> {
    let ms = interval.as_millis();
    Line::from(vec![
        Span::styled(" - ", Style::new().fg(Color::Cyan)),
        Span::styled(format!("{ms}ms"), Style::new().fg(Color::White).bold()),
        Span::styled(" + ", Style::new().fg(Color::Cyan)),
    ])
    .right_aligned()
}

#[cfg(test)]
mod tests {
    use crate::test_support;
    use insta::assert_snapshot;

    #[test]
    fn renders_with_root() {
        let app = test_support::make_app();
        let frame = test_support::render_widget(100, 9, |frame, area| {
            super::render_header(frame, &app, area);
        });
        assert_snapshot!(frame);
    }
}
