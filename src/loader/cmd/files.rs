// SPDX-License-Identifier: GPL-3.0-or-later
//
// The built-in CMD.EXE's file commands (ADR-0079): COPY, REN and MOVE, in
// OS/2's syntax, all through the VFS so that they stay inside the drives.

use super::*;
use crate::loader::Loader;
use crate::loader::vfs::FileAttribute;

/// The most files one COPY, REN or MOVE handles.
const MAX_FILES: usize = 65536;

/// `path` split into its directory part (with the separator, or `X:`) and
/// its last name.
fn split_dir(path: &str) -> (&str, &str) {
    match path.rfind(['\\', '/', ':']) {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    }
}

fn has_wildcard(name: &str) -> bool {
    name.contains(['*', '?'])
}

/// `dir` joined with `name`: `C:` + `X` is `C:X`, `SUB` + `X` is `SUB\X`.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir.ends_with(['\\', '/', ':']) { format!("{dir}{name}") } else { format!("{dir}\\{name}") }
}

impl CmdShell {
    /// Whether `path` names an existing directory.
    fn is_dir(&self, path: &str) -> bool {
        self.shared.drive_mgr.lock_or_recover().query_path_info(path, 1)
            .is_ok_and(|s| s.attributes.contains(FileAttribute::DIRECTORY))
    }

    /// The files a source names: a wildcard's matching files, a
    /// directory's files, or the file itself. `None` (with an error
    /// written) when nothing matches.
    fn source_files(&self, source: &str) -> Option<Vec<String>> {
        let files = if has_wildcard(source) {
            self.matching_files(source)
        } else if self.is_dir(source) {
            self.matching_files(&join(source, "*"))
        } else if self.exists(source) {
            vec![source.to_string()]
        } else {
            Vec::new()
        };
        if files.is_empty() {
            self.error(&format!("The system cannot find the file specified: {source}"));
            return None;
        }
        Some(files.into_iter().take(MAX_FILES).collect())
    }

    /// Where `file` goes for a target `dest`: into it when it is a
    /// directory (or ends with `\`, or is absent: the current directory),
    /// renamed by its wildcards, or `dest` itself.
    fn target_for(&self, file: &str, dest: Option<&str>) -> String {
        let name = split_dir(file).1;
        let Some(dest) = dest else { return name.to_string() };
        if dest.ends_with(['\\', '/', ':']) || self.is_dir(dest) {
            return join(dest, name);
        }
        let (dir, pattern) = split_dir(dest);
        if has_wildcard(pattern) { format!("{dir}{}", Loader::apply_edit_name(name, pattern)) } else { dest.to_string() }
    }

    /// Whether `dest` names one file (not a directory, no wildcards).
    fn names_one_file(&self, dest: Option<&str>) -> bool {
        dest.is_some_and(|d| !d.ends_with(['\\', '/', ':']) && !has_wildcard(split_dir(d).1) && !self.is_dir(d))
    }

    /// Whether two paths are the same entry on the host.
    fn same_entry(&self, a: &str, b: &str) -> bool {
        let dm = self.shared.drive_mgr.lock_or_recover();
        matches!((dm.resolve_to_host_path(a), dm.resolve_to_host_path(b)), (Some(x), Some(y)) if x == y)
    }

