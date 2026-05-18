//! Application state — UI coordination layer.
//!
//! `App` holds only what the renderer needs: the latest process snapshot,
//! rolling metric history for sparklines, selection/scroll state, and display
//! preferences.  All data collection lives in `collector`; all rendering in `ui`.

use std::{
    collections::{HashSet, VecDeque},
    time::Duration,
};

use crate::{
    format::Percent,
    process::{Node, Pid, ProcessDetail, Tree},
    tree::{Row, flatten},
};

const HISTORY_CAPACITY: usize = 200;

/// Polling-rate adjustment bounds and step size for the +/- key bindings.
const INTERVAL_STEP_MS: u64 = 100;
const INTERVAL_MIN_MS: u64 = 100;
const INTERVAL_MAX_MS: u64 = 60_000;

/// Number of characters shifted per Left/Right keypress in the tree view.
/// Tuned so a long command line scrolls into view in a few presses without
/// requiring tedious one-char-at-a-time stepping.
const H_SCROLL_STEP: usize = 4;

/// Bounded ring buffer of `Percent` samples for sparkline graphs.
pub struct History {
    values: VecDeque<Percent>,
    capacity: usize,
}

impl History {
    fn new(capacity: usize) -> Self {
        Self {
            values: VecDeque::new(),
            capacity,
        }
    }

    pub fn push(&mut self, p: Percent) {
        self.values.push_back(p);
        if self.values.len() > self.capacity {
            self.values.pop_front();
        }
    }

    /// Iterate over stored samples oldest-first, yielding `Percent` by value.
    pub fn iter(&self) -> impl Iterator<Item = Percent> + '_ {
        self.values.iter().copied()
    }

    // Provided as part of the bounded-buffer API; not currently called by renderers.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

pub struct App {
    root: Option<Tree>,
    /// Name of the observed root process; displayed in the header title.
    name: String,
    flat_rows: Vec<Row>,
    /// Index into `flat_rows` for keyboard selection.
    selected: usize,
    /// Ratatui stateful widget state (carries scroll offset).
    table_state: ratatui::widgets::TableState,
    show_threads: bool,
    /// Set to `true` when the monitored PID has disappeared.
    exited: bool,
    /// Number of tree rows currently visible on screen; set by the renderer.
    visible_rows: usize,
    /// PIDs whose subtrees are collapsed in the tree view.
    collapsed: HashSet<Pid>,
    /// CPU% per sample — feeds the header sparkline.
    cpu_history: History,
    /// MEM% per sample — feeds the header sparkline.
    mem_history: History,
    /// Total logical CPU count.  Aggregated CPU% can reach `cpu_count * 100`,
    /// so the UI uses this to scale traffic-light color thresholds.
    cpu_count: usize,
    /// Current sampling interval, adjustable via the +/- keys.  Mirrored to
    /// the collector through a watch channel; UI shows the value in the
    /// header's top-right title.
    interval: Duration,
    /// Horizontal scroll offset (in characters) applied to the Command column.
    /// The renderer clamps this each frame to a maximum derived from the
    /// longest visible row, so input handlers can grow the value freely.
    h_scroll: usize,
    /// PID currently shown in the detail panel; `None` when the panel is hidden.
    detail_pid: Option<Pid>,
    /// Last fetched detail data for `detail_pid`.  Cleared when the panel closes.
    detail_info: Option<ProcessDetail>,
    /// Case-insensitive substring filter applied to the tree.  Empty = no filter.
    filter: String,
    /// `true` while the user is typing into the filter buffer.  Keystrokes
    /// that would normally trigger actions are routed to the filter instead.
    filter_input: bool,
}

impl App {
    pub fn new(show_threads: bool, cpu_count: usize, interval: Duration) -> Self {
        Self {
            root: None,
            name: String::new(),
            flat_rows: Vec::new(),
            selected: 0,
            table_state: ratatui::widgets::TableState::default(),
            show_threads,
            exited: false,
            visible_rows: 20,
            collapsed: HashSet::new(),
            cpu_history: History::new(HISTORY_CAPACITY),
            mem_history: History::new(HISTORY_CAPACITY),
            cpu_count: cpu_count.max(1),
            interval: clamp_interval(interval),
            h_scroll: 0,
            detail_pid: None,
            detail_info: None,
            filter: String::new(),
            filter_input: false,
        }
    }

    // --- Read accessors ---

