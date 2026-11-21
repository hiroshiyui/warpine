# ADR-0105: Warpine's own message files: DosGetMessage, os2msg, and messages bound by lx-link

- **Status:** Accepted; amends ADR-0101's `DosGetMessage` choice
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Control Program Programming Reference*: `DosGetMessage`, `DosInsertMessage` and `DosQueryMessageCP`, their arguments and return codes (`ERROR_MR_MSG_TOO_LONG` 316, `ERROR_MR_MID_NOT_FOUND` 317, `ERROR_MR_UN_ACC_MSGF` 318, `ERROR_MR_INV_MSGF_FORMAT` 319, `ERROR_MR_INV_IVCOUNT` 320).
  - The oracle: Open Watcom's `os2386.lib` stub of `DosGetMessage` (object `msgseg32`), which passes a message segment before the documented arguments (ADR-0101).

## Context

`DosGetMessage` returned a placeholder text: Warpine read no message files. ADR-0101 left out programs that bind messages into their executable.

IBM's `MKMSGF` file format has no complete public description. There are only partial community notes (osFree's forum, the `file` magic rules, Archive Team), one of which doubts its own layout. There are no IBM-made files to test against, and reading them is not needed for an OS/2-alike platform.

The user chose Warpine's own format and tools.

## Decision

1. **A message file format of Warpine's own** (`src/loader/msgfile.rs`, the same module in Warpine, `os2msg` and `lx-link`):
   - a 32-byte header: magic `WARPMSG\x1A`, version 1, the texts' codepage, a 3-character component, the first number, the count, the index offset, the file size;
   - an index of (offset, length) per number, length 0 for none;
   - records: a type byte (`E` error, `W` warning, `I` information, `P` prompt, `H` help, `?` unused) and the text.
2. **`os2msg`** (`src/bin/os2msg.rs`, `$(OS2_MSG)` in `os2.mk`) compiles a text source:
   - a component line (`ABC`);
   - messages `ABC0001E: text`, continued on the lines up to the next id;
   - `;` comments;
   - each line ends with CR LF, except a prompt's last line;
   - numbers must rise, and a gap is a number with no message;
   - `--codepage N` records the codepage the bytes are in.
3. **Binding:** a `.msg` input to `lx-link` goes into a bound message segment in the data object, at `__warpine_message_segment`.
   - The segment has magic `WARPMSGS`, a count, and entries of (name offset, file offset, size). Names are the files' names in upper case, without directories.
   - The SDK's `DosGetMessage` stub (`libc/msgstub.c`) passes that symbol, which is a weak reference and so 0 without bound messages, as MSG.6's segment argument.
4. **`DosGetMessage`** (`msgcalls.rs`):
   - **Where it looks:** the segment's file of that name; else the file itself when the name has a drive or directory; else the current directory, then each `DPATH` directory of the process environment, through the drives.
   - **Errors and warnings** get their id first (`ABC0001: `).
   - **Insertions:** `%1` … `%9` take the table's strings; more than 9 is `ERROR_MR_INV_IVCOUNT`.
   - **A message too long** for the buffer is cut, with `ERROR_MR_MSG_TOO_LONG`.
   - **No file, a file in another format, or no message** of that number (or an unused one): the buffer gets a text saying so (`SYS0318`, `SYS0319`, `SYS0317`), and the call returns the matching error.
   - **Bounds:** every read from the segment is checked against guest memory; files and segments are bounded to 16 MB and 256 files; a segment that is not Warpine's (another compiler's) is ignored.

## Warpine's choices (unverified)

- **The format, the source syntax and the segment** are Warpine's own; IBM `MKMSGF` files are not read and give `ERROR_MR_INV_MSGF_FORMAT`.
- **The search order:** the bound segment, then the current directory, then `DPATH`.
- **Which types show their id:** `E` and `W` only, as `ABC0001: `.
- **The fall-back texts** and their wording, and that the buffer gets them along with the error.
- **Line ends:** CR LF after every line but a prompt's last.
- **Texts are not converted** between codepages: their bytes are given as they are.
- **Programs built by Open Watcom** pass their own library's segment, which Warpine ignores; they read message files from the drives.

## Consequences

- Programs from GCC and clang can carry their messages inside the executable, or ship `.msg` files beside it or along `DPATH`.
- **`samples/c_msg_test`**, built by GCC and by clang, binds `APP.MSG` and reads `DSK.MSG` along `DPATH`. It runs 12 checks: ids, insertions, two-line and prompt messages, missing numbers and files, a foreign file, a short buffer, too many strings, a file by its path.
- `ERROR_MR_MID_NOT_FOUND` and `ERROR_MR_INV_MSGF_FORMAT` join the SDK's `errors.h`.
- **Tests:**
  - `msgfile.rs`: compiling, round trips, refused files, segments;
  - `msgcalls.rs`: `DosGetMessage` from a segment, its arguments, foreign and damaged segments, the fall-back texts;
  - `os2msg.rs`: arguments and a compile;
  - `lx_link.rs`: the bound segment object;
  - `tests/integration.rs`: `dos_get_message_reads_bound_and_drive_message_files`.
