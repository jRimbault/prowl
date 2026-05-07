//! Snapshot-test scaffolding.
//!
//! Provides two pieces of plumbing for `insta`-driven UI tests:
//!
//! 1. `render` / `render_widget` — drive a `ratatui::Terminal<TestBackend>`
//!    once and return the resulting buffer as a deterministic `String` so
//!    `insta::assert_snapshot!` can diff it against the golden file.
//! 2. `make_app` / `make_detail` — build deterministic `App` and
//!    `ProcessDetail` instances that look real enough for layout tests
//!    without depending on the live `/proc` filesystem.
//!
//! The module is gated behind `#[cfg(test)]` so it never ships in release
//! builds.  All inner state is constructed via the public test helpers in
//! `process::tests`, keeping the production `Node` fields private.

use std::time::Duration;

use ratatui::{Frame, Terminal, backend::TestBackend, layout::Rect};

use crate::{
    app::App,
    process::{
        Pid, ProcessDetail, SchedPolicy, Tree,
        tests::{
            make_test_node, make_test_thread, push_child, set_cmdline, set_cpu_pct, set_cpu_time,
            set_elapsed, set_io, set_mem_pct, set_mem_rss_bytes, set_parent_name, set_state,
            set_user,
        },
    },
};

/// Render a full frame via `ui::render` and return the textual buffer.
///
/// Uses `TestBackend` (ratatui's pure in-memory backend), so no terminal
/// setup is required and styling/colour is dropped from the output —
/// only character cells make it into the snapshot, keeping snapshots
/// reproducible across runs and CI environments.
pub fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("TestBackend construction is infallible");
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .expect("TestBackend draw is infallible");
    format!("{}", terminal.backend())
}

/// Render a single widget closure to a deterministic buffer string.
///
/// Use when you want to exercise an inner renderer (e.g. just the tree
/// table or just the detail panel) without the rest of the chrome.
pub fn render_widget<F>(width: u16, height: u16, mut draw: F) -> String
where
    F: FnMut(&mut Frame, Rect),
{
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .expect("TestBackend construction is infallible");
    terminal
        .draw(|frame| {
            let area = frame.area();
            draw(frame, area);
        })
        .expect("TestBackend draw is infallible");
    format!("{}", terminal.backend())
}

/// Build a deterministic `App` with a small process tree (root + a couple
/// of children + a thread).  Numbers are picked so each table column has
/// a representative non-empty value (PID width, MiB-scale RES, mm:ss
/// ELAPSED, multi-second CPU time, non-zero IO totals).
pub fn make_app() -> App {
    let mut app = App::new(true, 8, Duration::from_millis(1000));
    app.apply_snapshot(make_tree());
    app
}

/// Mirror of `make_app` but with the detail panel pre-populated with the
/// supplied `ProcessDetail`.  Selects the row that matches the detail PID
/// before opening the panel so `App::toggle_detail` activates the right
/// row.
pub fn make_app_with_detail(detail: ProcessDetail) -> App {
    let mut app = make_app();
    let target = detail.pid;
    if let Some(idx) = app.flat_rows().iter().position(|r| r.pid() == target) {
        // The selection driver only exposes step-up / step-down; walk the
        // selection rather than reaching into private fields.
        for _ in 0..idx {
            app.move_down();
        }
    }
    let _ = app.toggle_detail();
    app.set_detail_info(detail);
    app
}

