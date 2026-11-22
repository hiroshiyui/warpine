// SPDX-License-Identifier: GPL-3.0-or-later
//
//! A cgroup for each Warpine session (ADR-0116): the session's first
//! process asks the systemd user manager for a transient scope holding
//! itself, with a memory and a task limit, before it runs guest code. The
//! processes it starts (its children, the desktop server) inherit the
//! cgroup, so one runaway or hostile program cannot take the machine's
//! memory or tasks; the kernel stops the session at its limits.
//!
//! The request is `StartTransientUnit` on the user manager's D-Bus
//! interface (systemd's `org.freedesktop.systemd1(5)`), sent with
//! `busctl`, as `systemd-run --scope` sends it. Without a reachable user
//! manager (no systemd, a container, a private `XDG_RUNTIME_DIR`) the
//! session runs without the limits, with a warning.
//!
//! `WARPINE_CGROUP=off` skips it; `WARPINE_SESSION_MEMORY_MAX` (bytes, or
//! with a K, M, G or T suffix) and `WARPINE_SESSION_TASKS_MAX` set the
//! limits (8 GB and 8192 by default).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Turns the session's cgroup off when set to `off`.
pub const CGROUP_ENV: &str = "WARPINE_CGROUP";
/// The session's memory limit (`MemoryMax`).
pub const MEMORY_MAX_ENV: &str = "WARPINE_SESSION_MEMORY_MAX";
/// The session's task limit (`TasksMax`): processes and threads.
pub const TASKS_MAX_ENV: &str = "WARPINE_SESSION_TASKS_MAX";
/// 8 GB: room for a few dozen processes of 256 MB of guest memory each.
pub const DEFAULT_MEMORY_MAX: u64 = 8 << 30;
/// Enough for a process at its 738 threads (ADR-0115) and many more.
pub const DEFAULT_TASKS_MAX: u64 = 8192;
/// The tool that sends the request, from systemd.
const BUSCTL: &str = "busctl";
/// How long to wait for the user manager to move this process.
const MOVE_WAIT: Duration = Duration::from_millis(500);

/// The limits of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub memory_max: u64,
    pub tasks_max: u64,
}

/// A size in bytes: a number, optionally followed by K, M, G or T (powers
/// of 1024, either case). None for anything else, or zero.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, shift) = match text.chars().last()?.to_ascii_uppercase() {
        'K' => (&text[..text.len() - 1], 10),
        'M' => (&text[..text.len() - 1], 20),
        'G' => (&text[..text.len() - 1], 30),
        'T' => (&text[..text.len() - 1], 40),
        _ => (text, 0),
    };
    let n: u64 = digits.parse().ok()?;
    n.checked_mul(1u64 << shift).filter(|&b| b > 0)
}

/// The limits `env` asks for; a value that does not parse keeps the
/// default, with a warning.
pub fn limits(env: &dyn Fn(&str) -> Option<String>) -> Limits {
    let read = |name: &str, default: u64, parse: &dyn Fn(&str) -> Option<u64>| match env(name) {
        None => default,
        Some(v) => parse(&v).unwrap_or_else(|| {
            log::warn!("{name}={v:?} is not a valid limit; using {default}");
            default
        }),
    };
    Limits {
        memory_max: read(MEMORY_MAX_ENV, DEFAULT_MEMORY_MAX, &parse_size),
        tasks_max: read(TASKS_MAX_ENV, DEFAULT_TASKS_MAX, &|v| v.trim().parse().ok().filter(|&n| n > 0)),
    }
}

/// The name of the scope for the session whose first process is `pid`.
pub fn unit_name(pid: u32) -> String {
    format!("warpine-session-{pid}.scope")
}

/// `busctl`'s arguments for a scope `unit` holding process `pid`, with
/// `limits`; the scope is collected when it ends, also after a failure.
pub fn busctl_args(unit: &str, pid: u32, limits: Limits) -> Vec<String> {
    [
        "--user", "--quiet", "call",
        "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager",
        "StartTransientUnit", "ssa(sv)a(sa(sv))", unit, "fail",
        "5",
        "Description", "s", "Warpine session",
        "PIDs", "au", "1", &pid.to_string(),
        "MemoryMax", "t", &limits.memory_max.to_string(),
        "TasksMax", "t", &limits.tasks_max.to_string(),
        "CollectMode", "s", "inactive-or-failed",
        "0",
    ].iter().map(|s| s.to_string()).collect()
}

