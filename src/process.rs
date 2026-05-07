//! Process data collection.
//!
//! This module handles all I/O against the Linux procfs filesystem.
//! It produces plain data types consumed by `app` (coordination) and
//! `ui` (rendering).

use std::{
    collections::HashMap,
    fmt, fs,
    time::{Duration, UNIX_EPOCH},
};

use procfs::process::{Process, all_processes};

use crate::format::Percent;

/// Newtype wrapping a Linux process/thread ID.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pid(i32);

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Pid {
    pub fn new(pid: i32) -> Self {
        Self(pid)
    }

    /// Return the raw `i32` value.
    pub fn get(self) -> i32 {
        self.0
    }
}

/// Cumulative read and write bytes for a process.
#[derive(Copy, Clone, Debug, Default)]
pub struct IoTotals {
    read: u64,
    write: u64,
}

impl IoTotals {
    pub fn new(read: u64, write: u64) -> Self {
        Self { read, write }
    }

    pub fn read(self) -> u64 {
        self.read
    }

    pub fn write(self) -> u64 {
        self.write
    }
}

/// System-level constants needed for CPU/memory percentage calculations.
///
/// Collected once at startup and passed through the sampling call chain
/// so callers don't have to remember unit conversions.
#[derive(Copy, Clone, Debug)]
pub struct SystemConfig {
    ticks_per_second: u64,
    page_size: u64,
    /// Total physical RAM in bytes (already multiplied from kilobytes at construction).
    mem_total_bytes: u64,
}

impl SystemConfig {
    pub fn new(ticks_per_second: u64, page_size: u64, mem_total_bytes: u64) -> Self {
        Self {
            ticks_per_second,
            page_size,
            mem_total_bytes,
        }
    }

    pub fn ticks_per_second(self) -> u64 {
        self.ticks_per_second
    }

    pub fn page_size(self) -> u64 {
        self.page_size
    }

    pub fn mem_total_bytes(self) -> u64 {
        self.mem_total_bytes
    }
}

/// A process tree rooted at a single `Node`.
///
/// Implements `FromIterator<Node>`: the first yielded node becomes the root,
/// all subsequent nodes are appended as its direct children.  This lets
/// callers build a tree by collecting from any iterator (channels, worker
/// tasks, test arrays, etc.).
#[derive(Clone, Default)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// The root node, if the tree is non-empty.
    pub fn root(&self) -> Option<&Node> {
        self.nodes.first()
    }

    /// Consume the tree and return the root node, if non-empty.
    #[cfg(test)]
    pub fn into_root(self) -> Option<Node> {
        self.nodes.into_iter().next()
    }
}

impl From<Node> for Tree {
    fn from(node: Node) -> Self {
        Self {
            nodes: Vec::from([node]),
        }
    }
}

/// Build a tree: first node = root, remaining nodes = its direct children.
impl std::iter::FromIterator<Node> for Tree {
    fn from_iter<I: IntoIterator<Item = Node>>(iter: I) -> Self {
        let mut iter = iter.into_iter();
        let Some(mut root) = iter.next() else {
            return Self::default();
        };
        std::iter::Extend::extend(&mut root.children, iter);
        Self::from(root)
    }
}

impl std::iter::Extend<Node> for Tree {
    fn extend<I: IntoIterator<Item = Node>>(&mut self, iter: I) {
        self.nodes.extend(iter);
    }
}

/// Full process/thread node in the tree.
#[derive(Clone)]
pub struct Node {
    pid: Pid,
    name: String,
    cmdline: String,
    user: String,
    state: char,
    cpu_pct: Percent,
    mem_rss_bytes: u64,
    mem_pct: Percent,
    /// Cumulative read/write bytes from `/proc/<pid>/io` (0 on permission denied).
    io: IoTotals,
    elapsed: Duration,
    /// Total CPU time consumed (utime + stime from `/proc/<pid>/stat`).
    cpu_time: Duration,
    parent_name: String,
    children: Tree,
    is_thread: bool,
}

