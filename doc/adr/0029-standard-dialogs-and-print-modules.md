# ADR-0029: Build the standard file and font dialogs in memory, and give PMCTLS and PMSPL module slots

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** batch 5; `src/loader/pm_stddlg.rs`, `src/loader/pm_filedlg.rs`, `src/loader/pm_fontdlg.rs`, `src/loader/pmspl.rs`, `src/loader/constants.rs` (`PMSPL_BASE`, `PMCTLS_BASE`, `STUB_AREA_SIZE`); `targets/os2api.def`; the Open Watcom import library `os2386.lib` (PMCTLS ordinals); IBM *Presentation Manager Programming Reference* (`WinFileDlg`, `WinFontDlg`, `<pmstddlg.h>`, the spooler); ADR-0005, ADR-0006, ADR-0009

## Context

`WinFileDlg` and `WinFontDlg` live in PMCTLS.DLL and the spooler calls in PMSPL.DLL, two DLLs Warpine did not provide. Programs import them by ordinal, so each built-in module needs an ordinal slot in the API stub area (ADR-0005). `doc/os2_ordinals.md` has a PMSPL section but no PMCTLS one.

On OS/2 the standard dialogs are dialog templates with a default dialog procedure; programs can subclass them (`pfnDlgProc` calling `WinDefFileDlgProc`) or supply their own templates holding the standard control ids. The file dialog must browse OS/2 drives, which in Warpine are host directories (ADR-0009).

## Decision

- **Two module slots:** `PMSPL_BASE` = 18432 and `PMCTLS_BASE` = 19456; `STUB_AREA_SIZE` grows to 20480, with compile-time checks. Both modules are wired like PMSHAPI and HELPMGR: built-in module list, `resolve_import`, `DosLoadModule`, registry, `api_trace`, `lx_link`, `gen_api`.
- **PMCTLS ordinals** come from the import records of `os2386.lib`, as ADR-0006 prescribes. The def file marks them with `# NOTE:` lines; `doc/os2_ordinals.md` stays a snapshot and is not edited.
- **The dialogs are built in memory, as `WinMessageBox` is:** an ordinary `#Dialog` of built-in controls (entry field, list boxes, combination boxes, push buttons) carrying the `<pmstddlg.h>` `DID_*` ids.
  - All dialog logic finds controls by id, so custom templates and subclassing dialog procedures work wherever they keep the standard controls.
  - State lives in `WindowManager::std_dialogs`.
  - `FILEDLG` / `FONTDLG` are written back once, when the dialog is dismissed, also when the program's own procedure calls `WinDismissDlg`.
- **The file dialog reads directories only through the drive backends** (`DriveManager`), with bounded listings and pure, bounded path resolution: `..` stops at the drive root, and no host path outside a mapped drive can be reached.
- **Printing is not supported:** the spooler reports no queues, printers or devices; `SplQm*`, `DevOpenDC(OD_QUEUED / OD_DIRECT)`, `DevEscape` and `DevPostDeviceModes` fail with documented errors.

## Consequences

- Programs that open files or choose fonts through the standard dialogs work, and the dialogs look and behave like other Warpine dialogs.
- Messages the dialogs should send synchronously to a subclassing procedure (`FDM_FILTER`, `FDM_VALIDATE`, `FDM_ERROR`, `FNTM_*`) are not sent yet. They need a call chain (ADR-0026).
- Printing remains a future project: a spooler that writes PostScript or PDF would replace the stubs behind the same module slot.
- Further built-in DLLs keep taking 1024-ordinal slots after 19456.