    pub fn root(&self) -> Option<&Node> {
        self.root.as_ref().and_then(|t| t.root())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn flat_rows(&self) -> &[Row] {
        &self.flat_rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    // Provided as part of the display-preferences API; may be used by future renderers.
    #[allow(dead_code)]
    pub fn show_threads(&self) -> bool {
        self.show_threads
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn cpu_history(&self) -> &History {
        &self.cpu_history
    }

    pub fn mem_history(&self) -> &History {
        &self.mem_history
    }

    pub fn cpu_count(&self) -> usize {
        self.cpu_count
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn h_scroll(&self) -> usize {
        self.h_scroll
    }

    pub fn detail_pid(&self) -> Option<Pid> {
        self.detail_pid
    }

    pub fn detail_info(&self) -> Option<&ProcessDetail> {
        self.detail_info.as_ref()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn filter_input(&self) -> bool {
        self.filter_input
    }

    /// Shift the command column rightward (text moves left under the
    /// viewport).  Saturates at the top of the configured range; the
    /// renderer applies a per-frame upper bound based on visible content.
    pub fn scroll_right(&mut self) {
        self.h_scroll = self.h_scroll.saturating_add(H_SCROLL_STEP);
    }

    /// Shift the command column leftward (text moves right under the
    /// viewport).  Saturates at zero.
    pub fn scroll_left(&mut self) {
        self.h_scroll = self.h_scroll.saturating_sub(H_SCROLL_STEP);
    }

    /// Cap `h_scroll` to a renderer-supplied maximum offset.  Prevents the
    /// view from scrolling past the longest visible command line.
    pub fn clamp_h_scroll(&mut self, max: usize) {
        if self.h_scroll > max {
            self.h_scroll = max;
        }
    }

    /// Increase the sampling interval by one step (slower polling).
    /// Returns `true` if the value changed.
    pub fn step_interval_up(&mut self) -> bool {
        let next_ms = (self.interval.as_millis() as u64).saturating_add(INTERVAL_STEP_MS);
        let new = Duration::from_millis(next_ms.min(INTERVAL_MAX_MS));
        if new != self.interval {
            self.interval = new;
            true
        } else {
            false
        }
    }

    /// Decrease the sampling interval by one step (faster polling).
    /// Returns `true` if the value changed.
    pub fn step_interval_down(&mut self) -> bool {
        let current_ms = self.interval.as_millis() as u64;
        let next_ms = current_ms.saturating_sub(INTERVAL_STEP_MS);
        let new = Duration::from_millis(next_ms.max(INTERVAL_MIN_MS));
        if new != self.interval {
            self.interval = new;
            true
        } else {
            false
        }
    }

    // --- Mutable accessors for ratatui stateful widget rendering ---

    pub fn table_state_mut(&mut self) -> &mut ratatui::widgets::TableState {
        &mut self.table_state
    }

    /// Update how many tree rows are visible on screen.
    ///
    /// Called by the tree renderer each frame before `sync_scroll` runs.
    pub fn set_visible_rows(&mut self, n: usize) {
        self.visible_rows = n;
    }

    // --- State mutation methods ---

    /// Replace the current snapshot with a freshly collected one.
    pub fn apply_snapshot(&mut self, tree: Tree) {
        let Some(root) = tree.root() else { return };
        self.name = root.name().to_owned();
        self.cpu_history.push(root.cpu_pct());
        self.mem_history.push(root.mem_pct());
        self.root = Some(tree);
        self.refresh_rows();
    }

    /// Re-flatten `self.root` using the current display preferences and
    /// filter, then re-sync the scroll position.  All mutations that change
    /// what the table shows (snapshot, thread/collapse/filter toggles) funnel
    /// through here so the row list, selection clamp, and scroll offset stay
    /// in lockstep.
    fn refresh_rows(&mut self) {
        if let Some(tree) = &self.root {
            self.flat_rows = flatten(tree, self.show_threads, &self.collapsed, &self.filter);
        }
        if !self.flat_rows.is_empty() && self.selected >= self.flat_rows.len() {
            self.selected = self.flat_rows.len() - 1;
        }
        self.sync_scroll();
    }

    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.sync_scroll();
    }

    pub fn move_down(&mut self) {
        if !self.flat_rows.is_empty() {
            self.selected = (self.selected + 1).min(self.flat_rows.len() - 1);
        }
        self.sync_scroll();
    }

    /// Jump the selection to the first visible row.
    pub fn move_to_top(&mut self) {
        self.selected = 0;
        self.sync_scroll();
    }

    /// Jump the selection to the last visible row.
    pub fn move_to_bottom(&mut self) {
        if !self.flat_rows.is_empty() {
            self.selected = self.flat_rows.len() - 1;
        }
        self.sync_scroll();
    }

    /// Reset the horizontal scroll to the leftmost position.
    pub fn scroll_to_start(&mut self) {
        self.h_scroll = 0;
    }

    /// Park the horizontal scroll at the rightmost position by overshooting;
    /// the next frame's `clamp_h_scroll` reduces this to the actual maximum
    /// derived from the longest visible command line.
    pub fn scroll_to_end(&mut self) {
        self.h_scroll = usize::MAX;
    }

    /// Toggle thread visibility using the already-cached snapshot — no refresh needed.
    pub fn toggle_threads(&mut self) {
        self.show_threads = !self.show_threads;
        self.refresh_rows();
    }

    /// Toggle collapse state of the currently selected node's subtree.
    pub fn toggle_collapse(&mut self) {
        if let Some(row) = self.flat_rows.get(self.selected) {
            let pid = row.pid();
            if !self.collapsed.remove(&pid) {
                self.collapsed.insert(pid);
            }
            self.refresh_rows();
        }
    }

    /// Toggle the detail panel for the currently selected row.
    ///
    /// - If the panel is already showing this PID, closes it and returns `None`.
    /// - Otherwise, opens the panel for the selected PID and returns that PID
    ///   so the caller can fetch `ProcessDetail` and call `set_detail_info`.
    pub fn toggle_detail(&mut self) -> Option<Pid> {
        if let Some(row) = self.flat_rows.get(self.selected) {
            let pid = row.pid();
            if self.detail_pid == Some(pid) {
                self.detail_pid = None;
                self.detail_info = None;
                None
            } else {
                self.detail_pid = Some(pid);
                self.detail_info = None;
                Some(pid)
            }
        } else {
            None
        }
    }

    /// Store freshly fetched detail info for the currently open panel.
    pub fn set_detail_info(&mut self, info: ProcessDetail) {
        self.detail_info = Some(info);
    }

    /// Enter filter-input mode.  Subsequent character keystrokes append to
    /// the filter buffer.  Does not clear the existing filter, so pressing
    /// `f` again lets the user edit the current text.
    pub fn enter_filter_input(&mut self) {
        self.filter_input = true;
    }

    /// Leave filter-input mode without changing the filter text.  The filter
    /// itself stays in effect.
    pub fn exit_filter_input(&mut self) {
        self.filter_input = false;
    }

    /// Append a character to the filter buffer and re-flatten the tree.
    pub fn push_filter_char(&mut self, c: char) {
        self.filter.push(c);
        self.refresh_rows();
    }

    /// Remove the last character from the filter buffer (Backspace) and
    /// re-flatten.  No-op when the buffer is empty.
    pub fn pop_filter_char(&mut self) {
        if self.filter.pop().is_some() {
            self.refresh_rows();
        }
    }

    /// Clear the filter entirely and leave input mode.  Returns `true` if
    /// anything changed (filter was non-empty or input mode was active).
    pub fn clear_filter(&mut self) -> bool {
        let changed = !self.filter.is_empty() || self.filter_input;
        self.filter.clear();
        self.filter_input = false;
        if changed {
            self.refresh_rows();
        }
        changed
    }

    /// Close the detail panel if it is open.  Returns `true` if it was open.
    pub fn close_detail(&mut self) -> bool {
        if self.detail_pid.is_some() {
            self.detail_pid = None;
            self.detail_info = None;
            true
        } else {
            false
        }
    }

    fn sync_scroll(&mut self) {
        if self.visible_rows == 0 {
            return;
        }
        let offset = self.table_state.offset();
        if self.selected < offset {
            *self.table_state.offset_mut() = self.selected;
        } else if self.selected >= offset + self.visible_rows {
            *self.table_state.offset_mut() = self.selected + 1 - self.visible_rows;
        }
        self.table_state.select(Some(self.selected));
    }
}

/// Clamp an arbitrary CLI-supplied interval into the supported range so the
/// initial value matches what +/- can later produce.
fn clamp_interval(d: Duration) -> Duration {
    let ms = (d.as_millis() as u64).clamp(INTERVAL_MIN_MS, INTERVAL_MAX_MS);
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> App {
        App::new(false, 8, Duration::from_millis(1000))
    }

    #[test]
    fn h_scroll_starts_at_zero() {
        assert_eq!(fixture().h_scroll(), 0);
    }

    #[test]
    fn scroll_left_at_zero_does_not_underflow() {
        let mut app = fixture();
        app.scroll_left();
        assert_eq!(app.h_scroll(), 0);
    }

    #[test]
    fn scroll_right_advances_by_step() {
        let mut app = fixture();
        app.scroll_right();
        assert_eq!(app.h_scroll(), H_SCROLL_STEP);
    }

    #[test]
    fn scroll_left_after_right_returns_to_zero() {
        let mut app = fixture();
        app.scroll_right();
        app.scroll_left();
        assert_eq!(app.h_scroll(), 0);
    }

    #[test]
    fn clamp_h_scroll_caps_above_max() {
        let mut app = fixture();
        app.scroll_right();
        app.scroll_right();
        app.clamp_h_scroll(2);
        assert_eq!(app.h_scroll(), 2);
    }

    #[test]
    fn clamp_h_scroll_leaves_value_below_max_untouched() {
        let mut app = fixture();
        app.scroll_right();
        app.clamp_h_scroll(usize::MAX);
        assert_eq!(app.h_scroll(), H_SCROLL_STEP);
    }

    #[test]
    fn scroll_to_start_resets_offset() {
        let mut app = fixture();
        app.scroll_right();
        app.scroll_right();
        app.scroll_to_start();
        assert_eq!(app.h_scroll(), 0);
    }

    #[test]
    fn scroll_to_end_overshoots_then_clamps() {
        let mut app = fixture();
        app.scroll_to_end();
        // Without a renderer in the loop, simulate the clamp the renderer
        // performs each frame to verify the overshoot collapses cleanly.
        app.clamp_h_scroll(42);
        assert_eq!(app.h_scroll(), 42);
    }
}