/// Build the canonical sample tree used across snapshot tests.
///
/// Layout:
/// ```text
/// myapp [1234]              R   12.3% 50 MiB
/// ├─ worker [1235]          S    1.2%  8 MiB
/// │  └─ helper-thread [200] S    0.0%
/// └─ logger [1236]          S    0.0%  4 MiB
/// ```
pub fn make_tree() -> Tree {
    let mut root = make_test_node(1234, "myapp");
    set_cmdline(&mut root, "/usr/bin/myapp --serve --port 8080");
    set_user(&mut root, "alice");
    set_state(&mut root, 'R');
    set_cpu_pct(&mut root, 12.3);
    set_mem_rss_bytes(&mut root, 50 * 1024 * 1024);
    set_mem_pct(&mut root, 1.5);
    set_elapsed(&mut root, Duration::from_secs(3725));
    set_cpu_time(&mut root, Duration::from_secs(60));
    set_io(&mut root, 4 * 1024 * 1024, 2 * 1024 * 1024);
    set_parent_name(&mut root, "bash");

    let mut worker = make_test_node(1235, "worker");
    set_cmdline(&mut worker, "/usr/bin/myapp worker --queue tasks");
    set_user(&mut worker, "alice");
    set_cpu_pct(&mut worker, 1.2);
    set_mem_rss_bytes(&mut worker, 8 * 1024 * 1024);
    set_elapsed(&mut worker, Duration::from_secs(3700));
    set_cpu_time(&mut worker, Duration::from_secs(12));
    set_parent_name(&mut worker, "myapp");

    let mut thread = make_test_thread(200, "helper");
    set_cmdline(&mut thread, "helper");
    set_user(&mut thread, "alice");
    set_parent_name(&mut thread, "worker");
    push_child(&mut worker, thread);

    let mut logger = make_test_node(1236, "logger");
    set_cmdline(&mut logger, "/usr/bin/myapp logger");
    set_user(&mut logger, "alice");
    set_mem_rss_bytes(&mut logger, 4 * 1024 * 1024);
    set_elapsed(&mut logger, Duration::from_secs(3700));
    set_cpu_time(&mut logger, Duration::from_secs(2));
    set_parent_name(&mut logger, "myapp");

    push_child(&mut root, worker);
    push_child(&mut root, logger);
    Tree::from(root)
}

/// Build a deterministic `ProcessDetail` for a process (not a thread).
pub fn make_process_detail() -> ProcessDetail {
    ProcessDetail {
        pid: Pid::new(1234),
        is_thread: false,
        tgid: Pid::new(1234),
        state: 'R',
        exe: Some("/usr/bin/myapp".to_owned()),
        cwd: Some("/home/alice/projects/myapp".to_owned()),
        fd_count: 42,
        fd_soft_limit: Some(1024),
        environ: vec![
            ("HOME".to_owned(), "/home/alice".to_owned()),
            ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
            ("SHELL".to_owned(), "/bin/bash".to_owned()),
            ("USER".to_owned(), "alice".to_owned()),
        ],
        nice: 0,
        priority: 20,
        policy: Some(SchedPolicy::Normal),
        rt_priority: Some(0),
        last_cpu: Some(3),
        vm_hwm_kb: Some(72_000),
        vm_rss_kb: Some(70_000),
        vm_size_kb: Some(820_000),
        vm_data_kb: Some(40_000),
        vm_stack_kb: Some(132),
        vm_swap_kb: Some(0),
        voluntary_ctxt_switches: Some(1_234),
        nonvoluntary_ctxt_switches: Some(56),
        minor_faults: 12_345,
        major_faults: 7,
        user_cpu_time: Duration::from_secs(8),
        system_cpu_time: Duration::from_secs(3),
        wchan: None,
        oom_score: Some(180),
        oom_score_adj: Some(0),
        thread_count: Some(8),
        cgroup: Some("/user.slice/user-1000.slice/session.scope".to_owned()),
    }
}

/// Build a deterministic `ProcessDetail` for a thread.  `is_thread = true`
/// (tgid != pid) drives the renderer onto the per-thread layout.
pub fn make_thread_detail() -> ProcessDetail {
    ProcessDetail {
        pid: Pid::new(200),
        is_thread: true,
        tgid: Pid::new(1235),
        state: 'S',
        // Threads inherit these from the parent process; collect_detail
        // still populates them, so we keep them set in the fixture.
        exe: Some("/usr/bin/myapp".to_owned()),
        cwd: Some("/home/alice/projects/myapp".to_owned()),
        fd_count: 0,
        fd_soft_limit: Some(1024),
        environ: Vec::new(),
        nice: 0,
        priority: 20,
        policy: Some(SchedPolicy::Normal),
        rt_priority: Some(0),
        last_cpu: Some(1),
        vm_hwm_kb: None,
        vm_rss_kb: None,
        vm_size_kb: None,
        vm_data_kb: None,
        vm_stack_kb: None,
        vm_swap_kb: None,
        voluntary_ctxt_switches: Some(89),
        nonvoluntary_ctxt_switches: Some(4),
        minor_faults: 64,
        major_faults: 0,
        user_cpu_time: Duration::from_secs(1),
        system_cpu_time: Duration::from_millis(500),
        wchan: Some("futex_wait_queue".to_owned()),
        oom_score: None,
        oom_score_adj: None,
        thread_count: None,
        cgroup: None,
    }
}
