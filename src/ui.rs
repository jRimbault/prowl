//! UI rendering entry point.
//!
//! Splits the terminal frame into a fixed-height header panel, a fill-height
//! tree table, and an optional detail panel at the bottom that appears when
//! the user opens it with Enter.

mod detail;
mod header;
mod tree;

use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Spacing},
};

/// Render a full terminal frame.
///
/// `app` is mutably borrowed because the tree renderer updates
/// `app.visible_rows` so scroll synchronisation in `App` knows how many
/// rows are currently on screen.
pub fn render(frame: &mut Frame, app: &mut App) {
    if app.detail_pid().is_some() {
        // Thread vs process layouts differ in vertical density, so the
        // detail block height is chosen by the panel rather than fixed
        // at the call site.
        let detail_h = detail::detail_height(app);
        let [header_area, tree_area, detail_area] = Layout::vertical([
            Constraint::Length(9),
            Constraint::Fill(1),
            Constraint::Length(detail_h),
        ])
        .spacing(Spacing::Overlap(1))
        .areas(frame.area());

        header::render_header(frame, app, header_area);
        tree::render_tree(frame, app, tree_area);
        detail::render_detail(frame, app, detail_area);
    } else {
        let [header_area, tree_area] =
            Layout::vertical([Constraint::Length(9), Constraint::Fill(1)])
                .spacing(Spacing::Overlap(1))
                .areas(frame.area());

        header::render_header(frame, app, header_area);
        tree::render_tree(frame, app, tree_area);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support;
    use insta::assert_snapshot;

    #[test]
    fn renders_frame_without_detail() {
        let mut app = test_support::make_app();
        let frame = test_support::render(&mut app, 100, 24);
        assert_snapshot!(frame);
    }

    #[test]
    fn renders_frame_with_process_detail() {
        let mut app = test_support::make_app_with_detail(test_support::make_process_detail());
        let frame = test_support::render(&mut app, 100, 30);
        assert_snapshot!(frame);
    }

    #[test]
    fn renders_frame_with_thread_detail() {
        let mut app = test_support::make_app_with_detail(test_support::make_thread_detail());
        let frame = test_support::render(&mut app, 100, 28);
        assert_snapshot!(frame);
    }
}