impl Node {
    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn cmdline(&self) -> &str {
        &self.cmdline
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn state(&self) -> char {
        self.state
    }

    pub fn cpu_pct(&self) -> Percent {
        self.cpu_pct
    }

    pub fn mem_rss_bytes(&self) -> u64 {
        self.mem_rss_bytes
    }

    pub fn mem_pct(&self) -> Percent {
        self.mem_pct
    }

    pub fn io(&self) -> IoTotals {
        self.io
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn cpu_time(&self) -> Duration {
        self.cpu_time
    }

    pub fn parent_name(&self) -> &str {
        &self.parent_name
    }

    pub fn children(&self) -> &[Node] {
        &self.children.nodes
    }

    pub fn is_thread(&self) -> bool {
        self.is_thread
    }

    /// Count all thread nodes contained in this subtree, excluding `self`.
    pub fn thread_count(&self) -> usize {
        self.children
            .nodes
            .iter()
            .map(Node::thread_count_inclusive)
            .sum()
    }

    /// Count all descendant process nodes in this subtree, excluding `self`.
    pub fn subprocess_count(&self) -> usize {
        self.children
            .nodes
            .iter()
            .map(Node::subprocess_count_inclusive)
            .sum()
    }

    /// Sum CPU time across all descendant process nodes, excluding `self`.
    pub fn subprocess_cpu_time(&self) -> Duration {
        self.children
            .nodes
            .iter()
            .fold(Duration::ZERO, |acc, node| {
                acc.saturating_add(node.subprocess_cpu_time_inclusive())
            })
    }

    fn thread_count_inclusive(&self) -> usize {
        usize::from(self.is_thread)
            + self
                .children
                .nodes
                .iter()
                .map(Node::thread_count_inclusive)
                .sum::<usize>()
    }

    fn subprocess_count_inclusive(&self) -> usize {
        usize::from(!self.is_thread)
            + self
                .children
                .nodes
                .iter()
                .map(Node::subprocess_count_inclusive)
                .sum::<usize>()
    }

    fn subprocess_cpu_time_inclusive(&self) -> Duration {
        let own = if self.is_thread {
            Duration::ZERO
        } else {
            self.cpu_time
        };
        self.children.nodes.iter().fold(own, |acc, node| {
            acc.saturating_add(node.subprocess_cpu_time_inclusive())
        })
    }
}

/// Parse `/etc/passwd` into a `uid → username` map.
///
/// Silently skips malformed lines and returns an empty map on IO error,
/// so callers can degrade to numeric UIDs rather than crash.
pub fn load_uid_map() -> HashMap<u32, String> {
    let content = match fs::read_to_string("/etc/passwd") {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, ':');
            let name = fields.next()?.to_owned();
            fields.next()?; // password placeholder
            let uid: u32 = fields.next()?.parse().ok()?;
            Some((uid, name))
        })
        .collect()
}

/// Collect the process tree rooted at `root_pid`.
///
/// Threads are always collected so the UI can toggle their visibility without
/// waiting for the next refresh cycle.  The returned `Tree` owns the full
/// hierarchy: root node first, child processes and threads as its children.
pub fn collect_tree(
    root_pid: Pid,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
    uid_map: &HashMap<u32, String>,
) -> anyhow::Result<Tree> {
    let proc = Process::new(root_pid.get())?;
    let stat = proc.stat()?;
    let user = resolve_user(&proc, uid_map);

    Ok(std::iter::once(build_node(
        &proc,
        &stat,
        root_pid,
        &user,
        prev_ticks,
        elapsed_secs,
        cfg,
    )?)
    .chain(collect_child_processes(
        root_pid,
        prev_ticks,
        elapsed_secs,
        cfg,
        uid_map,
    )?)
    .chain(collect_threads(
        &proc,
        root_pid,
        &user,
        &stat,
        prev_ticks,
        elapsed_secs,
        cfg,
    ))
    .collect())
}

