//! Tree-table widget — htop-style columnar view of the process tree.
//!
//! Renders the flattened `tree::Row` list from `App` as a ratatui `Table`.
//! The function calls `app.set_visible_rows` so that `App::sync_scroll` can
//! keep the selection visible on the next tick.
//!
//! At narrow terminal widths, lower-priority columns are progressively hidden
//! so the Command column always gets a reasonable amount of space.

use crate::{app::App, format};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    symbols::merge::MergeStrategy,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Row, Table},
};

/// Which columns are visible at the current terminal width.
struct ColumnSet {
    user: bool,
    state_full: bool, // full word vs single char
    res: bool,
    elapsed: bool,
    /// Show accumulated CPU time alongside wall-clock elapsed.
    cpu_time: bool,
}

impl ColumnSet {
    /// Pick the richest column set that leaves at least `min_cmd` columns for Command.
    fn for_width(w: u16) -> Self {
        // Thresholds based on total inner width. Each tier ensures at least
        // ~25 columns remain for the Command column.
        if w >= 120 {
            // Full layout + CPU time column
            Self {
                user: true,
                state_full: true,
                res: true,
                elapsed: true,
                cpu_time: true,
            }
        } else if w >= 100 {
            // Full layout
            Self {
                user: true,
                state_full: true,
                res: true,
                elapsed: true,
                cpu_time: false,
            }
        } else if w >= 80 {
            // Drop ELAPSED
            Self {
                user: true,
                state_full: true,
                res: true,
                elapsed: false,
                cpu_time: false,
            }
        } else if w >= 70 {
            // Drop RES; abbreviate STATE
            Self {
                user: true,
                state_full: false,
                res: false,
                elapsed: false,
                cpu_time: false,
            }
        } else if w >= 50 {
            // Drop USER
            Self {
                user: false,
                state_full: false,
                res: false,
                elapsed: false,
                cpu_time: false,
            }
        } else {
            // Minimal: PID + STATE(char) + CPU% + Command
            Self {
                user: false,
                state_full: false,
                res: false,
                elapsed: false,
                cpu_time: false,
            }
        }
    }
}