/// Whether this process should make the session's scope: it is the
/// session's first (no Warpine parent gave it `WARPINE_PPID`) and the
/// scope is not turned off.
pub fn wanted(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env(crate::loader::inherit::PPID_ENV).is_none()
        && !env(CGROUP_ENV).is_some_and(|v| v.eq_ignore_ascii_case("off"))
}

/// Where the cgroup v2 hierarchy is mounted.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Whether moving out of the cgroup at `path` (as `/proc/self/cgroup`
/// gives it, under the hierarchy at `root`) into a new scope with `limits`
/// would loosen a limit: a `memory.max` or `pids.max` below the new
/// scope's on a cgroup of the path that is not also above the new scope
/// (systemd's default `TasksMax`, on nearly every unit, is far above
/// ours). The user manager puts
/// the scope in its `app.slice`, so limits from there up (systemd's own
/// `TasksMax` on the user's slice) stay; the ones below it, such as the
/// tests' and the pre-commit hook's `systemd-run --scope`, would be left
/// behind, and the process stays. Without a user manager on the path (a
/// system service, a container) any limit on it keeps the process.
pub fn would_escape_limits(root: &std::path::Path, path: &str, limits: Limits) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    // How many leading parts the new scope shares: those up to the user
    // manager's app.slice.
    let shared = match parts.iter().position(|p| p.starts_with("user@") && p.ends_with(".service")) {
        Some(manager) => manager + 1 + usize::from(parts.get(manager + 1) == Some(&"app.slice")),
        None => 0,
    };
    (shared + 1..=parts.len()).any(|depth| {
        let dir = parts[..depth].iter().fold(root.to_path_buf(), |d, p| d.join(p));
        let below = |file: &str, ours: u64| std::fs::read_to_string(dir.join(file))
            .is_ok_and(|v| v.trim().parse::<u64>().is_ok_and(|n| n < ours));
        below("memory.max", limits.memory_max) || below("pids.max", limits.tasks_max)
    })
}

/// This process's cgroup v2 path (`0::<path>` in `/proc/self/cgroup`).
fn own_cgroup() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    text.lines().find_map(|l| l.strip_prefix("0::")).map(str::to_string)
}

