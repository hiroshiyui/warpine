# ADR-0091: Hidden, system and archive kept beside host files

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - IBM's *Control Program Programming Reference*: `FILESTATUS3.attrFile`, `DosFindFirst`'s attribute filter, `DosSetPathInfo` / `DosSetFileInfo`, `DosCopy`, `DosMove`; `bsedos.h` `FILE_*`.
  - The Linux man pages `xattr(7)` and `setxattr(2)`.
  - The code: `src/loader/file_attrs.rs` (new), `vfs_hostdir.rs`, `vfs.rs` (`copy_file`).
  - The sample: `samples/attr_test`.
  - ADR-0041 (read-only as the host's write bits).

## Context

OS/2 files have five attributes. Read-only maps to the host's write permission bits (ADR-0041), and directory is the file's kind. Hidden, system and archive have no Linux counterpart:
- Warpine reported every file as archived and none as hidden or system;
- setting them changed nothing;
- `DosFindFirst`'s filter, which shows hidden and system files only when asked, never had any to hide.

The VFS already keeps OS/2 EAs, in extended attributes `user.os2.ea.<name>` where the filesystem has them, else in a sidecar `.os2ea/<name>.ea`. That sidecar had gaps:
- a rename lost it;
- a delete left it behind;
- its folder showed in listings and was reachable;
- a folder holding only it could not be deleted;
- `DosCopy` copied no EAs at all.

## Decision

1. **One byte per file** (`file_attrs.rs`):
   - hidden, system and archive, in the extended attribute `user.warpine.attr`, outside the OS/2 EA names so `DosEnumAttribute` does not list it;
   - or, without extended attributes, in a sidecar `.os2ea/<name>.attr` beside the EA sidecar;
   - nothing stored means the default (archived for a file, nothing for a directory), so existing files are unchanged; storing the default removes the byte.
2. **Reporting.** `DosQueryPathInfo`, `DosQueryFileInfo` and `DosFindFirst` report the stored byte or the default (`HostDirBackend::status_at`).
3. **Setting.** `DosSetPathInfo`, `DosSetFileInfo` and `DosSetFileMode` store the byte, with read-only applied as before. An extended attribute needs write permission, so a read-only file is made writable for the moment and its final permissions come last (`apply_attributes`).
4. **Archive.** Opening a file for writing, or replacing it, archives it again when its archive attribute was cleared.
5. **Sidecars follow:**
   - a rename moves the `.ea` and `.attr` sidecars;
   - a delete removes them;
   - a folder holding only `.os2ea` is empty to `DosDeleteDir`;
   - `.os2ea` is never listed, and any path through it is `ERROR_ACCESS_DENIED` (`enforce_sandbox`).
6. **Copies.** `DosCopy`, on one drive or across drives, gives the copy the source's EAs and attributes, in this order: the attributes without read-only, then the EAs, then all attributes.

## Warpine's choices (unverified)

- Archived at the opening for writing, not at the first write or the close.
- Directories default to no attributes.
- EAs or attributes that cannot be copied are left out without an error; OS/2 may fail the copy.
- The sidecar folder is refused rather than hidden only.

## Consequences

- **Programs** see and keep hidden, system and archive. Backup-style programs can clear archive and see it come back after a write.
- **EAs** now survive renames and copies on filesystems without extended attributes, and EAs copy everywhere.
- **Cost:** reading a file's status costs one more `getxattr` (or a small file read without extended attributes).
- **Tests:**
  - unit tests in `file_attrs.rs` (defaults, both stores, malformed bytes);
  - `vfs_hostdir.rs`, with extended attributes and with sidecars: attributes kept, the filter, archive on write, read-only together with others, renames, deletes, the hidden and unreachable sidecar folder;
  - `vfs.rs`: a copy carries EAs and attributes on one drive and across drives;
  - `tests/integration.rs` `test_file_attributes_are_kept`: `attr_test`, 15 checks.
