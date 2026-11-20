// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's batch control (ADR-0078): a `.CMD` script runs
// line by line from a position GOTO can move, with its arguments as
// %0–%9, ECHO ON / OFF and `@`, IF, FOR, CALL, SHIFT, SETLOCAL /
// ENDLOCAL and PAUSE. Syntax as OS/2's CMD.EXE.

use super::*;
use super::parse::{first_word, parse_for, parse_if, IfCondition};
use crate::loader::vfs::FileAttribute;

/// The most scripts running inside one another (CALL, or a script started
/// from a script): each is a nested call on the host's stack.
const MAX_SCRIPT_DEPTH: usize = 32;
/// The largest script read.
const MAX_SCRIPT_BYTES: u64 = 4 * 1024 * 1024;
/// The most SETLOCALs a script keeps open at once.
const MAX_LOCAL_DEPTH: usize = 32;
/// The most items a FOR runs its command for (after wildcards).
const MAX_FOR_ITEMS: usize = 65536;
/// What PAUSE writes (Warpine's wording, unverified).
const PAUSE_PROMPT: &str = "Press any key when ready . . .";

/// A running `.CMD` script.
pub(super) struct Batch {
    lines: Vec<String>,
    /// The index of the line to run next.
    next: usize,
    /// `%0` (the name as typed), then the arguments.
    pub(super) params: Vec<String>,
    /// What each open SETLOCAL saved.
    locals: Vec<Local>,
    /// Set by GOTO: the rest of the current line is not run.
    jumped: bool,
    /// Set when the script started another script without CALL: control
    /// does not come back to it.
    ended: bool,
}

/// What SETLOCAL saves: the environment and the current drive and
/// directories.
struct Local {
    env: Vec<(String, String)>,
    cwd: Vec<u8>,
}

/// How a script ended.
pub(super) struct ScriptEnd {
    /// Its return code: EXIT's, or that of its last command.
    pub rc: u32,
    /// EXIT ran in it: the whole shell ends.
    pub exit: bool,
}

impl CmdShell {
    /// Run the script at `path` with `params` (`%0`, then its arguments),
    /// in this shell (`depth` scripts deep). Lines are echoed with the
    /// prompt while ECHO is on, except those starting with `@`.
    pub(super) fn run_script(&mut self, path: &std::path::Path, params: Vec<String>) -> ScriptEnd {
        let failed = |shell: &Self, msg: &str| {
            shell.error(msg);
            ScriptEnd { rc: RC_BUILTIN_FAILED, exit: false }
        };
        if self.depth >= MAX_SCRIPT_DEPTH {
            return failed(self, "Too many scripts running inside one another.");
        }
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SCRIPT_BYTES) {
            return failed(self, "The script is too large.");
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => return failed(self, &format!("Cannot open script: {e}")),
        };
        // A script is text in the process codepage.
        let codepage = self.shared.active_codepage.load(Ordering::Relaxed);
        let text = crate::loader::codepage::cp_decode(&bytes, codepage);
        let lines = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        *self.batch.borrow_mut() = Some(Batch { lines, next: 0, params, locals: Vec::new(), jumped: false, ended: false });

        let mut exit = None;
        loop {
            if self.shared.exit_requested.load(Ordering::Relaxed) { break; }
            let line = {
                let mut batch = self.batch.borrow_mut();
                let Some(b) = batch.as_mut() else { break };
                if b.ended || b.next >= b.lines.len() { break; }
                b.next += 1;
                b.jumped = false;
                b.lines[b.next - 1].clone()
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') { continue; }
            if self.echo_on.get() && !trimmed.starts_with('@') {
                let shown = self.expand_line(trimmed);
                self.write_attr(self.build_prompt().as_bytes(), ATTR_HEADER);
                self.writeln(&shown);
            }
            if let Some(code) = self.execute_line(trimmed) {
                exit = Some(code);
                break;
            }
        }
        // An open SETLOCAL ends with its script.
        while self.end_local() {}
        *self.batch.borrow_mut() = None;
        match exit {
            Some(code) => ScriptEnd { rc: code, exit: true },
            None => ScriptEnd { rc: self.last_rc.get(), exit: false },
        }
    }