/// Assemble a single process `Node` from procfs data and sampled metrics.
fn build_node(
    proc: &Process,
    stat: &procfs::process::Stat,
    pid: Pid,
    user: &str,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
) -> anyhow::Result<Node> {
    let (cpu_pct, cpu_time) = sample_task_cpu(pid, stat, prev_ticks, elapsed_secs, cfg);
    let (mem_rss_bytes, mem_pct) = compute_memory(stat, cfg);
    Ok(Node {
        pid,
        name: stat.comm.clone(),
        cmdline: read_cmdline(proc, stat),
        user: user.to_owned(),
        state: stat.state,
        cpu_pct,
        mem_rss_bytes,
        mem_pct,
        io: read_io_totals(proc)?,
        elapsed: compute_elapsed(stat.starttime, cfg.ticks_per_second()),
        cpu_time,
        parent_name: lookup_parent_name(stat.ppid),
        children: Tree::default(),
        is_thread: false,
    })
}

/// Compute CPU utilisation since the last sample for a single task.
///
/// Returns the per-second CPU% and the total accumulated CPU time (utime +
/// stime).  `prev_ticks` is updated in-place so the next call can compute
/// a fresh delta.
fn sample_task_cpu(
    pid: Pid,
    stat: &procfs::process::Stat,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
) -> (Percent, Duration) {
    sample_ticks(pid, stat.utime + stat.stime, prev_ticks, elapsed_secs, cfg)
}

fn sample_ticks(
    pid: Pid,
    current_ticks: u64,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
) -> (Percent, Duration) {
    let delta = current_ticks.saturating_sub(*prev_ticks.get(&pid).unwrap_or(&current_ticks));
    let cpu_pct = if elapsed_secs > 0.0 {
        (delta as f64 / cfg.ticks_per_second() as f64) / elapsed_secs * 100.0
    } else {
        0.0
    };
    prev_ticks.insert(pid, current_ticks);
    let cpu_time = Duration::from_secs_f64(current_ticks as f64 / cfg.ticks_per_second() as f64);
    (Percent::new(cpu_pct), cpu_time)
}

/// Derive resident memory in bytes and as a percentage of total RAM.
///
/// `stat.rss` is measured in pages; we multiply by the page size once here
/// so downstream code never needs to know the page size.
fn compute_memory(stat: &procfs::process::Stat, cfg: &SystemConfig) -> (u64, Percent) {
    let mem_rss_bytes = stat.rss * cfg.page_size();
    let mem_pct = if cfg.mem_total_bytes() > 0 {
        mem_rss_bytes as f64 / cfg.mem_total_bytes() as f64 * 100.0
    } else {
        0.0
    };
    (mem_rss_bytes, Percent::new(mem_pct))
}

/// Read cumulative I/O totals from `/proc/<pid>/io`.
///
/// `/proc/<pid>/io` is only readable by the process owner or root;
/// `PermissionDenied` gracefully falls back to zero rather than failing
/// the entire tree collection.
fn read_io_totals(proc: &Process) -> anyhow::Result<IoTotals> {
    match proc.io() {
        Ok(io) => Ok(IoTotals::new(io.read_bytes, io.write_bytes)),
        Err(procfs::ProcError::PermissionDenied(_)) => Ok(IoTotals::default()),
        Err(e) => Err(e.into()),
    }
}

/// Map the process's effective UID to a username via the pre-loaded passwd map.
///
/// Falls back to the numeric UID string if the username is unknown, or to
/// an empty string if `/proc/<pid>/status` is unreadable.
fn resolve_user(proc: &Process, uid_map: &HashMap<u32, String>) -> String {
    proc.status()
        .ok()
        .map(|s| {
            uid_map
                .get(&s.euid)
                .cloned()
                .unwrap_or_else(|| s.euid.to_string())
        })
        .unwrap_or_default()
}

/// Read the full command line (argv joined by spaces).
///
/// Kernel threads and processes whose `/proc/<pid>/cmdline` is empty fall
/// back to the short `comm` name from stat.
fn read_cmdline(proc: &Process, stat: &procfs::process::Stat) -> String {
    proc.cmdline()
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.join(" "))
        .unwrap_or_else(|| stat.comm.clone())
}

