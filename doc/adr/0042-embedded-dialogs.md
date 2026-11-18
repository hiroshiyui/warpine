# ADR-0042: Embed a dialog whose parent is a window into that window

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_types.rs` (`is_embedded_dialog`, `is_top_dialog`, `in_dialog`, `embedded_dialogs`, `hit_controls`, `embedded_dialog_at`, `find_frame_for_hwnd`, `get_abs_rect_in_frame`), `src/loader/pm_win.rs` (`create_dialog`, `WinEndPaint`), `src/loader/mod.rs` (dialog painting), `src/gui/sdl_renderer.rs` (`route_button1`); `samples/pm_child_dialog_test`; IBM *Presentation Manager Programming Reference* (`WinLoadDlg`, `WinDlgBox`: `hwndParent`) and *PM Programming Guide* ("Windows": a child window is clipped to its parent)

## Context

In PM a dialog is an ordinary window, and its parent decides where it lives. With `HWND_DESKTOP` it is a top-level window. With a client, a notebook or another dialog, it is a child window, positioned relative to that parent and clipped to it. Notebook pages (`BKM_SETPAGEWINDOWHWND`) are almost always dialogs loaded with the notebook as parent, and many programs place dialogs inside their client windows.

Warpine made every `#Dialog` a frame: its own surface, Z-order place and chrome, positioned relative to its parent's *frame*. A dialog meant to be part of a window floated over it instead, did not follow the frame when it moved, and could end up behind the frame when the frame was activated.

## Decision

- **Two kinds of dialog.** A dialog whose parent is a window (not `HWND_DESKTOP` / 0) is embedded (`is_embedded_dialog`). A dialog on the desktop stays a frame (`is_top_dialog`, `is_frame`).
- **An embedded dialog is a child window like any other:**
  - Its template position is relative to its parent.
  - `find_frame_for_hwnd` and `get_abs_rect_in_frame` pass through it to the enclosing frame, whose surface it is drawn into (its face is clipped to its rectangle).
  - It has no surface, Z-order place, chrome, activation or initial focus.
  - It is shown only if its template has `WS_VISIBLE`.
- **Its controls stay its own.** They paint with the dialog and are not among the parent's controls (`builtin_control_descendants` still stops at every dialog), so its Tab order and `WinDefDlgProc` keys stay inside it. The pointer still finds them: `hit_controls` / `control_at` go through visible embedded dialogs, and a click on the dialog's own area goes to the dialog in its coordinates (`embedded_dialog_at`).
- **Repainting order.** After a window paints (`WinEndPaint`), and after a dialog paints its face, the dialogs embedded in it are invalidated, so they paint over it again.
- **A modal dialog embedded in a window whose frame `WinDlgBox` disables cannot be clicked,** as in PM, where a disabled window's children get no input. Programs pass `HWND_DESKTOP` for modal dialogs.

## Consequences

- Dialogs in client windows and, next, notebook pages behave as in PM. They move with their frame and are clipped to their parent.
- `WinDlgBox` / `WinLoadDlg` calls with a window parent change behaviour. Their dialogs were floating frames before; all samples pass `HWND_DESKTOP` and are unchanged.
- **Not covered yet:** an embedded dialog that asks for chrome (`FCF_TITLEBAR`, a border) is drawn without it, and Tab does not move between an embedded dialog's controls and the controls around it.
