# ADR-0087: Tier 1 follow-ups: PM names from the dispatchers, raw pipe records, exit lists on thread 1

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - IBM's *Control Program Programming Reference* (`DosQueryProcAddr`, `DosExitList`) and *Control Program Guide* ("Creating an Exit List");
  - the OS/2 Warp 4.5 Toolkit's `bsedos.h` at the commit ADR-0072 pins (the `DosRawReadNPipe` / `DosRawWriteNPipe` prototypes; no reference documents them);
  - `doc/os2_ordinals.md` (DOSCALLS 246 and 247);
  - the code: `build.rs`, `src/loader/descriptors.rs`, `stubs.rs`, `npipe.rs`, `npipe_api.rs`, `exitlist.rs`, `vcpu.rs`;
  - the samples: `samples/dos_misc_test`, `samples/pm_winutil_test`, `samples/npipe_test`, `samples/exitlist_test`.

## Context

ADR-0086 left three small gaps:

- `DosQueryProcAddr` by name, and so `WinLoadProcedure`, found no PMWIN or PMGPI call. Those modules are sub-dispatched: their calls are match arms, not registry entries, so the name lookup had no table. By ordinal they resolved any number, while the registry modules answered only for calls Warpine implements (ADR-0086).
- `DosRawReadNPipe` / `DosRawWriteNPipe` were missing. Their prototypes are public; their behaviour is not.
- Exit lists ran on the thread that ended the process. The Control Program Guide has thread 1 run them. A guest `HLT` ended the process without them, and so did a failed `KVM_RUN` on thread 1.

## Decision

1. **PM names come from the dispatchers.**
   - `build.rs` reads the PMWIN and PMGPI dispatcher sources: each `N => {` arm followed by a `// Name(` comment.
   - It emits `$OUT_DIR/pm_dispatch_names.rs`, two `(name, ordinal)` tables.
   - These are the arms the dispatch-ordinal and count tests already check, so a table cannot list a call that is not dispatched. A unit test checks that the tables hold the same ordinals as the counts.
   - `DosQueryProcAddr` by name looks there, in any case.
   - By ordinal, PMWIN and PMGPI now find only the ordinals in the tables, like the registry modules.
   - VIOCALLS, KBDCALLS and MOUCALLS keep their hand-written name tables and resolve any ordinal, as before.
2. **Raw pipe records.**
   - On a message pipe a raw record is a little-endian USHORT length followed by that many bytes. It is the 2-byte header `DosPeekNPipe` already counts per message.
   - `DosRawReadNPipe` takes as many whole records as fit.
     - A record that does not fit is cut: a shorter record, `ERROR_MORE_DATA`, and its rest as the next record.
     - A buffer too small for a header takes nothing (`ERROR_MORE_DATA`).
     - A message longer than 65,535 bytes reads as several records.
   - `DosRawWriteNPipe` sends each record as one message.
     - A record running past `cbWrite` is `ERROR_INVALID_PARAMETER`, and nothing is written.
     - A full non-blocking pipe stops after the records written so far.
     - `*pcbActual` counts the bytes taken, headers included.
   - On a byte pipe the two calls are `DosRead` / `DosWrite`.
3. **Exit lists run on thread 1.**
   - `DosExit(EXIT_PROCESS)` on another thread, with routines on the list and thread 1 running, asks thread 1 to end the process. This is the same request a fault on another thread already made (`wait_for_thread_1_to_end`).
   - The calling thread waits until thread 1 has taken over, or at most `KILL_GRACE` (5 s), then the process ends without the list.
   - A guest `HLT` is handled as a trap (`TC_TRAP`, exit code 1).
   - A failed `KVM_RUN` on thread 1 now tries the exit list on that vCPU with its starting segment state (`TC_HARDERROR`); if the vCPU fails again in a routine, the process ends.

## Warpine's choices (unverified)

- PMWIN / PMGPI names matching in any case.
- The raw record format; `ERROR_MORE_DATA` for a cut record or a buffer too small for a header; long messages split into records; `ERROR_INVALID_PARAMETER` for a record past the buffer; what `*pcbActual` counts.
- The 5-second wait for thread 1; a thread 1 in 16-bit code ending the process without the list; `HLT` as `TC_TRAP`; running the list after a failed `KVM_RUN`.

## Consequences

- **Programs** can look up PM calls at run time (`DosQueryProcAddr`, `WinLoadProcedure`), and probing for a PM call Warpine lacks fails cleanly instead of reaching an unknown-ordinal arm.
- **Coverage:** two more DOSCALLS entry points, 877 in all.
- **Exit-list routines** that rely on thread 1's state (its PM message queue, thread-local data) now see it.
- **Build:** `build.rs` reruns when a PM dispatcher source changes.
- **Tests:**
  - unit tests in `descriptors.rs` (name lookup, dispatched ordinals), `api_registry.rs` (the tables match the counts), `npipe.rs` (`raw_records`) and `npipe_api.rs` (`raw_reads_and_writes`);
  - `tests/integration.rs`:
    - `test_dos_misc_calls`: 39 checks;
    - `gui_pm_winutil_test_checks_the_tier_one_pm_calls`: 96 checks;
    - `test_named_pipes_and_child_pipes`: now with the raw calls;
    - the exitlist variants check that the routines run on thread 1, and the new `test_exit_list_runs_after_hlt` covers `HLT`.