/// Look up the parent process's short name for display context.
///
/// Returns an empty string if the parent has already exited.
fn lookup_parent_name(ppid: i32) -> String {
    Process::new(ppid)
        .and_then(|p| p.stat())
        .map(|s| s.comm)
        .unwrap_or_default()
}

/// Recursively collect direct child processes of `parent_pid`.
///
/// Enumerates all processes via `/proc` and filters to those whose ppid
/// matches.  Each child is itself collected as a full sub-tree.  Processes
/// that vanish mid-collection are silently skipped.
fn collect_child_processes(
    parent_pid: Pid,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
    uid_map: &HashMap<u32, String>,
) -> anyhow::Result<Vec<Node>> {
    Ok(all_processes()?
        .filter_map(|r| r.ok())
        .filter(|p| {
            p.stat()
                .map(|s| s.ppid == parent_pid.get())
                .unwrap_or(false)
        })
        .filter_map(|p| {
            collect_tree(Pid::new(p.pid()), prev_ticks, elapsed_secs, cfg, uid_map)
                .ok()
                .and_then(|t| t.nodes.into_iter().next())
        })
        .collect())
}

/// Collect the threads (tasks) belonging to a process.
///
/// Each thread is represented as a leaf `Node` with `is_thread = true`.
/// The main thread (tid == pid) is excluded since it is the process itself.
/// Thread names are read from `/proc/<pid>/task/<tid>/comm` which provides
/// the full name without the 15-character truncation of `stat.comm`.
fn collect_threads(
    proc: &Process,
    pid: Pid,
    user: &str,
    stat: &procfs::process::Stat,
    prev_ticks: &mut HashMap<Pid, u64>,
    elapsed_secs: f64,
    cfg: &SystemConfig,
) -> Vec<Node> {
    let Ok(tasks) = proc.tasks() else {
        return Vec::new();
    };
    tasks
        .filter_map(|r| r.ok())
        .filter(|t| t.tid != pid.get())
        .filter_map(|task| {
            let tstat = task.stat().ok()?;
            let tid = Pid::new(task.tid);
            let (cpu_pct, cpu_time) = sample_task_cpu(tid, &tstat, prev_ticks, elapsed_secs, cfg);
            let thread_name =
                fs::read_to_string(format!("/proc/{}/task/{}/comm", pid.get(), task.tid))
                    .map(|s| s.trim_end().to_owned())
                    .unwrap_or_else(|_| tstat.comm.clone());
            Some(Node {
                pid: tid,
                name: thread_name.clone(),
                cmdline: thread_name,
                user: user.to_owned(),
                state: tstat.state,
                cpu_pct,
                mem_rss_bytes: tstat.rss * cfg.page_size(),
                mem_pct: Percent::new(0.0),
                io: IoTotals::default(),
                elapsed: Duration::ZERO,
                cpu_time,
                parent_name: stat.comm.clone(),
                children: Tree::default(),
                is_thread: true,
            })
        })
        .collect()
}

/// Linux scheduling policy.
///
/// Mirrors the small set of `SCHED_*` constants exposed via
/// `/proc/<pid>/stat`'s `policy` field.  Unknown values keep the raw integer
/// for diagnostic display.
#[derive(Copy, Clone, Debug)]
pub enum SchedPolicy {
    Normal,
    Fifo,
    RoundRobin,
    Batch,
    Idle,
    Deadline,
    Unknown(u32),
}

impl SchedPolicy {
    fn from_raw(raw: u32) -> Self {
        // Values come from <linux/sched.h>; SCHED_NORMAL=0, FIFO=1, RR=2,
        // BATCH=3, IDLE=5, DEADLINE=6.  Held in the kernel's `policy` field
        // of /proc/<pid>/stat.
        match raw {
            0 => Self::Normal,
            1 => Self::Fifo,
            2 => Self::RoundRobin,
            3 => Self::Batch,
            5 => Self::Idle,
            6 => Self::Deadline,
            other => Self::Unknown(other),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Normal => "OTHER".to_owned(),
            Self::Fifo => "FIFO".to_owned(),
            Self::RoundRobin => "RR".to_owned(),
            Self::Batch => "BATCH".to_owned(),
            Self::Idle => "IDLE".to_owned(),
            Self::Deadline => "DEADLINE".to_owned(),
            Self::Unknown(raw) => format!("?({raw})"),
        }
    }
}

