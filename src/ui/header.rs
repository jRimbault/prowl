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

    render_cpu_panel(frame, cpu_panel, app.cpu_history(), root.cpu_pct());
    render_info_panel(frame, info_panel, root, app.mem_history());
}

/// Render the left CPU panel: narrow label column + tall braille graph.
///
/// Row 0 shows the current percentage; rows 1-3 show the "C", "P", "U" label
/// letters; remaining rows are blank.  The graph fills all rows from the bottom
/// upward so the trace reads as a filled area chart.  The y-axis spans 0–100 %
/// (single-core capacity), matching btop's per-process CPU graph: typical
/// sub-100 % activity stays readable, and rare multi-threaded bursts above
/// 100 % clip at the top of the panel rather than squashing the common case
/// against the baseline.
fn render_cpu_panel(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    cpu_pct: crate::format::Percent,
) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    const LABEL_W: u16 = 7;
    let [label_col, graph_col] =
        Layout::horizontal([Constraint::Length(LABEL_W), Constraint::Fill(1)]).areas(area);

    let rows = graph_col.height as usize;
    let graph_rows =
        format::braille_graph_multi(history.iter(), graph_col.width as usize, rows, 100.0);
    let label_color = cpu_pct.color();
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
            Style::new().fg(label_color).bold()
        } else {
            Style::new().fg(Color::White)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(label_text, label_style)),
            Rect::new(label_col.left(), y, label_col.width, 1),
        );

        if let Some(row_str) = graph_rows.get(r) {
            let row_color = cpu_row_color(r, rows);
            frame.render_widget(
                Paragraph::new(Span::styled(row_str.as_str(), Style::new().fg(row_color))),
                Rect::new(graph_col.left(), y, graph_col.width, 1),
            );
        }
    }
}

/// Color for graph row `row` (0 = topmost) based on the vertical position it
/// occupies when the y-axis spans 0–100 %.  Mirrors btop's gradient: bottom
/// rows green, middle yellow, top red — so a single column's gradient reads
/// as the intensity of that sample.  Uses 24-bit RGB and interpolates between
/// three anchor colours so neighbouring rows differ by a few RGB units
/// instead of jumping between the three terminal-palette slots; the result
/// reads as a continuous gradient rather than discrete bands.
fn cpu_row_color(row: usize, total_rows: usize) -> Color {
    // Anchor stops along the green → yellow → red path used by btop.
    const LOW: (u8, u8, u8) = (0x2b, 0xd6, 0x24);
    const MID: (u8, u8, u8) = (0xf5, 0xd6, 0x30);
    const HIGH: (u8, u8, u8) = (0xde, 0x2c, 0x2c);

    if total_rows == 0 {
        return Color::Rgb(LOW.0, LOW.1, LOW.2);
    }
    // t = 0 at the bottom row, t = 1 at the top row.  A single-row graph
    // collapses to t = 0 (LOW) rather than dividing by zero.
    let denom = (total_rows - 1).max(1) as f64;
    let t = (total_rows - 1 - row) as f64 / denom;
    let (r, g, b) = if t < 0.5 {
        lerp_rgb(LOW, MID, t * 2.0)
    } else {
        lerp_rgb(MID, HIGH, (t - 0.5) * 2.0)
    };
    Color::Rgb(r, g, b)
}

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
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

    /// Idle history (small CPU%).  With the y-axis fixed at 100 %, the
    /// trace must sit close to the baseline — never spike to fill the
    /// panel.  Regression guard for the auto-rescaling behaviour we
    /// removed.
    #[test]
    fn renders_with_idle_cpu_history() {
        let app = test_support::make_app_with_history(&[
            (2.0, 0.4),
            (3.5, 0.5),
            (1.2, 0.5),
            (4.1, 0.6),
            (2.7, 0.5),
            (1.8, 0.5),
            (3.0, 0.6),
            (2.4, 0.6),
        ]);
        let frame = test_support::render_widget(100, 9, |frame, area| {
            super::render_header(frame, &app, area);
        });
        assert_snapshot!(frame);
    }

    /// Mixed ramp covering most of the 0–100 % range so successive rows
    /// of the multi-row graph each carry visible dots.
    #[test]
    fn renders_with_mixed_cpu_history() {
        let app = test_support::make_app_with_history(&[
            (5.0, 1.0),
            (15.0, 1.5),
            (30.0, 2.5),
            (55.0, 4.0),
            (80.0, 6.0),
            (70.0, 5.5),
            (50.0, 4.5),
            (35.0, 4.0),
            (22.0, 3.5),
            (15.0, 3.0),
        ]);
        let frame = test_support::render_widget(100, 9, |frame, area| {
            super::render_header(frame, &app, area);
        });
        assert_snapshot!(frame);
    }

    /// Saturated history — every sample sits near the 100 % single-core
    /// ceiling, so the graph fills the panel from baseline to the top row.
    #[test]
    fn renders_with_saturated_cpu_history() {
        let app = test_support::make_app_with_history(&[
            (95.0, 8.0),
            (99.0, 9.0),
            (96.0, 8.5),
            (99.0, 9.0),
            (97.0, 8.8),
            (100.0, 9.2),
            (98.0, 9.0),
            (99.0, 9.1),
        ]);
        let frame = test_support::render_widget(100, 9, |frame, area| {
            super::render_header(frame, &app, area);
        });
        assert_snapshot!(frame);
    }

    /// Bottom of the graph reads green, top reads red, every row is a
    /// distinct shade — i.e. a continuous green→yellow→red gradient
    /// rather than three palette bands.
    #[test]
    fn cpu_row_color_gradient_is_continuous() {
        use ratatui::style::Color;
        let rows = 6;
        let colors: Vec<(u8, u8, u8)> = (0..rows)
            .map(|r| match super::cpu_row_color(r, rows) {
                Color::Rgb(r, g, b) => (r, g, b),
                other => panic!("expected Rgb, got {other:?}"),
            })
            .collect();
        // No two rows share an exact shade (the previous palette-banded
        // implementation collapsed pairs of rows to identical colours).
        let mut unique = colors.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            colors.len(),
            "expected a distinct shade per row, got {colors:?}"
        );
        // Bottom is greener than red; top is redder than green.
        let (br, bg, _) = *colors.last().expect("non-empty");
        let (tr, tg, _) = colors[0];
        assert!(
            bg > br,
            "bottom row should be greener than red: ({br}, {bg}, _)"
        );
        assert!(
            tr > tg,
            "top row should be redder than green: ({tr}, {tg}, _)"
        );
    }
}
