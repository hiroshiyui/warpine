# ADR-0069: An embedded dialog draws its own chrome, and Tab crosses its edge

- **Status:** Accepted; extends ADR-0042 (dialogs embedded in a window)
- **Date:** 2026-11-20
- **Sources:**
  - The code:
    - `src/loader/pm_dialog.rs` (`paint_embedded_dialog_chrome`, `tab_around_embedded_dialogs`);
    - `src/loader/mod.rs` (the dialog's `WM_PAINT`);
    - `src/loader/pm_types.rs` (`tab_scope`, `tab_items`, `tab_around_embedded_dialogs`);
    - `src/loader/pm_win.rs` (`def_window_proc`);
    - `src/loader/constants.rs` (the chrome colours, moved from `src/gui/sdl_renderer.rs`).
  - Sample: `samples/pm_embdlg_test`.
  - IBM *PM Programming Reference* (Warp 4): `WinLoadDlg`, `WinDefDlgProc` (Tab and Backtab), `WinDefWindowProc`, `WinWindowFromPoint`, the `FCF_*` and `FS_*` frame flags; *PM Programming Guide*: "Windows" (a child window is clipped to its parent), "Dialog windows".

## Context

ADR-0042 made a dialog whose parent is a window an ordinary child window, with three gaps:
- **Chrome.** It had no title bar or border, even when its template asked for them. The compositor draws chrome only around frames with their own surface.
- **Tab.** Tab stayed inside it: from its last control Tab went back to its first, never to the controls of the client around it, and Tab on those controls never reached it.
- **The pointer.** It was unclear whether `WinWindowFromPoint` and pointer messages reach windows of the program's own class inside it.

Pointer input has been routed when it is read since ADR-0051, through `window_under_pointer`, which walks into any child window. The new sample shows that both already reach such a window.

## Decision

### Chrome

- **Drawn with the dialog.** After it fills its face, an embedded dialog draws the chrome its frame flags ask for, inside its own rectangle, in the surface of the frame it lies in:
  - `FCF_TITLEBAR`: a title bar along its top with its text;
  - `FCF_BORDER`, `FCF_DLGBORDER`, `FCF_SIZEBORDER`: a border.
- **Like a frame's.** It has the compositor's geometry and colours: `CHROME_TITLE_H`, `CHROME_BORDER_W`, the system title colours. Those colours now live in `constants.rs`, where the loader and the compositor both read them.
- **Its controls stay where they were.** They are placed relative to the dialog window, border included, as in a dialog frame.

### Tab across the dialog's edge

- **From the dialog.** `WinDefDlgProc`'s Tab cycles through the controls of the tab scope (`tab_scope`). When the outermost dialog of a chain is itself embedded in a window that is not a dialog, that scope is the control root of the frame around it (the client), not the dialog. Tab therefore goes from the dialog's last control to the client's next one.
- **Into the dialog.** A control passes the keys it does not use to its owner, so Tab on a client's control reaches the client. The client's `WinDefWindowProc` then moves the focus to the next tab stop among its own controls and those of the dialogs embedded in it. It does so only for Tab and Shift+Tab, only when the window holds a shown embedded dialog, and only when the focus is on one of those controls.
- **Notebook pages are unchanged.** A page in a notebook in a dialog keeps that dialog as its scope (ADR-0042, the notebook record).

### The pointer

There is no change. The sample checks that `WinWindowFromPoint` finds such a window from the desktop, and the dialog from the client, and that a click on it gets `WM_BUTTON1DOWN` in its own coordinates.

### Warpine's choices (unverified)

- **The title bar.**
  - It has the inactive colours: an embedded dialog is never the active window in Warpine.
  - It has no system-menu, minimize, maximize or close buttons.
  - It is not a `FID_TITLEBAR` window: a click on it goes to the dialog as `WM_BUTTON1DOWN`, and dragging it does not move the dialog.
- **Tab.**
  - That Tab crosses between an embedded dialog and the client's controls at all: the references describe Tab only within a dialog.
  - That the client's `WinDefWindowProc` handles Tab when the client holds an embedded dialog.

## Consequences

- **Templates that ask for a border show one.** Embedded dialogs whose templates ask for a border (the resource compiler's default style gives one) now show it. Notebook pages are among them, which is why OS/2 programs write `NOT FS_DLGBORDER` for theirs. The golden picture of `pm_notebook_test` changed for this reason.
- **Tab in a client.** A client holding an embedded dialog now takes Tab between its controls; one without stays as it was.
- **Still open:**
  - a working title bar on an embedded dialog (moving it, its buttons);
  - `WM_QUERYDLGCODE`.
- **Tests:**
  - `pm_dialog.rs`: `an_embedded_dialog_draws_its_own_chrome`;
  - `pm_types.rs`: `tab_crosses_between_a_client_and_its_embedded_dialog`;
  - `pm_window.rs`: `def_window_proc_tabs_into_an_embedded_dialog`;
  - `samples/pm_embdlg_test`: 14 checks on both desktops, with a golden picture.
