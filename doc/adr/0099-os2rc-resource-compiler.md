# ADR-0099: os2rc, a resource compiler for the C SDK, and resources in lx-link

- **Status:** Accepted; builds on ADR-0095–0098
- **Date:** 2026-11-21
- **Sources:**
  - IBM's *Presentation Manager Programming Reference* and the resource-script statements it documents (MENU, DLGTEMPLATE, ACCELTABLE, STRINGTABLE …); `<pmwin.h>`'s `MENUTEMPLATE`, `DLGTEMPLATE` / `DLGTITEM` and `ACCELTABLE` layouts.
  - The classic OS/2 `.res` format (0xFF, type, 0xFF, name, flags, size, data).
  - The LX format: object flags (`OBJ_RESOURCE`), the resource table.
  - Warpine's own readers of these formats: `pm_win.rs` (menus), `pm_dialog.rs` (dialogs, presentation parameters), `pm_winmisc.rs` (string bundles), `pm_accel.rs`, `lx/header.rs`.
  - The oracle: Open Watcom's `wrc -zm -bt=os2`, run on scripts written for this purpose and on the samples' scripts. Only its output is observed, never its source.

## Context

ADR-0095–0098 let GCC and clang build OS/2 programs with `os2.h` and a C runtime, but such a program had no resources: no menus, dialogs, string tables or icons. The user chose Warpine's own resource compiler over embedding Open Watcom's `.res` files, so that the second toolchain does not need Open Watcom to build anything.

## Decision

1. **`os2rc` (`src/bin/os2rc.rs`)** compiles a resource script into a `.res` file in the classic OS/2 format:

   ```bash
   os2rc [-I dir]... [-D name[=value]]... in.rc [-o out.res]
   ```

   `lx-link` takes `.res` files beside the objects.
2. **A C preprocessor** runs first:
   - `#define` (object-like and function-like), `#undef`, `#include`, `#if` / `#ifdef` / `#ifndef` / `#elif` / `#else` / `#endif` with `defined`, `#error`;
   - comments and line continuations; `RCINCLUDE`;
   - `RC_INVOKED` is defined;
   - header files (`.h`) contribute only their directives, so a script includes the SDK's `os2.h`. Under `RC_INVOKED`, `os2.h` selects the window manager's constants, and `os2def.h` skips its compiler check.