    /// `COPY source[+source…] [target] [/A] [/B] [/V]`. Copies each file
    /// (wildcards allowed) to the target, replacing what is there; several
    /// files into one file, or `+`, join them into it. The switches are
    /// accepted and ignored: every copy is binary.
    pub(super) fn cmd_copy(&self, args: &[&str]) {
        let paths: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with('/')).collect();
        let (Some(&source), dest, None) = (paths.first(), paths.get(1).copied(), paths.get(2)) else {
            self.error("The syntax of the command is incorrect.");
            return;
        };
        let pieces: Vec<&str> = source.split('+').filter(|p| !p.is_empty()).collect();
        let mut files = Vec::new();
        for piece in &pieces {
            files.extend(self.source_files(piece).unwrap_or_default());
        }
        if files.is_empty() { self.failed.set(true); return; }
        let join_into = (pieces.len() > 1 || files.len() > 1) && self.names_one_file(dest);
        if pieces.len() > 1 && !join_into {
            self.error("Joining files with + needs a target file.");
            return;
        }
        let copied = if join_into {
            usize::from(self.join_files(&files, dest.unwrap_or_default()))
        } else {
            let show = files.len() > 1 || has_wildcard(source);
            let mut copied = 0;
            for file in &files {
                let target = self.target_for(file, dest);
                if self.same_entry(file, &target) {
                    self.error(&format!("The file cannot be copied onto itself: {file}"));
                    continue;
                }
                if show { self.writeln(file); }
                match self.shared.drive_mgr.lock_or_recover().copy_file(file, &target) {
                    Ok(()) => copied += 1,
                    Err(e) => self.error(&format!("Cannot copy {file} to {target}: {e:?}")),
                }
            }
            copied
        };
        self.writeln(&format!("{copied:>9} file(s) copied."));
        if copied < files.len().min(if join_into { 1 } else { usize::MAX }) { self.failed.set(true); }
    }

    /// Join `files` into the file `dest` (made anew). Whether it worked.
    fn join_files(&self, files: &[String], dest: &str) -> bool {
        if files.iter().any(|f| self.same_entry(f, dest)) {
            self.error("The target is one of the files to join.");
            return false;
        }
        let (sources, target) = {
            let dm = self.shared.drive_mgr.lock_or_recover();
            let sources: Option<Vec<_>> = files.iter().map(|f| dm.resolve_to_host_path(f)).collect();
            (sources, dm.resolve_new_host_path(dest))
        };
        let (Some(sources), Some(target)) = (sources, target) else {
            self.error(&format!("Cannot create {dest}."));
            return false;
        };
        let result = std::fs::File::create(&target).and_then(|mut out| {
            for source in &sources {
                std::io::copy(&mut std::fs::File::open(source)?, &mut out)?;
            }
            Ok(())
        });
        for file in files { self.writeln(file); }
        if let Err(e) = result { self.error(&format!("Cannot create {dest}: {e}")); }
        true
    }

    /// `REN old new` (also `RENAME`): rename a file or directory in its
    /// own directory; `new` is a name, wildcards taking the old name's
    /// parts (`REN *.TXT *.BAK`). An existing name is not replaced.
    pub(super) fn cmd_ren(&self, args: &[&str]) {
        let (Some(&old), Some(&new)) = (args.first(), args.get(1)) else {
            self.error("The syntax of the command is incorrect.");
            return;
        };
        if new.contains(['\\', '/', ':']) {
            self.error("The new name cannot have a drive or a directory.");
            return;
        }
        let files = if has_wildcard(old) { self.matching_files(old) }
                    else if self.exists(old) { vec![old.to_string()] } else { Vec::new() };
        if files.is_empty() {
            self.error(&format!("The system cannot find the file specified: {old}"));
            return;
        }
        for file in files.iter().take(MAX_FILES) {
            let (dir, name) = split_dir(file);
            let target = format!("{dir}{}", if has_wildcard(new) { Loader::apply_edit_name(name, new) } else { new.to_string() });
            // Only a change of case may name an existing entry: itself.
            if self.exists(&target) && !self.same_entry(file, &target) {
                self.error(&format!("A duplicate file name exists: {target}"));
                continue;
            }
            if let Err(e) = self.shared.drive_mgr.lock_or_recover().rename_file(file, &target) {
                self.error(&format!("Cannot rename {file}: {e:?}"));
            }
        }
    }

    /// `MOVE source [target]`: move files (wildcards allowed), or one
    /// directory, on one drive. Into a directory (the current one without
    /// a target), or to a new name for one file. An existing name is not
    /// replaced.
    pub(super) fn cmd_move(&self, args: &[&str]) {
        let Some(&source) = args.first() else {
            self.error("The syntax of the command is incorrect.");
            return;
        };
        let dest = args.get(1).copied();
        let files = if has_wildcard(source) { self.matching_files(source) }
                    else if self.exists(source) { vec![source.to_string()] } else { Vec::new() };
        if files.is_empty() {
            self.error(&format!("The system cannot find the file specified: {source}"));
            return;
        }
        if files.len() > 1 && self.names_one_file(dest) {
            self.error("Several files cannot move to one file name.");
            return;
        }
        let mut moved = 0;
        for file in files.iter().take(MAX_FILES) {
            let target = self.target_for(file, dest);
            if self.exists(&target) {
                self.error(&format!("A file or directory named {target} exists already."));
                continue;
            }
            match self.shared.drive_mgr.lock_or_recover().rename_file(file, &target) {
                Ok(()) => moved += 1,
                Err(crate::loader::vfs::Os2Error::NOT_SAME_DEVICE) => {
                    self.error("MOVE cannot move to another drive; use COPY and DEL.");
                    break;
                }
                Err(e) => self.error(&format!("Cannot move {file}: {e:?}")),
            }
        }
        self.writeln(&format!("{moved:>9} file(s) moved."));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::vfs_hostdir::HostDirBackend;

    /// A mock loader with drive C (and D) in temporary directories holding
    /// `files` (`/` for directories in names; a name ending in `/` is a
    /// directory).
    fn drives_with(files: &[(&str, &str)]) -> (Loader, tempfile::TempDir, tempfile::TempDir) {
        let loader = Loader::new_mock();
        let (c, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (name, text) in files {
            match name.strip_suffix('/') {
                Some(dir) => std::fs::create_dir_all(c.path().join(dir)).unwrap(),
                None => std::fs::write(c.path().join(name), text).unwrap(),
            }
        }
        {
            let mut dm = loader.shared.drive_mgr.lock_or_recover();
            dm.mount(2, Box::new(HostDirBackend::new(c.path().to_path_buf()).unwrap()));
            dm.mount(3, Box::new(HostDirBackend::new(d.path().to_path_buf()).unwrap()));
            dm.set_current_disk(3).unwrap();
        }
        (loader, c, d)
    }

    /// Run `line`; returns the output and the return code.
    fn run(loader: &Loader, line: &str) -> (String, u32) {
        let mut shell = CmdShell::new(Arc::clone(&loader.shared), Vec::new());
        *shell.output_sink.borrow_mut() = OutputSink::Buffer(Vec::new());
        shell.execute_line(line);
        let out = match &*shell.output_sink.borrow() {
            OutputSink::Buffer(b) => String::from_utf8_lossy(b).into_owned(),
            _ => unreachable!(),
        };
        (out, shell.last_rc.get())
    }

    fn read(dir: &tempfile::TempDir, name: &str) -> Option<String> {
        std::fs::read_to_string(dir.path().join(name)).ok()
    }

    #[test]
    fn split_and_join_paths() {
        assert_eq!(split_dir(r"C:\SUB\A.TXT"), (r"C:\SUB\", "A.TXT"));
        assert_eq!(split_dir("C:A.TXT"), ("C:", "A.TXT"));
        assert_eq!(split_dir("A.TXT"), ("", "A.TXT"));
        assert_eq!(join("SUB", "A"), r"SUB\A");
        assert_eq!(join(r"C:\", "A"), r"C:\A");
        assert_eq!(join("D:", "A"), "D:A");
    }

    /// COPY: one file, into a directory, by wildcard renaming, across
    /// drives; never onto itself.
    #[test]
    fn copy_copies_files() {
        let (loader, c, d) = drives_with(&[("A.TXT", "a"), ("B.TXT", "b"), ("SUB/", "")]);
        assert_eq!(run(&loader, "copy A.TXT NEW.TXT").1, 0);
        assert_eq!(read(&c, "NEW.TXT").as_deref(), Some("a"));
        let (out, rc) = run(&loader, "copy *.TXT SUB");
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("3 file(s) copied."), "{out}");
        assert_eq!(read(&c, "SUB/B.TXT").as_deref(), Some("b"));
        assert_eq!(run(&loader, "copy *.TXT *.BAK").1, 0);
        assert_eq!(read(&c, "A.BAK").as_deref(), Some("a"), "wildcard target");
        assert_eq!(read(&c, "NEW.BAK").as_deref(), Some("a"));
        assert_eq!(run(&loader, "copy A.TXT B.TXT C.TXT").1, 1, "one source, one target");
        assert_eq!(run(&loader, "copy C:\\A.TXT D:\\").1, 0);
        assert_eq!(read(&d, "A.TXT").as_deref(), Some("a"), "to drive D");
        let (out, rc) = run(&loader, "copy A.TXT a.txt");
        assert!(out.contains("onto itself") && rc == 1, "{out}");
        assert_eq!(read(&c, "A.TXT").as_deref(), Some("a"), "not emptied");
        let (out, rc) = run(&loader, "copy NONE.TXT X.TXT");
        assert!(out.contains("cannot find") && rc == 1, "{out}");
    }

    /// COPY joins files given with `+`, or several files into one file.
    #[test]
    fn copy_joins_files() {
        let (loader, c, _d) = drives_with(&[("A.TXT", "a"), ("B.TXT", "b")]);
        assert_eq!(run(&loader, "copy A.TXT+B.TXT AB.OUT").1, 0);
        assert_eq!(read(&c, "AB.OUT").as_deref(), Some("ab"));
        assert_eq!(run(&loader, "copy A.TXT /V BOTH.OUT /B").1, 0, "switches ignored");
        assert_eq!(read(&c, "BOTH.OUT").as_deref(), Some("a"));
        let (out, rc) = run(&loader, "copy A.TXT+B.TXT A.TXT");
        assert!(out.contains("one of the files") && rc == 1, "{out}");
        assert_eq!(read(&c, "A.TXT").as_deref(), Some("a"));
    }

    /// REN renames in place, by wildcards too; it never replaces a file,
    /// and a change of case is allowed.
    #[test]
    fn ren_renames_in_place() {
        let (loader, c, _d) = drives_with(&[("A.TXT", "a"), ("B.TXT", "b"), ("SUB/", ""), ("SUB/X.LOG", "x")]);
        assert_eq!(run(&loader, "ren A.TXT C.TXT").1, 0);
        assert_eq!(read(&c, "C.TXT").as_deref(), Some("a"));
        let (out, rc) = run(&loader, "ren B.TXT C.TXT");
        assert!(out.contains("duplicate") && rc == 1, "{out}");
        assert_eq!(read(&c, "B.TXT").as_deref(), Some("b"), "not replaced");
        assert_eq!(run(&loader, "ren SUB\\*.LOG *.OLD").1, 0);
        assert_eq!(read(&c, "SUB/X.OLD").as_deref(), Some("x"), "in its own directory");
        assert_eq!(run(&loader, "ren b.txt b.TXT").1, 0, "a change of case");
        assert_eq!(run(&loader, "ren C.TXT SUB\\D.TXT").1, 1, "a name, not a path");
    }

    /// MOVE moves files and directories on one drive, never over an
    /// existing name, never to another drive.
    #[test]
    fn move_moves_on_one_drive() {
        let (loader, c, _d) = drives_with(&[("A.TXT", "a"), ("B.TXT", "b"), ("DIR/", ""), ("DIR/IN.TXT", "i"), ("SUB/", "")]);
        let (out, rc) = run(&loader, "move *.TXT SUB");
        assert_eq!(rc, 0, "{out}");
        assert!(out.contains("2 file(s) moved."), "{out}");
        assert_eq!(read(&c, "SUB/A.TXT").as_deref(), Some("a"));
        assert!(read(&c, "A.TXT").is_none());
        assert_eq!(run(&loader, "move DIR SUB").1, 0, "a directory");
        assert_eq!(read(&c, "SUB/DIR/IN.TXT").as_deref(), Some("i"));
        std::fs::write(c.path().join("A.TXT"), "new").unwrap();
        let (out, rc) = run(&loader, "move A.TXT SUB");
        assert!(out.contains("exists already") && rc == 1, "{out}");
        assert_eq!(read(&c, "SUB/A.TXT").as_deref(), Some("a"), "not replaced");
        assert_eq!(run(&loader, "move A.TXT RENAMED.TXT").1, 0, "a new name");
        let (out, rc) = run(&loader, "move RENAMED.TXT D:\\");
        assert!(out.contains("another drive") && rc == 1, "{out}");
        let (out, rc) = run(&loader, "move SUB\\*.TXT ONE.TXT");
        assert!(out.contains("one file name") && rc == 1, "{out}");
    }
}