/// Put this process, the session's first, into a scope of its own with
/// the session's limits (see the module documentation). Never fails the
/// program: without a user manager it says so and goes on.
pub fn enter_session_scope() {
    let env = |name: &str| std::env::var(name).ok();
    if !wanted(&env) {
        return;
    }
    let limits = limits(&env);
    if let Some(path) = own_cgroup()
        && would_escape_limits(std::path::Path::new(CGROUP_ROOT), &path, limits) {
        log::info!("the session stays in its cgroup {path}, whose limits are tighter than a scope of its own");
        return;
    }
    let pid = std::process::id();
    let unit = unit_name(pid);
    let status = Command::new(BUSCTL).args(busctl_args(&unit, pid, limits))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
    match status {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            log::warn!("no session cgroup (systemd user manager: {}); the session runs without memory and task limits \
                        ({CGROUP_ENV}=off hides this)", String::from_utf8_lossy(&out.stderr).trim());
            return;
        }
        Err(e) => {
            log::warn!("no session cgroup ({BUSCTL}: {e}); the session runs without memory and task limits \
                        ({CGROUP_ENV}=off hides this)");
            return;
        }
    }
    // The manager moves the process when the scope's start job runs.
    let start = Instant::now();
    while start.elapsed() < MOVE_WAIT {
        if std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| c.contains(&unit)) {
            log::info!("session cgroup {unit}: MemoryMax={}, TasksMax={}", limits.memory_max, limits.tasks_max);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    log::warn!("the session cgroup {unit} was started, but this process is not in it yet");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn sizes_take_binary_suffixes() {
        assert_eq!(parse_size("8G"), Some(8 << 30));
        assert_eq!(parse_size(" 512m "), Some(512 << 20));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("1t"), Some(1 << 40));
        for bad in ["", "G", "0", "0K", "-1G", "1.5G", "8GB", "99999999999T"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn limits_default_and_bad_values_keep_the_default() {
        assert_eq!(limits(&env_of(&[])), Limits { memory_max: DEFAULT_MEMORY_MAX, tasks_max: DEFAULT_TASKS_MAX });
        assert_eq!(limits(&env_of(&[(MEMORY_MAX_ENV, "2G"), (TASKS_MAX_ENV, "100")])),
                   Limits { memory_max: 2 << 30, tasks_max: 100 });
        assert_eq!(limits(&env_of(&[(MEMORY_MAX_ENV, "lots"), (TASKS_MAX_ENV, "0")])),
                   Limits { memory_max: DEFAULT_MEMORY_MAX, tasks_max: DEFAULT_TASKS_MAX });
    }

    /// Only a session's first process makes the scope, unless it is off.
    #[test]
    fn only_the_first_process_makes_the_scope() {
        assert!(wanted(&env_of(&[])));
        assert!(!wanted(&env_of(&[("WARPINE_PPID", "12")])), "a child inherits its parent's cgroup");
        assert!(!wanted(&env_of(&[(CGROUP_ENV, "OFF")])));
        assert!(wanted(&env_of(&[(CGROUP_ENV, "on")])));
    }

    /// Limits below the user manager's app.slice keep the process where
    /// it is; limits the new scope shares do not; without a user manager
    /// any limit keeps it.
    #[test]
    fn only_limits_a_new_scope_would_leave_keep_the_process() {
        let root = tempfile::tempdir().unwrap();
        let write = |dir: &str, file: &str, v: &str| {
            let d = root.path().join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), v).unwrap();
        };
        const MANAGER: &str = "user.slice/user-1000.slice/user@1000.service";
        const TERM: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/term.scope/tab.scope";
        write(MANAGER, "memory.max", "max\n");
        write(&format!("{MANAGER}/app.slice"), "pids.max", "9000\n");
        write(&format!("{MANAGER}/app.slice/term.scope/tab.scope"), "memory.max", "max\n");
        let ours = Limits { memory_max: DEFAULT_MEMORY_MAX, tasks_max: DEFAULT_TASKS_MAX };
        let escapes = |path: &str| would_escape_limits(root.path(), path, ours);
        write("user.slice/user-1000.slice", "pids.max", "100\n");
        assert!(!escapes(TERM), "the user's slice and app.slice: shared with the new scope");
        write(&format!("{MANAGER}/app.slice/term.scope"), "pids.max", "76609\n");
        assert!(!escapes(TERM), "systemd's default TasksMax, looser than ours");
        write(&format!("{MANAGER}/app.slice/term.scope"), "pids.max", "100\n");
        assert!(escapes(TERM), "the terminal's own, tighter limit would be left");
        write(&format!("{MANAGER}/app.slice/run-1.scope"), "pids.max", "4096\n");
        assert!(escapes(&format!("/{MANAGER}/app.slice/run-1.scope")), "the hook's scope: 4096 tasks");
        write(&format!("{MANAGER}/app.slice/run-2.scope"), "memory.max", "4294967296\n");
        assert!(escapes(&format!("/{MANAGER}/app.slice/run-2.scope")), "4 GB of memory");
        write(&format!("{MANAGER}/app.slice/run-3.scope"), "memory.max", "25769803776\n");
        assert!(!escapes(&format!("/{MANAGER}/app.slice/run-3.scope")), "24 GB: looser than ours");
        write("system.slice/x.service", "memory.max", "max\n");
        assert!(!escapes("/system.slice/x.service"));
        write("system.slice", "pids.max", "500\n");
        assert!(escapes("/system.slice/x.service"), "no user manager: any tighter limit on the path");
        assert!(!escapes("/"));
    }

    /// The request: one scope with this process, the two limits, and
    /// collection after it ends (StartTransientUnit's signature
    /// `ssa(sv)a(sa(sv))`: name, mode, properties, no auxiliary units).
    #[test]
    fn the_request_names_the_process_and_the_limits() {
        let args = busctl_args("warpine-session-42.scope", 42, Limits { memory_max: 1 << 30, tasks_max: 64 });
        let at = |s: &str| args.iter().position(|a| a == s).unwrap_or_else(|| panic!("{s} in {args:?}"));
        assert_eq!(&args[..3], ["--user", "--quiet", "call"]);
        assert_eq!(args[at("StartTransientUnit") + 1..at("StartTransientUnit") + 5],
                   ["ssa(sv)a(sa(sv))", "warpine-session-42.scope", "fail", "5"]);
        assert_eq!(args[at("PIDs") + 1..at("PIDs") + 4], ["au", "1", "42"]);
        assert_eq!(args[at("MemoryMax") + 1..at("MemoryMax") + 3], ["t", "1073741824"]);
        assert_eq!(args[at("TasksMax") + 1..at("TasksMax") + 3], ["t", "64"]);
        assert_eq!(args.last().map(String::as_str), Some("0"), "no auxiliary units");
        // busctl's 3, the destination's 3, method, signature, name and
        // mode, the property count, five properties of name, type and
        // value (PIDs' array its length too), no auxiliary units.
        assert_eq!(args.len(), 3 + 3 + 4 + 1 + 5 * 3 + 1 + 1);
        assert_eq!(unit_name(42), "warpine-session-42.scope");
    }
}
