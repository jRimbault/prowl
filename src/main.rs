//! `prowl` — TUI process-tree monitor.
//!
//! Module layout:
//!
//! * `format`    — stateless formatting utilities
//! * `process`   — procfs I/O; produces plain data structs
//! * `tree`      — flatten logic; `Row` type for the table widget
//! * `collector` — async task that owns sampling state and publishes via `watch`
//! * `app`       — UI state: selection, scroll, display preferences
//! * `ui`        — rendering: reads `App`, writes to the terminal frame
//! * `main`      — event loop, raw-mode setup/teardown

use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use clap::Parser;
use crossterm::{
    cursor,
    event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt as _;
use prowl::{app, collector, picker, process, ui};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::{sync::watch, task};

fn styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Style};
    clap::builder::Styles::styled()
        .header(
            Style::new()
                .fg_color(Some(AnsiColor::Yellow.into()))
                .bold()
                .underline(),
        )
        .usage(Style::new().fg_color(Some(AnsiColor::Yellow.into())).bold())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Cyan.on_default())
}

/// Monitor a PID and its subprocess tree in a TUI.
#[derive(Parser)]
#[command(version, styles = styles())]
struct Args {
    /// PID to monitor; omit to launch the interactive process picker
    pid: Option<i32>,
    /// Refresh interval in milliseconds
    #[arg(short, long, default_value = "1000", value_parser = parse_millis)]
    interval: Duration,
    /// Show threads on startup
    #[arg(short, long)]
    threads: bool,
}

fn parse_millis(s: &str) -> Result<Duration, String> {
    let ms: u64 = s
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    if ms == 0 {
        return Err("value must be at least 1ms".to_string());
    }
    Ok(Duration::from_millis(ms))
}

/// RAII guard that restores the terminal unconditionally on drop.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Resolve the PID: use the CLI argument if provided, otherwise launch the
    // interactive picker.  Return early (clean exit) if the user cancels.
    let pid = match args.pid {
        Some(raw) => process::Pid::new(raw),
        None => match picker::pick()? {
            Some(pid) => pid,
            None => return Ok(()),
        },
    };

    let uid_map = Arc::new(process::load_uid_map());
    let cpu_count = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);
    let mut app = app::App::new(args.threads, cpu_count, args.interval);

    let (tx, mut rx) = watch::channel(None::<process::Tree>);
    // Polling-rate channel: the UI publishes new intervals (via +/-) and the
    // collector rebuilds its ticker on receipt.
    let (interval_tx, interval_rx) = watch::channel(app.interval());
    tokio::spawn(collector::run(
        pid,
        app.interval(),
        interval_rx,
        uid_map,
        tx,
    ));

    // Block until the first snapshot arrives so the first TUI frame is populated.
    rx.changed()
        .await
        .context("process not found or collector failed before first sample")?;
    match rx.borrow_and_update().clone() {
        Some(tree) => app.apply_snapshot(tree),
        None => anyhow::bail!("process {} exited before it could be sampled", pid),
    }

    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, cursor::Hide)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut events = EventStream::new();

    terminal.draw(|frame| ui::render(frame, &mut app))?;

    loop {
        tokio::select! {
            result = rx.changed() => {
                // `Err` means the sender was dropped (collector finished or process gone).
                match result {
                    Ok(()) => match rx.borrow_and_update().clone() {
                        Some(tree) => app.apply_snapshot(tree),
                        None => app.mark_exited(),
                    },
                    Err(_) => app.mark_exited(),
                }
                // Refresh the detail panel while it is open.
                if let Some(pid) = app.detail_pid()
                    && let Ok(Ok(info)) = task::spawn_blocking(move || process::collect_detail(pid)).await
                {
                    app.set_detail_info(info);
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        match key.code {
                            // --- Filter input mode ---
                            // While the filter prompt is active, character
                            // keystrokes edit the filter rather than firing
                            // their normal actions; Esc/Enter exit input mode
                            // without clearing the filter; Backspace edits.
                            KeyCode::Char(c) if app.filter_input() => app.push_filter_char(c),
                            KeyCode::Backspace if app.filter_input() => app.pop_filter_char(),
                            KeyCode::Enter if app.filter_input() => app.exit_filter_input(),
                            // Del always clears the filter and leaves input mode.
                            KeyCode::Delete => { let _ = app.clear_filter(); }

                            KeyCode::Char('q') => break,
                            // Esc precedence: leave filter input → close detail → quit.
                            KeyCode::Esc => {
                                if app.filter_input() {
                                    app.exit_filter_input();
                                } else if !app.close_detail() {
                                    break;
                                }
                            }
                            // Ctrl+arrow jumps to the corresponding extremity;
                            // plain arrows step by one. The Ctrl variants are
                            // matched first so the unmodified arms only fire
                            // when no modifier is held.
                            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.move_to_top()
                            }
                            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.move_to_bottom()
                            }
                            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.scroll_to_start()
                            }
                            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.scroll_to_end()
                            }
                            KeyCode::Up => app.move_up(),
                            KeyCode::Down => app.move_down(),
                            KeyCode::Left => app.scroll_left(),
                            KeyCode::Right => app.scroll_right(),
                            KeyCode::Char('f') => app.enter_filter_input(),
                            KeyCode::Char('t') => app.toggle_threads(),
                            KeyCode::Char(' ') => app.toggle_collapse(),
                            KeyCode::Enter => {
                                if let Some(pid) = app.toggle_detail() {
                                    // Fetch detail on a blocking thread so the async
                                    // runtime is not stalled by procfs I/O.
                                    if let Ok(Ok(info)) = task::spawn_blocking(move || process::collect_detail(pid)).await {
                                        app.set_detail_info(info);
                                    }
                                }
                            }
                            // `+` and `=` both bound so the user does not have
                            // to hold Shift on US-style keyboards. `-` is the
                            // sole minus key.
                            KeyCode::Char('+') | KeyCode::Char('=')
                                if app.step_interval_up() => {
                                    let _ = interval_tx.send(app.interval());
                                }
                            KeyCode::Char('-')
                                if app.step_interval_down() => {
                                    let _ = interval_tx.send(app.interval());
                                }
                            _ => {}
                        }
                    }
                    None => break, // event stream closed (terminal lost)
                    _ => {}
                }
            }
        }

        terminal.draw(|frame| ui::render(frame, &mut app))?;

        if app.exited() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            break;
        }
    }

    Ok(())
}