pub fn render_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    // Subtract 3 for top border + column header row + bottom border.
    app.set_visible_rows((area.height as usize).saturating_sub(3));

    // Inner width minus left/right borders.
    let inner_width = area.width.saturating_sub(2);
    let cols = ColumnSet::for_width(inner_width);

    let mut header_cells: Vec<Cell> = vec![Cell::new(Line::from("PID").centered())];
    let mut widths: Vec<Constraint> = vec![Constraint::Length(8)];

    if cols.user {
        header_cells.push(Cell::new("USER"));
        widths.push(Constraint::Length(9));
    }
    if cols.state_full {
        header_cells.push(Cell::new("STATE"));
        widths.push(Constraint::Length(9));
    } else {
        header_cells.push(Cell::new("S"));
        widths.push(Constraint::Length(3));
    }
    header_cells.push(Cell::new("CPU%"));
    widths.push(Constraint::Length(5));
    if cols.res {
        header_cells.push(Cell::new("RES"));
        widths.push(Constraint::Length(8));
    }
    if cols.elapsed {
        header_cells.push(Cell::new("ELAPSED"));
        widths.push(Constraint::Length(9));
    }
    if cols.cpu_time {
        header_cells.push(Cell::new("CPUT"));
        widths.push(Constraint::Length(9));
    }
    header_cells.push(Cell::new("Command"));
    widths.push(Constraint::Fill(1));

    let header = Row::new(header_cells).style(Style::new().fg(Color::White));

    // Compose the full Command-column string for each visible row up front.
    // Done before clamping the horizontal scroll because the longest of
    // these strings determines the maximum valid scroll offset.
    let composed_cmds: Vec<String> = app
        .flat_rows()
        .iter()
        .map(|fr| {
            let collapse_marker = if fr.has_children() {
                if fr.is_collapsed() { "▸ " } else { "▾ " }
            } else {
                ""
            };
            format!("{}{}{}", fr.connector(), collapse_marker, fr.cmdline())
        })
        .collect();

    // Derive the Command column's pixel/cell width from the constraint list:
    // sum the fixed-length columns and the inter-column spacing, then
    // subtract from the table's inner width.  Using the same Vec we feed to
    // ratatui keeps this number consistent with the layout it produces.
    let fixed_widths_sum: u16 = widths
        .iter()
        .filter_map(|c| match c {
            Constraint::Length(n) => Some(*n),
            _ => None,
        })
        .sum();
    let spacing_total: u16 = (widths.len() as u16).saturating_sub(1);
    let cmd_width = inner_width
        .saturating_sub(fixed_widths_sum)
        .saturating_sub(spacing_total) as usize;

    // Clamp the horizontal scroll so we never scroll past the longest visible
    // line.  Input handlers grow `h_scroll` freely; this is the single point
    // where it is bounded against actual content.
    let max_cmd_len = composed_cmds
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0);
    let max_h_scroll = max_cmd_len.saturating_sub(cmd_width);
    app.clamp_h_scroll(max_h_scroll);
    let h_scroll = app.h_scroll();

    let selected = app.selected();
    let rows: Vec<Row> = app
        .flat_rows()
        .iter()
        .enumerate()
        .map(|(i, fr)| {
            let is_selected = i == selected;
            let cpu_color = fr.cpu_pct().color_scaled(app.cpu_count() as f64 * 100.0);

            // Char-skip (not byte-slice) so multibyte characters in command
            // lines don't panic the slice operation.
            let cmd: String = composed_cmds[i].chars().skip(h_scroll).collect();

            let base_style = if is_selected {
                Style::new()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if fr.is_thread() {
                Style::new().add_modifier(Modifier::DIM)
            } else {
                Style::new()
            };

            let is_thread = fr.is_thread();
            let mut cells: Vec<Cell> = vec![Cell::new(format!("{:>7}", fr.pid()))];

            if cols.user {
                cells.push(Cell::new(if is_thread {
                    String::new()
                } else {
                    format!("{:<8}", fr.user())
                }));
            }
            if cols.state_full {
                cells.push(Cell::new(format!("{:<9}", format::state_word(fr.state()))));
            } else {
                cells.push(Cell::new(format!("{} ", fr.state())));
            }
            cells.push(
                Cell::new(format!("{:>4.1}", fr.cpu_pct().value()))
                    .style(Style::new().fg(cpu_color)),
            );
            if cols.res {
                cells.push(Cell::new(if is_thread {
                    String::new()
                } else {
                    format!("{:>7}", format::format_bytes(fr.mem_rss_bytes()))
                }));
            }
            if cols.elapsed {
                cells.push(Cell::new(if is_thread {
                    String::new()
                } else {
                    format!("{:>8}", format::format_duration(fr.elapsed()))
                }));
            }
            if cols.cpu_time {
                cells.push(Cell::new(format!(
                    "{:>8}",
                    format::format_duration(fr.cpu_time())
                )));
            }
            cells.push(Cell::new(cmd));

            Row::new(cells).style(base_style)
        })
        .collect();

    let mut footer_hints = [
        Line::from(Vec::from([
            Span::styled(" Esc", Style::new().fg(Color::Cyan)),
            Span::styled("/", Style::new().fg(Color::White)),
            Span::styled("q", Style::new().fg(Color::Cyan)),
            Span::styled("uit ", Style::new().fg(Color::White)),
        ])),
        Line::from(Vec::from([
            Span::styled(" ←↑↓→", Style::new().fg(Color::Cyan)),
            Span::styled(" nav ", Style::new().fg(Color::White)),
        ])),
        Line::from(Vec::from([
            Span::styled(" ␣", Style::new().fg(Color::Cyan)),
            Span::styled(" collapse ", Style::new().fg(Color::White)),
        ])),
        Line::from(Vec::from([
            Span::styled(" t", Style::new().fg(Color::Cyan)),
            Span::styled("hreads ", Style::new().fg(Color::White)),
        ])),
    ]
    .into_iter();

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::DarkGray))
                .merge_borders(MergeStrategy::Fuzzy)
                .title_bottom(footer_hints.next().unwrap())
                .title_bottom(footer_hints.next().unwrap())
                .title_bottom(footer_hints.next().unwrap())
                .title_bottom(footer_hints.next().unwrap()),
        )
        .row_highlight_style(Style::new().bg(Color::DarkGray).bold())
        .column_spacing(1);

    frame.render_stateful_widget(table, area, app.table_state_mut());
}
