// SPDX-License-Identifier: GPL-3.0-or-later
//! The process confines its own file access with Landlock (ADR-0111): a
//! second wall behind Warpine's own checks (ADR-0066), should a guest ever
//! escape into Warpine's process.
//!
//! Every Warpine process (programs, CMD.EXE, the desktop server) calls
//! [`confine`] before guest code runs. From then on the kernel lets it
//! reach only:
//! - **read and write:** the drives' host directories (read only for a
//!   read-only drive), the runtime
//!   directory (the session's sockets and lock files), `/dev` (KVM, the
//!   GPU, sound; Unix permissions still apply), the crash-dump directory,
//!   and the files a GUI script or `WARPINE_DESKTOP_LOG` names;
//! - **read:** the system's files (`/usr`, `/etc`, `/sys`, `/proc`,
//!   `/run`), the program's own directory (its DLLs), and the few files
//!   in the home directory the display and sound need;
//! - **execute:** Warpine's own binary (it starts its children and the
//!   desktop server) and the shared libraries.
//!
//! Everything else in the home directory is out of reach. Landlock is
//! inherited: the processes a confined one starts are confined too.
//! `WARPINE_SANDBOX=off` leaves the process unconfined; a kernel without
//! Landlock leaves it unconfined with a warning.

use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};
use std::path::{Path, PathBuf};

/// The environment variable that turns the sandbox off (`off`).
pub const SANDBOX_ENV: &str = "WARPINE_SANDBOX";

/// `warpine --sandbox-check <path>`: confine as a program with no drives
/// would, then say whether `path` can still be read.
pub const CHECK_SWITCH: &str = "--sandbox-check";

/// The Landlock ABI the policy is written for; older kernels apply what
/// they know of it (best effort).
const POLICY_ABI: ABI = ABI::V5;

/// System directories read, never written.
const SYSTEM_READ: &[&str] = &["/usr", "/etc", "/sys", "/proc", "/run", "/var/lib/dbus", "/bin", "/sbin"];
/// Library directories, from which the dynamic loader maps and runs code
/// (the interpreter of Warpine's own binary when it starts its children).
const LIBRARIES: &[&str] = &["/lib", "/lib32", "/lib64", "/libx32", "/usr/lib", "/usr/lib32", "/usr/lib64",
                             "/usr/libexec"];
/// Files and directories under the home directory the display and sound
/// read: the X authority file, cursor themes, PulseAudio's cookie.
const HOME_READ: &[&str] = &[".Xauthority", ".icons", ".local/share/icons", ".config/pulse"];

/// What a process may reach.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub read_write: Vec<PathBuf>,
    pub read: Vec<PathBuf>,
    pub execute: Vec<PathBuf>,
}

/// What [`confine`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Landlock enforces the policy (fully, or what the kernel knows of it).
    Confined { fully: bool },
    /// `WARPINE_SANDBOX=off`.
    Disabled,
    /// The kernel has no Landlock.
    Unsupported,
}

/// Whether the environment turns the sandbox off.
pub fn disabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v.eq_ignore_ascii_case("off") || v == "0" || v.eq_ignore_ascii_case("no"))
}

/// A drive's host directory, and whether the drive is read-only.
pub type Drive = (PathBuf, bool);

/// The policy for a process with `drives`, running the program at
/// `program` (None: the desktop server, CMD.EXE).
pub fn policy(drives: &[Drive], program: Option<&Path>, env: &dyn Fn(&str) -> Option<PathBuf>) -> Policy {
    let mut p = Policy::default();
    for (dir, read_only) in drives {
        if *read_only { p.read.push(dir.clone()) } else { p.read_write.push(dir.clone()) }
    }
    p.read_write.push(env("XDG_RUNTIME_DIR").unwrap_or_else(std::env::temp_dir));
    p.read_write.push(PathBuf::from("/dev"));
    p.read_write.push(crate::loader::crash_dump::crash_dir());
    // Test and automation aids: the desktop server's log, a GUI script's
    // dumps (their directories, where the files are made).
    if let Some(log) = env(crate::desktop::client::DESKTOP_LOG_ENV) {
        p.read_write.extend(log.parent().map(Path::to_path_buf));
    }
    if let Some(script) = env(crate::gui::gui_script::SCRIPT_ENV) {
        p.read.push(script.clone());
        if let Ok(text) = std::fs::read_to_string(&script)
            && let Ok(steps) = crate::gui::gui_script::parse(&text) {
            for step in steps {
                if let crate::gui::gui_script::Action::Dump(path) = step.action {
                    p.read_write.extend(path.parent().map(Path::to_path_buf));
                }
            }
        }
    }
    p.read.extend(SYSTEM_READ.iter().map(PathBuf::from));
    if let Some(home) = env("HOME") {
        p.read.extend(HOME_READ.iter().map(|f| home.join(f)));
    }
    p.read.extend(env("XAUTHORITY"));
    if let Some(program) = program {
        p.read.extend(program.parent().map(|d| if d.as_os_str().is_empty() { PathBuf::from(".") } else { d.to_path_buf() }));
        p.read.push(program.to_path_buf());
    }
    p.execute.extend(LIBRARIES.iter().map(PathBuf::from));
    p.execute.extend(std::env::current_exe().ok());
    p
}