/// Detailed information about a single process or thread, fetched on-demand
/// when the user opens the detail panel.
///
/// `is_thread` selects between two renderer layouts: the process layout
/// shows process-wide context (memory, FDs, environ, cgroup) while the
/// thread layout drops those fields (they belong to the parent process)
/// and emphasises per-task scheduling and activity counters.
pub struct ProcessDetail {
    pub pid: Pid,
    /// True when this PID is a non-leader thread (`tgid != pid`).
    pub is_thread: bool,
    /// Thread group ID — equals the parent process's PID for threads.
    pub tgid: Pid,
    pub state: char,
    pub exe: Option<String>,
    pub cwd: Option<String>,
    pub fd_count: usize,
    /// Soft `RLIMIT_NOFILE` (None for unlimited / unknown).
    pub fd_soft_limit: Option<u64>,
    /// Environment variables as (key, value) pairs, sorted by key.
    pub environ: Vec<(String, String)>,
    pub nice: i64,
    pub priority: i64,
    pub policy: Option<SchedPolicy>,
    pub rt_priority: Option<u32>,
    /// Last logical CPU the task ran on.
    pub last_cpu: Option<i32>,
    /// Resident-set high water mark — peak resident memory, more meaningful
    /// than `VmPeak` (which is the peak *virtual* size).
    pub vm_hwm_kb: Option<u64>,
    pub vm_size_kb: Option<u64>,
    pub vm_rss_kb: Option<u64>,
    pub vm_data_kb: Option<u64>,
    pub vm_stack_kb: Option<u64>,
    pub vm_swap_kb: Option<u64>,
    pub voluntary_ctxt_switches: Option<u64>,
    pub nonvoluntary_ctxt_switches: Option<u64>,
    /// Minor page faults — page reclaims that did not require disk I/O.
    pub minor_faults: u64,
    /// Major page faults — required reading a page from disk; high values
    /// indicate memory pressure or first-touch.
    pub major_faults: u64,
    pub user_cpu_time: Duration,
    pub system_cpu_time: Duration,
    /// Kernel function the task is sleeping in, when applicable.
    pub wchan: Option<String>,
    pub oom_score: Option<u16>,
    pub oom_score_adj: Option<i16>,
    pub thread_count: Option<u64>,
    /// First non-empty cgroup path from `/proc/<pid>/cgroup`.
    pub cgroup: Option<String>,
}