    /// `line` expanded: `%name%` and `%%`, and in a script `%0`–`%9`.
    pub(super) fn expand_line(&self, line: &str) -> String {
        let batch = self.batch.borrow();
        expand_vars(line, &self.env.borrow(), batch.as_ref().map(|b| b.params.as_slice()))
    }

    /// Whether a GOTO ran in the line being run.
    pub(super) fn jumped(&self) -> bool {
        self.batch.borrow().as_ref().is_some_and(|b| b.jumped || b.ended)
    }

    /// The script `name` (typed so) runs in a nested shell with `args`;
    /// returns how it ended. Without CALL, a script started from a script
    /// takes its place: the caller does not go on afterwards.
    pub(super) fn run_nested_script(&self, path: &std::path::Path, typed: &str, args: &[&str], call: bool) -> ScriptEnd {
        // A script shares the shell's environment (its SETs stay), current
        // output and input (`script > file`, `x | script`) and ECHO state.
        let mut shell = CmdShell::new(Arc::clone(&self.shared), self.env.borrow().clone());
        shell.depth = self.depth + 1;
        let in_script = self.batch.borrow().is_some();
        shell.echo_on.set(if in_script { self.echo_on.get() } else { true });
        shell.output_sink.swap(&self.output_sink);
        shell.input_source.swap(&self.input_source);
        let mut params = vec![typed.to_string()];
        params.extend(args.iter().map(|a| a.to_string()));
        let end = shell.run_script(path, params);
        shell.output_sink.swap(&self.output_sink);
        shell.input_source.swap(&self.input_source);
        *self.env.borrow_mut() = shell.env.into_inner();
        if in_script {
            // ECHO is one state for a script and the scripts it runs.
            self.echo_on.set(shell.echo_on.get());
            if !call && let Some(b) = self.batch.borrow_mut().as_mut() { b.ended = true; }
        }
        end
    }

    // ── Commands ─────────────────────────────────────────────────────────────

    /// `ECHO` (shows the state), `ECHO ON` / `ECHO OFF`, `ECHO text`
    /// (written as typed, blanks included).
    pub(super) fn cmd_echo(&self, text: &str) {
        match text.trim() {
            "" => self.writeln(if self.echo_on.get() { "ECHO is on." } else { "ECHO is off." }),
            t if t.eq_ignore_ascii_case("ON") => self.echo_on.set(true),
            t if t.eq_ignore_ascii_case("OFF") => self.echo_on.set(false),
            _ => self.writeln(text),
        }
    }

    /// `GOTO label`: go on after the line `:label` (the label's first word,
    /// matched without case). A label that is not there ends the script.
    /// Nothing at the prompt.
    pub(super) fn cmd_goto(&self, rest: &str) {
        let (label, _) = first_word(rest);
        let label = label.trim_start_matches(':');
        let mut batch = self.batch.borrow_mut();
        let Some(b) = batch.as_mut() else { return };
        let target = b.lines.iter().position(|l| {
            l.trim().strip_prefix(':').is_some_and(|l| first_word(l).0.eq_ignore_ascii_case(label))
        });
        b.jumped = true;
        match target {
            Some(i) => b.next = i + 1,
            None => {
                b.ended = true;
                drop(batch);
                self.error(&format!("The label {label} was not found."));
            }
        }
    }

    /// `SHIFT`: `%1` becomes `%0`, `%2` becomes `%1`, … Nothing at the prompt.
    pub(super) fn cmd_shift(&self) {
        if let Some(b) = self.batch.borrow_mut().as_mut() && !b.params.is_empty() {
            b.params.remove(0);
        }
    }

    /// `SETLOCAL`: save the environment and the current drive and
    /// directories until ENDLOCAL or the script's end.
    pub(super) fn cmd_setlocal(&self) {
        let cwd = crate::loader::inherit::cwd_strings(&self.shared.drive_mgr.lock_or_recover(), None);
        let mut batch = self.batch.borrow_mut();
        let Some(b) = batch.as_mut() else { return };
        if b.locals.len() >= MAX_LOCAL_DEPTH {
            drop(batch);
            self.error("Too many SETLOCALs.");
            return;
        }
        b.locals.push(Local { env: self.env.borrow().clone(), cwd });
    }