3. **Expressions** follow C (`| & ^ << >> + - * / %`, unary `- ~ !`, parentheses, casts such as `((PSZ)0xFFFF0003L)`). `NOT x` in a style clears x's bits. A name the script did not define is an error with its file and line.
4. **The statements and their bytes**, each matched to `wrc`:
   - **`MENU`:** a template of size, codepage, the word 4 and the item count, then items (style, attribute, id, text). A submenu's items form a nested template.
     - `MENUITEM SEPARATOR` is `MIS_SEPARATOR` with `MIA_DISABLED` and id 0xFFFF.
     - A given style replaces `MIS_TEXT`, and `SUBMENU` adds `MIS_SUBMENU`.
   - **`DLGTEMPLATE` / `WINDOWTEMPLATE`:** the 14-byte header (status 1, no focus item, codepage), the 30-byte items in depth-first order, then a pool. For each item, the pool holds its class name, text (an `ICON` control's is 0xFF and the resource id), control data and presentation parameters.
     - **Frame-like items:** `DIALOG` defaults to `FS_DLGBORDER | WS_SAVEBITS | WS_CLIPSIBLINGS`, `FRAME` to `WS_VISIBLE`, `WINDOW` to nothing. All three always carry 4 bytes of `FCF_*` control data.
     - **Controls:** each control statement has its class and default style (for example `PUSHBUTTON`: `BS_PUSHBUTTON | WS_TABSTOP | WS_VISIBLE`). A given style is ORed into the default; `CONTROL` takes its style as given.
     - **`CTLDATA` and `PRESPARAMS`:** `CTLDATA` words are 16 bits. A `PRESPARAMS` block is its size, then id / size / data entries; a number takes 4 bytes, and a string keeps its NUL.
   - **`ACCELTABLE`:** a count, a codepage, then fs / key / cmd entries.
     - `AF_CHAR` is set unless `VIRTUALKEY` is.
     - `"^x"` is x with `AF_CONTROL`.
   - **`STRINGTABLE` / `MESSAGETABLE`:** bundles of 16 strings (string n in bundle n / 16 + 1). Each bundle is a codepage, then for each string a length that counts the NUL, the bytes and the NUL. Blocks merge, and the bundles come after the other resources.
   - **`RCDATA`:** a number takes 2 bytes, 4 with an L suffix; a string keeps its NUL.
   - **File resources:**
     - `ICON` / `POINTER` / `BITMAP` / `FONT` / `RESOURCE` hold the file as it is;
     - a pointer or icon with id 1 is also written as type 22, name 22;
     - `DEFAULTICON` writes type 22 / 22 and pointer 999;
     - `DLGINCLUDE` holds the file name.
   - **`CODEPAGE`** applies to the resources after it (850 before any).
   - **Memory flags:** `PRELOAD`, `LOADONCALL`, `MOVEABLE`, `FIXED`, `DISCARDABLE`, `PURE`, `IMPURE`. The defaults are 0x1030, or 0x0030 for accelerator tables and bitmaps.
5. **`lx-link`** reads `.res` inputs (numeric types and names) and writes:
   - **a fourth object** flagged `OBJ_READ | OBJ_RESOURCE`, at base 0 because the loader places resource objects (ADR-0094), holding every resource 4-byte aligned;
   - **the LX resource table,** 14 bytes per entry, after the page map; its offset and count go in the header.

   A type and name given twice is a link error.
6. **The SDK:** `os2.mk` names the compiler (`OS2_RC`, `OS2_RCFLAGS`). `pm.h` gains the control styles scripts use (`CBS_*`, `MLS_*`, `SPBS_*`, `SLS_*`, `VS_*`, `CCS_*`, `BKS_*`), and `dos.h` the `RT_*` types, all checked against Warpine's constants by `check-headers`.
7. **`samples/c_pm_test`** has a resource script with:
   - a menu and accelerator table its frame loads (`FCF_MENU | FCF_ACCELTABLE`);
   - a dialog `WinDlgBox` runs, whose entry field's text the dialog procedure reads before dismissing it;
   - a string `WinLoadString` reads;
   - `RCDATA` read with `DosGetResource`.

## Warpine's choices (unverified)

These follow `wrc`; the IBM resource compiler may differ:
- **Default styles:** the control statements' defaults, and that a given style is ORed into them.
- **`CTLDATA`** numbers are 16 bits even with an L.
- **The default icon:** the type-22 copy of pointer 1, and `DEFAULTICON` writing pointer 999.
- **The menu template** carries the word 4 after its codepage.
- **Memory flags** default as above.
- **`FONT`** holds the file as it is, with no font directory.
- **Not supported:** `HELPTABLE`, `HELPSUBTABLE`, `ASSOCTABLE` and `FONTDIR`, which are an error; and resource names given as strings.

## Consequences

- **A C program can carry resources:** `c_pm_test` passes 47 checks built by GCC and by clang. Six of them are about its resources.
- **The output is byte-identical to `wrc -zm`** for every script in `samples/` (16) and `tests/rc/` (6). `os2rc_matches_open_watcom_wrc` compares each resource whenever Open Watcom is installed. Three unit tests keep `wrc`'s bytes for scripts that use menus, dialogs with presentation parameters, control data, class names and icons, accelerators, string and message tables, and `RCDATA`, so they hold without Open Watcom.
- **`lx-link` can link programs with resources** for any compiler that emits ELF (Rust guests too).
- **Tests:**
  - `os2rc.rs`, 8 unit tests;
  - `lx_link.rs`, 2: the resource object and table; `.res` parsing and its errors;
  - `tests/integration.rs`: `os2rc_matches_open_watcom_wrc`, and `gui_c_pm_from_gcc_and_clang` with resources.