/// Collect detailed per-process information for the detail panel.
///
/// Unlike `collect_tree`, this reads only a single PID and gathers fields
/// that are too expensive or too verbose to include in every tree row.
/// Called only when the panel is open, never during background sampling.
pub fn collect_detail(pid: Pid) -> anyhow::Result<ProcessDetail> {
    // Threads also have `/proc/<tid>` entries; procfs::Process accepts both.
    let proc = Process::new(pid.get())?;
    let stat = proc.stat()?;
    let status = proc.status().ok();

    let tgid = status.as_ref().map(|s| Pid::new(s.tgid)).unwrap_or(pid);
    let is_thread = tgid != pid;

    let ticks = procfs::ticks_per_second().max(1);
    let user_cpu_time = Duration::from_secs_f64(stat.utime as f64 / ticks as f64);
    let system_cpu_time = Duration::from_secs_f64(stat.stime as f64 / ticks as f64);

    // Thread-local fields fall back gracefully — exe()/cwd()/environ() may
    // return permission errors for processes owned by other users.
    let exe = proc.exe().ok().map(|p| p.to_string_lossy().into_owned());
    let cwd = proc.cwd().ok().map(|p| p.to_string_lossy().into_owned());
    let fd_count = proc.fd_count().unwrap_or(0);
    let fd_soft_limit = proc
        .limits()
        .ok()
        .and_then(|l| limit_value_to_u64(&l.max_open_files.soft_limit));

    // Environ is irrelevant for threads (shared with the parent process)
    // and can be expensive to read; skip for threads.
    let environ = if is_thread {
        Vec::new()
    } else {
        let mut env: Vec<(String, String)> = proc
            .environ()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        env.sort_by(|(a, _), (b, _)| a.cmp(b));
        env
    };

    let vm_hwm_kb = status.as_ref().and_then(|s| s.vmhwm);
    let vm_rss_kb = status.as_ref().and_then(|s| s.vmrss);
    let vm_size_kb = status.as_ref().and_then(|s| s.vmsize);
    let vm_data_kb = status.as_ref().and_then(|s| s.vmdata);
    let vm_stack_kb = status.as_ref().and_then(|s| s.vmstk);
    let vm_swap_kb = status.as_ref().and_then(|s| s.vmswap);
    let voluntary_ctxt_switches = status.as_ref().and_then(|s| s.voluntary_ctxt_switches);
    let nonvoluntary_ctxt_switches = status.as_ref().and_then(|s| s.nonvoluntary_ctxt_switches);
    let thread_count = status.as_ref().map(|s| s.threads);

    // wchan returns "0" or empty when the task is running; squash both into
    // None so the renderer can show a placeholder.
    let wchan = proc
        .wchan()
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty() && s != "0");

    let oom_score = proc.oom_score().ok();
    let oom_score_adj = proc.oom_score_adj().ok();

    let cgroup = read_first_cgroup_path(pid);

    Ok(ProcessDetail {
        pid,
        is_thread,
        tgid,
        state: stat.state,
        exe,
        cwd,
        fd_count,
        fd_soft_limit,
        environ,
        nice: stat.nice,
        priority: stat.priority,
        policy: stat.policy.map(SchedPolicy::from_raw),
        rt_priority: stat.rt_priority,
        last_cpu: stat.processor,
        vm_hwm_kb,
        vm_size_kb,
        vm_rss_kb,
        vm_data_kb,
        vm_stack_kb,
        vm_swap_kb,
        voluntary_ctxt_switches,
        nonvoluntary_ctxt_switches,
        minor_faults: stat.minflt,
        major_faults: stat.majflt,
        user_cpu_time,
        system_cpu_time,
        wchan,
        oom_score,
        oom_score_adj,
        thread_count,
        cgroup,
    })
}

/// Translate a procfs `LimitValue` into a plain `Option<u64>` (None = unlimited).
fn limit_value_to_u64(v: &procfs::process::LimitValue) -> Option<u64> {
    match v {
        procfs::process::LimitValue::Value(n) => Some(*n),
        procfs::process::LimitValue::Unlimited => None,
    }
}

/// Read `/proc/<pid>/cgroup` and return the first non-empty cgroup path.
///
/// Cgroup v2 emits a single `0::<path>` line; v1 emits one line per
/// controller.  Either way, the path component (after the second colon)
/// is enough to identify what slice / scope / container the task lives in.
fn read_first_cgroup_path(pid: Pid) -> Option<String> {
    let raw = fs::read_to_string(format!("/proc/{}/cgroup", pid.get())).ok()?;
    raw.lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .find(|p| !p.is_empty() && *p != "/")
        .map(str::to_owned)
}