    /// End the innermost SETLOCAL: put back what it saved. Whether there
    /// was one.
    pub(super) fn end_local(&self) -> bool {
        let local = self.batch.borrow_mut().as_mut().and_then(|b| b.locals.pop());
        let Some(local) = local else { return false };
        *self.env.borrow_mut() = local.env;
        crate::loader::inherit::apply_cwd(&mut self.shared.drive_mgr.lock_or_recover(), &local.cwd);
        true
    }

    /// `PAUSE`: wait for a key (a byte of a pipe or file read as input).
    pub(super) fn cmd_pause(&self) {
        self.writeln(PAUSE_PROMPT);
        let from_data = match &mut *self.input_source.borrow_mut() {
            InputSource::Data(data) => { data.pop_front(); true }
            InputSource::Kbd => false,
        };
        if !from_data { let _ = self.read_key(); }
    }

    /// `IF [NOT] condition command`. The command runs when the condition
    /// holds (or not, with NOT) and leaves its return code; otherwise the
    /// return code stays as it was. Returns the command's exit request.
    pub(super) fn cmd_if(&mut self, rest: &str) -> Option<u32> {
        let Some(cmd) = parse_if(rest) else {
            self.error("The syntax of the command is incorrect.");
            self.last_rc.set(RC_BUILTIN_FAILED);
            return None;
        };
        let holds = match &cmd.condition {
            IfCondition::ErrorLevel(n) => self.last_rc.get() >= *n,
            IfCondition::Exist(name) => self.exists(name),
            IfCondition::Equal(a, b) => a == b,
        };
        if holds != cmd.not { self.execute_segment(&cmd.command) } else { None }
    }

    /// Whether a file or directory matches `pattern` (wildcards allowed).
    pub(super) fn exists(&self, pattern: &str) -> bool {
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        match dm.find_first(pattern, FileAttribute(0x37), 1) {
            Ok((handle, _)) => { let _ = dm.find_close(handle); true }
            Err(_) => false,
        }
    }

    /// `FOR %v IN (set) DO command`: run the command once for each item,
    /// `%v` replaced by it. An item with `*` or `?` stands for the files
    /// it matches (not directories), with the directory as written.
    pub(super) fn cmd_for(&mut self, rest: &str) -> Option<u32> {
        let Some(cmd) = parse_for(rest) else {
            self.error("The syntax of the command is incorrect.");
            self.last_rc.set(RC_BUILTIN_FAILED);
            return None;
        };
        let mut items = Vec::new();
        for item in &cmd.items {
            if item.contains(['*', '?']) { items.extend(self.matching_files(item)); } else { items.push(item.clone()); }
            if items.len() >= MAX_FOR_ITEMS { items.truncate(MAX_FOR_ITEMS); break; }
        }
        let var = format!("%{}", cmd.var);
        for item in items {
            if self.shared.exit_requested.load(Ordering::Relaxed) || self.jumped() { break; }
            let command = cmd.command.replace(&var, &item);
            if let Some(code) = self.execute_segment(&command) { return Some(code); }
        }
        None
    }

    /// The files `pattern` matches, in the directory's order, each with
    /// the pattern's directory part.
    pub(super) fn matching_files(&self, pattern: &str) -> Vec<String> {
        let dir = pattern.rfind(['\\', '/', ':']).map_or("", |i| &pattern[..=i]);
        let mut dm = self.shared.drive_mgr.lock_or_recover();
        let Ok((handle, first)) = dm.find_first(pattern, FileAttribute(0x37), 1) else { return Vec::new() };
        let mut names = Vec::new();
        let mut entry = Some(first);
        while let Some(e) = entry {
            if !e.status.attributes.contains(FileAttribute::DIRECTORY) && names.len() < MAX_FOR_ITEMS {
                names.push(format!("{dir}{}", e.name));
            }
            entry = dm.find_next(handle).ok();
        }
        let _ = dm.find_close(handle);
        names
    }

