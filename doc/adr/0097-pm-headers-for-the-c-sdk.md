# ADR-0097: PM headers for the C SDK, and PMWIN / PMGPI in the ordinal table

- **Status:** Accepted; amends ADR-0096 (what `check-headers` compares) and ADR-0095 (`lx-link`'s modules)
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Presentation Manager Programming Reference*: the prototypes, structures, messages and flags.
  - `doc/os2_ordinals.md` (the Open Watcom `os2386.lib` snapshot): the PMWIN and PMGPI ordinals and names.
  - Warpine's code: the PM sub-dispatchers' arms (`N => {` followed by `// Name(`), the constants in `src/`, and the structure offsets Warpine reads and writes (`FRAMECDATA_*`, `SWP_*`, `QMSG_*`, `FATTRS_*`, `FM_OFF_*`).

## Context

ADR-0096 gave GCC and clang an `os2.h` for the Control Program and the text subsystems. The Presentation Manager was left out because `targets/os2api.def` had no PMWIN or PMGPI entries. `lx-link` resolves calls through that table, so it could not link one PM call. Looking closer turned up three more problems:
- **`lx-link`'s module table** lacked PMDRAG, SOM, REXX, REXXAPI and PMWP. A C call into one of them warned and was linked to the wrong address, since a `call` reaches an import directly.
- **`check-headers` read only `APIENTRY` declarations,** so the `EXPENTRY` window procedures would have gone unchecked.
- **8 PMWIN calls were invisible to the tools that read the dispatchers,** among them `WinInitialize` and `WinCreateStdWindow`: their arms were marked `// WinInitialize` without the `(` that `gen_compat` and `build.rs` look for. The compatibility chart counted 209 of the 217 PMWIN calls Warpine serves, and `DosQueryProcAddr` could not find the 8 by name.
- **Header constants:** PM headers need hundreds of message, style and flag values. Warpine already defines most of them, checked against the toolkit headers by `constants.rs`'s own test, but nothing tied a header's `#define` to them.

## Decision

1. **The ordinal table gains PMWIN and PMGPI** (880 entries, from 518). Each entry is a call a PM sub-dispatcher serves: 217 PMWIN and 145 PMGPI ordinals, exactly the 362 PM ordinals of the dispatchers. The names come from `doc/os2_ordinals.md`, so `validate-doc` confirms all of them. The trace drift check still leaves PMWIN and PMGPI out: their names come from the sub-dispatchers, not `api_trace.rs`.
2. **`sdk/include/os2/pm.h`** declares 236 calls:
   - 168 PMWIN calls: queues and messages, windows and their words, focus, timers, painting, rectangles, dialogs, resources, menus, accelerators, system values, pointers, the clipboard, atoms, codepages;
   - 68 PMGPI calls: presentation spaces, attributes, lines, boxes, arcs, areas, paths, pels, text, fonts, bitmaps, regions, `DevOpenDC`;
   - the types, `QMSG`, `SWP`, `FRAMECDATA`, `CLASSINFO`, `MENUITEM`, `FATTRS`, `FONTMETRICS` (the 2.x layout, 208 bytes), and the `MPFROM*` / `*FROMMP` / `MRFROM*` macros;
   - the constants.

   `os2.h` includes it for `INCL_WIN`, `INCL_GPI` or `INCL_PM`, and the familiar finer names (`INCL_WINWINDOWMGR`, `INCL_GPIPRIMITIVES` …). Calls are `APIENTRY`; window and dialog procedures are `EXPENTRY` (both cdecl). Its structures are packed to 2 bytes, as `FRAMECDATA` must be (10 bytes).
3. **The header's constants come from Warpine where Warpine has them.** A one-off script wrote `pm.h`'s constant blocks from Warpine's own values. The rest are written from the reference: 39 values, such as `WM_SHOW`, the colours 3–15 and `MB_INFORMATION`.
4. **`gen_api check-headers` compares every integer constant:** each `#define NAME value` whose value is an integer, possibly negative or cast (`((HWND)1)`). It is compared with Warpine's `const NAME: <integer> = value` wherever Warpine has the name. A name Warpine defines with two different values is left out. Warpine's values are held to the toolkit headers by `constants.rs`, so the SDK is held to them too without carrying toolkit text. Of the SDK's 962 constants, 801 are compared.
5. **`lx-link` knows every module** Warpine provides. An unknown module is a link error naming the symbol, not a warning. A unit test checks that every module of the embedded def resolves.
6. **The 8 dispatcher comments** take the `// Name(params)` form the other arms use, so `gen_compat` and `build.rs` find them. A `gen_compat` test rejects a PM arm comment without the `(`.
7. **The sample:** `samples/c_pm_test` checks 33 things, built by GCC and by clang. It builds a frame and client, runs the message loop, and checks window procedures called back on an aligned stack, sent and posted messages, a timer, window words and text, rectangles, atoms, and GPI drawing read back with `GpiQueryPel`. It ends itself.

## Warpine's choices (unverified)

- **`FONTMETRICS`** stops at `lMatch` (208 bytes): Warpine writes that much, and later fields (`FamilyNameAtom`, `FaceNameAtom`, `PANOSE`) are left out.
- **The whole of `pm.h` is packed to 2 bytes:** `FRAMECDATA` requires it; for the other structures it changes nothing.
- **The 39 constants not in Warpine** come from the reference without a second check.

## Consequences

- **A C program can be a PM program:** `c_pm_test` passes its 33 checks built by GCC 16 and by clang 21.
- **Header drift is caught for PM too:**
  - a call not served by a dispatcher has no def entry, so declaring it fails the check;
  - a constant that disagrees with Warpine fails `cargo test`.
- **The compatibility chart counts all 217 PMWIN calls** (86% of the snapshot's PMWIN, from 83%).
- **Coverage:** of the def's entries for the modules the headers cover, 149 are not declared yet, most of them PM calls with structures not declared yet (hooks, drag and drop, bitmaps built in memory, help).
- **Tests:**
  - `lx_link.rs`: `test_every_def_module_resolves`;
  - `gen_compat.rs`: `every_pm_arm_comment_names_its_call_with_parentheses`;
  - `gen_api.rs`: `EXPENTRY` declarations are found too (`WinDefWindowProc` and `WinDefDlgProc` were missed); the constant test now covers negative, cast and commented values and function-like macros;
  - `tests/integration.rs`: `gui_c_pm_from_gcc_and_clang`.
