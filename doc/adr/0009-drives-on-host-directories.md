# ADR-0009: Map OS/2 drives to host directories, with C: in the user's data directory

- **Status:** Accepted
- **Date:** 2026-03-14
- **Sources:** commit df92bba ("define VFS trait architecture …"); `src/loader/vfs.rs`, `vfs_hostdir.rs`; developer guide (VFS); commit 802e7af (tests off drive C)

## Context

OS/2 programs expect drive letters, case-insensitive HPFS names, extended attributes and OS/2 sharing and locking semantics. Warpine runs on a Linux file system.

## Decision

A `VfsBackend` trait carries the OS/2 file-system semantics; `DriveManager` maps drive letters to backends. `HostDirBackend` serves a host directory as an HPFS-like drive: case-insensitive, case-preserving lookup, long names, sharing modes, sandboxing to its root, extended attributes in xattrs or a sidecar, `fcntl` locks.

- Drive C: is `$XDG_DATA_HOME/warpine/drive_c` (by default `~/.local/share/warpine/drive_c`), created on first use.
- Tests never use the user's drive C: integration tests run Warpine with a temporary `XDG_DATA_HOME`, and unit tests mount `target/test-drive-c`.

## Consequences

- OS/2 paths never reach the host unchecked; `..` cannot leave a drive.
- The user's OS/2 files persist between runs in one place.
- Other backends (archives, network) can be added behind the same trait.