    /// `CALL name args`: run a script and come back (or a program, as
    /// without CALL).
    pub(super) fn cmd_call(&mut self, rest: &str) -> Option<u32> {
        let tokens = tokenize(rest);
        let Some((&name, args)) = tokens.split_first() else {
            self.error("The syntax of the command is incorrect.");
            self.last_rc.set(RC_BUILTIN_FAILED);
            return None;
        };
        self.exec_external(name, args, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;
    use crate::loader::vfs_hostdir::HostDirBackend;

    /// A mock loader whose drive C is a temporary directory holding
    /// `files` (name, contents; a name ending in `\` is a directory).
    fn drive_with(files: &[(&str, &str)]) -> (Loader, tempfile::TempDir) {
        let loader = Loader::new_mock();
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let name = name.replace('\\', "/");
            match name.strip_suffix('/') {
                Some(d) => std::fs::create_dir_all(dir.path().join(d)).unwrap(),
                None => std::fs::write(dir.path().join(name), text.replace('\n', "\r\n")).unwrap(),
            }
        }
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(HostDirBackend::new(dir.path().to_path_buf()).unwrap()));
            dm.set_current_disk(3).unwrap();
        }
        (loader, dir)
    }

    /// Run `line` in a fresh shell writing into a buffer; returns what it
    /// wrote, the exit request and the shell.
    fn run(loader: &Loader, line: &str) -> (String, Option<u32>, CmdShell) {
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        let exit = shell.execute_line(line);
        let out = match &*shell.output_sink.borrow() {
            OutputSink::Buffer(b) => String::from_utf8_lossy(b).into_owned(),
            _ => unreachable!(),
        };
        (out, exit, shell)
    }

    /// ECHO is on in a script: each line is shown after the prompt,
    /// expanded, except one starting with `@`; ECHO OFF stops it.
    #[test]
    fn script_lines_are_echoed_until_echo_off() {
        let (loader, _d) = drive_with(&[("S.CMD", "echo one %1\n@echo  spaced   text\necho off\necho two\necho\necho.\necho.dot\n")]);
        let (out, _, shell) = run(&loader, "S.CMD arg");
        assert_eq!(out, "[C:\\] echo one arg\r\none arg\r\n spaced   text\r\n[C:\\] echo off\r\ntwo\r\nECHO is off.\r\n\r\ndot\r\n");
        assert!(shell.echo_on.get(), "the prompt's ECHO state is its own");
    }

    /// GOTO goes on after its label (without case); a missing label ends
    /// the script with an error, and the rest of GOTO's line is not run.
    #[test]
    fn goto_jumps_to_labels() {
        let (loader, _d) = drive_with(&[("S.CMD", "@echo off\ngoto Later & echo same-line\necho skipped\n:LATER extra words\necho later\ngoto nowhere\necho after\n")]);
        let (out, exit, shell) = run(&loader, "S.CMD");
        assert_eq!(exit, None);
        assert!(out.starts_with("later\r\n"), "{out}");
        assert!(out.contains("nowhere was not found"), "{out}");
        for wrong in ["same-line", "skipped", "after"] { assert!(!out.contains(wrong), "{wrong}: {out}"); }
        assert_eq!(shell.last_rc.get(), RC_BUILTIN_FAILED);
    }

    /// IF's three conditions, with NOT; a false IF leaves the return code.
    #[test]
    fn if_tests_errorlevel_exist_and_strings() {
        let script = "@echo off\ncd NOSUCH\nif errorlevel 1 echo el1\nif not errorlevel 2 echo not-el2\n\
                      if errorlevel 2 echo wrong-el2\nif exist S.CMD echo exists\nif not exist NO.TXT echo missing\n\
                      if exist *.CMD echo wildcard\nif exist SUB echo a-directory\nif %1==x echo eq\n\
                      if \"%2\"==\"\" echo empty2\nif a==A echo wrong-case\nif not a==b echo not-eq\n";
        let (loader, _d) = drive_with(&[("S.CMD", script), ("SUB\\", "")]);
        let (out, _, _) = run(&loader, "S.CMD x");
        assert_eq!(out, "The system cannot find the path specified.\r\nel1\r\nnot-el2\r\nexists\r\nmissing\r\nwildcard\r\na-directory\r\neq\r\nempty2\r\nnot-eq\r\n");
        let (out, _, shell) = run(&loader, "cd NOSUCH & if a==b echo x");
        assert_eq!(shell.last_rc.get(), RC_BUILTIN_FAILED, "a false IF keeps the code: {out}");
        let (out, _, shell) = run(&loader, "if errorlevel x echo x");
        assert!(out.contains("syntax"), "{out}");
        assert_eq!(shell.last_rc.get(), RC_BUILTIN_FAILED);
    }

    /// FOR runs its command for each item; wildcards stand for files, not
    /// directories, with the directory as written; `%%v` in a script.
    #[test]
    fn for_runs_a_command_per_item() {
        let (loader, _d) = drive_with(&[("S.CMD", "@echo off\nfor %%v in (a b,c) do echo [%%v]\nfor %%f in (SUB\\*.TXT) do echo %%f\n"),
                                        ("SUB\\", ""), ("SUB\\B.TXT", "b"), ("SUB\\A.TXT", "a"), ("SUB\\D.TXT\\", "")]);
        let (out, _, _) = run(&loader, "S.CMD");
        let mut lines: Vec<&str> = out.lines().collect();
        assert_eq!(&lines[..3], ["[a]", "[b]", "[c]"]);
        lines[3..].sort_unstable();
        assert_eq!(&lines[3..], ["SUB\\A.TXT", "SUB\\B.TXT"], "{out}");
        let (out, _, _) = run(&loader, "for %x in (1 2) do echo n%x");
        assert_eq!(out, "n1\r\nn2\r\n", "%x at the prompt");
    }

    /// `%0`–`%9` and SHIFT.
    #[test]
    fn parameters_shift() {
        let (loader, _d) = drive_with(&[("S.CMD", "@echo off\necho %0 %1 %2 [%3]\nshift\necho %0 %1 [%2]\n")]);
        let (out, _, _) = run(&loader, "s one \"two 2\"");
        assert_eq!(out, "s one \"two 2\" []\r\none \"two 2\" []\r\n");
    }

    /// SETLOCAL keeps the environment and directories until ENDLOCAL, or
    /// the script's end.
    #[test]
    fn setlocal_restores_environment_and_directory() {
        let script = "@echo off\nset X=outer\nsetlocal\nset X=inner\ncd SUB\necho [%X%]\nendlocal\necho [%X%]\n\
                      setlocal\nset Y=gone\ncd SUB\n";
        let (loader, _d) = drive_with(&[("S.CMD", script), ("SUB\\", "")]);
        let (out, _, shell) = run(&loader, "S.CMD");
        assert_eq!(out, "[inner]\r\n[outer]\r\n");
        let env = shell.env.borrow();
        assert!(env.iter().any(|(k, v)| k == "X" && v == "outer"));
        assert!(!env.iter().any(|(k, _)| k == "Y"), "the open SETLOCAL ended with the script");
        assert_eq!(shell.current_path(), "C:\\");
    }

    /// CALL comes back; a script started without CALL takes the caller's
    /// place; EXIT in a script ends the shell.
    #[test]
    fn call_returns_and_a_plain_start_does_not() {
        let (loader, _d) = drive_with(&[("A.CMD", "@echo off\ncall B.CMD 1\necho back\nB.CMD 2\necho never\n"),
                                        ("B.CMD", "@echo in-b %1\n"), ("E.CMD", "@echo off\nexit 3\necho never\n")]);
        let (out, exit, _) = run(&loader, "A.CMD");
        assert_eq!((out.as_str(), exit), ("in-b 1\r\nback\r\nin-b 2\r\n", None));
        let (out, exit, _) = run(&loader, "E.CMD & echo not-after-exit");
        assert_eq!((out.as_str(), exit), ("", Some(3)));
    }

    /// A script that calls itself stops at MAX_SCRIPT_DEPTH.
    #[test]
    fn scripts_nest_only_so_deep() {
        let (loader, _d) = drive_with(&[("R.CMD", "@echo off\ncall R.CMD\n")]);
        let (out, exit, _) = run(&loader, "R.CMD");
        assert_eq!(exit, None);
        assert_eq!(out.matches("Too many scripts").count(), 1, "{out}");
    }

    /// PAUSE takes one byte of a pipe or file read as input.
    #[test]
    fn pause_reads_a_key() {
        let (loader, _d) = drive_with(&[]);
        let shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        *shell.input_source.borrow_mut() = InputSource::Data(b"xy".to_vec().into());
        shell.cmd_pause();
        assert!(matches!(&*shell.input_source.borrow(), InputSource::Data(d) if d.iter().eq(b"y")));
    }
}