/// Compute how long the process has been running.
///
/// `starttime` is clock ticks since boot (from `/proc/<pid>/stat`).
fn compute_elapsed(starttime: u64, ticks_per_second: u64) -> Duration {
    // boot_time_secs() avoids the chrono dependency here and returns u64 directly.
    let boot_secs = procfs::boot_time_secs().unwrap_or(0);
    let start_secs = boot_secs.saturating_add(starttime / ticks_per_second.max(1));
    let now_secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(start_secs);
    Duration::from_secs(now_secs.saturating_sub(start_secs))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Append a fully-formed child `Node` to a parent.  Used by builders
    /// that need to construct grandchildren before attaching.
    pub fn push_child(parent: &mut Node, child: Node) {
        parent.children.nodes.push(child);
    }

    /// Build a minimal process `Node` for unit tests.
    pub fn make_test_node(pid: i32, name: &str) -> Node {
        Node {
            pid: Pid::new(pid),
            name: name.to_owned(),
            cmdline: String::new(),
            user: String::new(),
            state: 'S',
            cpu_pct: Percent::new(0.0),
            mem_rss_bytes: 0,
            mem_pct: Percent::new(0.0),
            io: IoTotals::default(),
            elapsed: Duration::ZERO,
            cpu_time: Duration::ZERO,
            parent_name: String::new(),
            children: Tree::default(),
            is_thread: false,
        }
    }

    /// Build a minimal thread `Node` for unit tests.
    pub fn make_test_thread(pid: i32, name: &str) -> Node {
        Node {
            is_thread: true,
            ..make_test_node(pid, name)
        }
    }

    /// Overwrite the `cmdline` field of a `Node`.
    pub fn set_cmdline(node: &mut Node, cmdline: &str) {
        node.cmdline = cmdline.to_owned();
    }

    /// Overwrite the `cpu_time` field of a `Node`.
    pub fn set_cpu_time(node: &mut Node, cpu_time: Duration) {
        node.cpu_time = cpu_time;
    }

    /// Overwrite the `user` field of a `Node`.
    pub fn set_user(node: &mut Node, user: &str) {
        node.user = user.to_owned();
    }

    /// Overwrite the `state` character of a `Node`.
    pub fn set_state(node: &mut Node, state: char) {
        node.state = state;
    }

    /// Overwrite the `cpu_pct` field of a `Node`.
    pub fn set_cpu_pct(node: &mut Node, pct: f64) {
        node.cpu_pct = Percent::new(pct);
    }

    /// Overwrite the resident memory of a `Node`.  `mem_pct` is left at zero;
    /// callers can set it separately if a particular percentage matters.
    pub fn set_mem_rss_bytes(node: &mut Node, bytes: u64) {
        node.mem_rss_bytes = bytes;
    }

    /// Overwrite the memory percentage of a `Node`.
    pub fn set_mem_pct(node: &mut Node, pct: f64) {
        node.mem_pct = Percent::new(pct);
    }

    /// Overwrite the wall-clock elapsed time of a `Node`.
    pub fn set_elapsed(node: &mut Node, d: Duration) {
        node.elapsed = d;
    }

    /// Overwrite the cumulative I/O totals of a `Node`.
    pub fn set_io(node: &mut Node, read: u64, write: u64) {
        node.io = IoTotals::new(read, write);
    }

    /// Overwrite the parent-name field of a `Node`.
    pub fn set_parent_name(node: &mut Node, name: &str) {
        node.parent_name = name.to_owned();
    }

    #[test]
    fn counts_threads_and_subprocesses_recursively() {
        let mut root = make_test_node(1, "root");
        let mut child = make_test_node(2, "child");

        push_child(&mut child, make_test_node(3, "grandchild"));
        push_child(&mut child, make_test_thread(11, "thread-b"));
        push_child(&mut root, child);
        push_child(&mut root, make_test_thread(10, "thread-a"));

        assert_eq!(root.thread_count(), 2);
        assert_eq!(root.subprocess_count(), 2);
    }

    #[test]
    fn sums_subprocess_cpu_time_recursively_excluding_threads() {
        let mut root = make_test_node(1, "root");
        let mut child = make_test_node(2, "child");
        let mut grandchild = make_test_node(3, "grandchild");
        let mut thread = make_test_thread(10, "thread");

        set_cpu_time(&mut root, Duration::from_secs(5));
        set_cpu_time(&mut child, Duration::from_secs(7));
        set_cpu_time(&mut grandchild, Duration::from_secs(11));
        set_cpu_time(&mut thread, Duration::from_secs(13));

        push_child(&mut child, grandchild);
        push_child(&mut root, child);
        push_child(&mut root, thread);

        assert_eq!(root.subprocess_cpu_time(), Duration::from_secs(18));
    }
}
