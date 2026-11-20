# ADR-0079: The built-in CMD.EXE's file commands, and the VFS fixes they needed

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - OS/2's COPY, REN / RENAME and MOVE as IBM's public *OS/2 Command Reference* describes them, followed for what is implemented;
  - `DosEditName` and `DosCopy` in IBM's *Control Program Programming Reference*;
  - the code: `src/loader/cmd/files.rs`, `src/loader/vfs.rs` (`copy_file`), `src/loader/vfs_hostdir.rs` (`resolve_for_create`, `DirCache::lookup`), `src/loader/stubs.rs` (`dos_edit_name`, `apply_edit_name`);
  - the sample: `samples/cmd_scripts/files.cmd`.

## Context

Stage 2's third slice gives the shell its file commands. They must work through the VFS, as every other shell command does, so they stay inside the drives (ADR-0066, ADR-0075). Building them showed five faults in the layers under them:
- **`DosEditName` could overflow.** It computed `cb_target - 1` with no check: a program passing a length of 0 made it panic in a debug build. In a release build the subtraction wrapped, and it wrote past the buffer the program had given. It also edited names byte by byte, garbling those beyond ASCII.
- **A file copied onto itself was emptied.** `DriveManager::copy_file` went to the host's `fs::copy`, which truncates the target first. So copying a file onto itself, in any case of its name, emptied it; that included `DosCopy`.
- **No copies across drives.** `copy_file` refused another drive, which OS/2's `DosCopy` allows.
- **Names in another case made a second file.** `resolve_for_create` did not match a new name against existing entries without regard to case. Creating, copying or renaming onto `OLD.TXT` beside `Old.txt` therefore made a second file, which OS/2's case-insensitive file systems never have. The same gap let a create through a dangling symbolic link of another case past the dangling-link check (ADR-0075).
- **New files were invisible for a moment.** The host backend's directory cache answered "not there" for up to two seconds after a file was made outside the backend (a shell redirection, a copy across drives, another process).

## Decision

1. **COPY source[+source…] [target] [/A] [/B] [/V]** (`files.rs`):
   - **Sources.** Each file of the source is copied: a wildcard's files, a directory's files, or the file itself.
   - **The target** is a directory (or absent: the current directory), a name with wildcards (`COPY *.TXT *.BAK`, through `apply_edit_name`, the transformation `DosEditName` uses), or a file. A copy replaces the target.
   - **Joining.** `+`, or several files copied into one file name, join them into it.
   - **Refusals.** A file is never copied onto itself, nor joined into a file that is one of its sources.
   - **The switches** are accepted and ignored: every copy is binary.
   - **Output.** COPY writes the names when there are several files, then `n file(s) copied.`
2. **REN old new** (also RENAME) renames a file, or a directory, in its own directory.
   - **The new name** is a name, not a path; with wildcards it takes the old name's parts (`REN *.LOG *.OLD`).
   - **No replacing.** An existing name is not replaced, but a change of case is allowed.
3. **MOVE source [target]** moves files (wildcards allowed), or one directory, on one drive.
   - **The target** is a directory (the current one without a target), or a new name for a single file.
   - **No replacing.** An existing name is not replaced. Another drive is refused, with a hint to use COPY and DEL.
4. **Return codes.** Each command returns 1 when any of its files failed (ADR-0077).
5. **`DosEditName`:**
   - The result and its NUL must fit the given length, else `ERROR_BUFFER_OVERFLOW`, and nothing is written.
   - A NULL target is `ERROR_INVALID_PARAMETER`.
   - The edit works on characters, and the result is encoded in the process codepage.
6. **`DriveManager::copy_file`:**
   - A copy whose source and target are the same host file is `ERROR_ACCESS_DENIED`.
   - Across drives, each end is resolved by its own backend: the source as an existing file, the target as a new one. Neither can leave its drive or go through a dangling link.
7. **`resolve_for_create`** uses an existing entry's name when one matches without regard to case, and only then checks the sandbox, dangling links included.
8. **`DirCache::lookup`** reads a directory again when a fresh cached listing does not know the name.

## Warpine's choices (unverified)

- COPY's message `n file(s) copied.`, right-aligned to 9 characters, and its listing of names.
- The `/A` / `/B` / `/V` switches are ignored: no Ctrl+Z handling, no verification.
- `COPY a+b` without a target is an error; DOS appended to the first file.
- MOVE never replaces an existing name, and its message `n file(s) moved.`.
- REN of a directory.
- Wildcard sources stand for files only, not directories.
- `DosCopy` onto itself is `ERROR_ACCESS_DENIED`.
- `DosEditName`'s overflow is `ERROR_BUFFER_OVERFLOW`, and nothing is written.
- Case-insensitive matching is ASCII only, as before.

## Consequences

- **What scripts can do.** `.CMD` scripts can copy, rename and move files: `samples/cmd_scripts/files.cmd` copies an OS/2 program into a directory and runs it from there, joins files and counts the result with that program.
- **For OS/2 programs.** They get a safe `DosEditName`, a `DosCopy` that never empties a file and works across drives, and file names that stay unique without regard to case.
- **Tests:**
  - `files.rs`: `split_and_join_paths`, `copy_copies_files`, `copy_joins_files`, `ren_renames_in_place`, `move_moves_on_one_drive`;
  - `vfs.rs`: `files_copy_across_drives_but_not_onto_themselves`;
  - `vfs_hostdir.rs`: `a_new_name_in_another_case_is_the_existing_entry`, `a_file_made_after_caching_is_found`;
  - `stubs.rs`: `test_edit_name_non_ascii`, `test_dos_edit_name_checks_the_target_size`;
  - `tests/integration.rs`: `test_cmd_script_file_commands`.