/// Confine this process to `policy`, unless `WARPINE_SANDBOX=off`. Paths
/// that do not exist are left out (the crash directory is made first).
pub fn confine(policy: &Policy) -> Result<Outcome, String> {
    if disabled_by(std::env::var_os(SANDBOX_ENV).as_deref()) {
        return Ok(Outcome::Disabled);
    }
    let _ = std::fs::create_dir_all(crate::loader::crash_dump::crash_dir());
    let all = AccessFs::from_all(POLICY_ABI);
    let read = AccessFs::ReadFile | AccessFs::ReadDir;
    let execute = read | AccessFs::Execute;
    // A rule for a single file takes only the rights that apply to files.
    let rules = |paths: &[PathBuf], access: BitFlags<AccessFs>| {
        paths.iter().filter_map(|p| {
            let rights = if p.is_dir() { access } else { access & AccessFs::from_file(POLICY_ABI) };
            PathFd::new(p).ok().map(|fd| (fd, rights))
        })
        .map(|(fd, rights)| Ok::<_, landlock::RulesetError>(PathBeneath::new(fd, rights)))
        .collect::<Vec<_>>()
    };
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all).map_err(|e| e.to_string())?
        .create().map_err(|e| e.to_string())?
        .add_rules(rules(&policy.read_write, all)).map_err(|e| e.to_string())?
        .add_rules(rules(&policy.read, read)).map_err(|e| e.to_string())?
        .add_rules(rules(&policy.execute, execute)).map_err(|e| e.to_string())?
        .restrict_self().map_err(|e| e.to_string())?;
    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => Outcome::Confined { fully: true },
        RulesetStatus::PartiallyEnforced => Outcome::Confined { fully: false },
        RulesetStatus::NotEnforced => Outcome::Unsupported,
    })
}

/// [`policy`] and [`confine`] for this process, reporting what happened:
/// a failure or a kernel without Landlock is a warning, not an error.
pub fn confine_process(drives: &[Drive], program: Option<&Path>) {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    match confine(&policy(drives, program, &env)) {
        Ok(Outcome::Confined { fully }) => log::debug!("sandbox: Landlock confines this process{}",
                                                       if fully { "" } else { " (partly: an older kernel)" }),
        Ok(Outcome::Disabled) => log::debug!("sandbox: off ({SANDBOX_ENV})"),
        Ok(Outcome::Unsupported) => log::warn!("sandbox: this kernel has no Landlock; the process runs unconfined"),
        Err(e) => log::warn!("sandbox: Landlock failed ({e}); the process runs unconfined"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_and_its_spellings_turn_it_off() {
        use std::ffi::OsStr;
        for v in ["off", "OFF", "0", "no"] {
            assert!(disabled_by(Some(OsStr::new(v))), "{v}");
        }
        for v in ["on", "1", ""] {
            assert!(!disabled_by(Some(OsStr::new(v))), "{v}");
        }
        assert!(!disabled_by(None), "on by default");
    }

    #[test]
    fn the_policy_grants_the_drives_and_the_session_and_reads_the_rest() {
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some(PathBuf::from("/run/user/1000")),
            "HOME" => Some(PathBuf::from("/home/u")),
            _ => None,
        };
        let p = policy(&[(PathBuf::from("/home/u/.local/share/warpine/drive_c"), false),
                         (PathBuf::from("/mnt/cd"), true)],
                       Some(Path::new("/home/u/apps/prog.exe")), &env);
        for rw in ["/home/u/.local/share/warpine/drive_c", "/run/user/1000", "/dev"] {
            assert!(p.read_write.contains(&PathBuf::from(rw)), "{rw} read-write");
        }
        assert!(p.read.contains(&PathBuf::from("/mnt/cd")) && !p.read_write.contains(&PathBuf::from("/mnt/cd")),
                "a read-only drive is read only");
        assert!(p.read.contains(&PathBuf::from("/home/u/apps")), "the program's directory, for its DLLs");
        assert!(p.read.contains(&PathBuf::from("/home/u/.Xauthority")));
        assert!(!p.read.iter().chain(&p.read_write).any(|d| d == Path::new("/home/u")),
                "never the whole home directory");
        assert!(p.execute.contains(&PathBuf::from("/usr/lib")));
        assert!(!p.execute.iter().any(|d| d == Path::new("/usr/bin")), "no other program to run");
    }

    #[test]
    fn a_program_named_without_a_directory_reads_the_current_one() {
        let p = policy(&[], Some(Path::new("prog.exe")), &|_| None);
        assert!(p.read.contains(&PathBuf::from(".")));
    }
}
