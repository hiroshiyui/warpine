# ADR-0067: WM_CONTEXTMENU and the mouse wheel as routed pointer input; container tree lines, tree moves, grids, erasing and stacking

- **Status:** Accepted; extends ADR-0051 and ADR-0058 (pointer input routed when read), ADR-0052 (desktop protocol, now version 7) and the container ADRs 0043–0046 and 0065
- **Date:** 2026-11-19
- **Sources:**
  - The code:
    - `src/loader/pm_input.rs` (`WM_CONTEXTMENU`, `is_context_key`, `WHEEL_LINES`, `take_up_input`);
    - `src/gui/sdl_renderer.rs` (the button-2 click, `Event::MouseWheel`);
    - `src/gui/desktop_events.rs` (`wheel_steps`);
    - `src/desktop/proto.rs` (`Input::Wheel`, `MAX_WHEEL_STEPS`);
    - `src/gui/desktop_server.rs`;
    - `src/gui/gui_script.rs` (`wheel X Y N`);
    - `src/loader/pm_container.rs` (`tree_lines`, `arrange_grid`, `default_grid`, `CnrState::move_tree` / `position` / `lower`, `cnr_workspace`).
  - Sample: `samples/pm_cnrgrid_test`.
  - IBM *PM Programming Reference* (Warp 4): `WM_CONTEXTMENU`, `CM_MOVETREE`, `CM_SETGRIDINFO` / `CM_QUERYGRIDINFO`, `CM_ARRANGE`, `CM_ERASERECORD`, `CN_CONTEXTMENU`, `CN_HELP`, `RECORDINSERT.zOrder`, `CNRINFO` / `TREEITEMDESC`; the Warp 4 Toolkit's `<pmstddlg.h>` and `<pmwin.h>`.

## Context

The container's last open features needed input that Warpine did not produce:
- **Context menus.** `CN_CONTEXTMENU` follows `WM_CONTEXTMENU`, Warp 4's message for the context-menu gesture. That gesture is a button-2 click, or Shift+F10 from the keyboard.
- **The wheel.** Containers, like any window, scroll with the mouse wheel. Warp 4 has no wheel message; OS/2 wheel drivers send `WM_VSCROLL` to the window under the pointer.

On the session desktop (ADR-0052), input reaches a program through the desktop server's protocol, which had no wheel message.

The container also lacked:
- the tree view's look: tree lines, and expander pictures from `CNRINFO` and each record's `TREEITEMDESC`;
- `CM_MOVETREE`;
- the grid arrangement;
- `CM_ERASERECORD`;
- a stacking order of its own (`RECORDINSERT.zOrder`);
- the workspace rectangle.

## Decision

### Input

- **The context-menu gesture.**
  - Button 2 coming up without having started a drag (`DragPress` not fired) queues a pointer event `WM_CONTEXTMENU` after `WM_BUTTON2UP`. Like every pointer event it is routed when read, and delivered with mp1 = the point in the window, mp2 = TRUE.
  - Shift+F10 pressed becomes `WM_CONTEXTMENU` (mp1 = 0, mp2 = FALSE) for the focus window, instead of its `WM_CHAR` (`is_context_key`).
- **The wheel.**
  - Each notch is `WHEEL_LINES` (3) pointer events `WM_VSCROLL` with `SB_LINEUP` / `SB_LINEDOWN` in mp2's high word.
  - When read, it goes to the window under the pointer, or to a scroll bar's owner. Its mp1 is 0, not a point.
  - On the session desktop the server sends it as the new `Input::Wheel { x, y, steps }` (protocol version 7; `steps` within ±64), routed to the program under the pointer as motion is.
  - Scripts drive it with `wheel X Y N`.

### Containers

- **Messages the container answers.**
  - `WM_CONTEXTMENU` and `WM_HELP` → `CN_CONTEXTMENU` / `CN_HELP` to the owner, as owner calls (sent).
  - mp2 is the record under the pointer for a pointer context menu, else the cursored record.
- **Tree look.**
  - `CA_TREELINE` draws lines `cxTreeLine` pels wide (1 by default). Which levels run through a row is the pure `tree_lines`: one backward pass for "has a later sibling", one forward pass for the ancestors' lines.
  - An expander is a picture when the record's `TREEITEMDESC` or `CNRINFO` has one for its state. Icons are preferred, bitmaps with `CA_DRAWBITMAP` alone. The expander is then `TREE_PICTURE` (16) pels wide.
  - Expander hits are measured from the expander's own place. The old hit test measured back from the text, which missed whenever the tree view showed record pictures.
- **`CM_MOVETREE`** (`TREEMOVE`)
  - It moves the record, with `flMoveSiblings` also the siblings after it, and the descendants, under the new parent at `pRecordOrder`, in order.
  - A record cannot move under itself or its descendants, nor after a moving record.
  - It shares `CnrState::position` with insertion.
- **Grid.**
  - `CM_SETGRIDINFO` sets the square (`cxGrid` × `cyGrid`, up to 1024).
  - `CM_QUERYGRIDINFO` reports it with the rows and columns of the work area.
  - `CM_ARRANGE(CMA_ARRANGEGRID)` places icons row by row in squares (`arrange_grid`).
  - `CMA_ARRANGESELECTED` in mp2 arranges only the selected records.
- **Erasing.** `CM_ERASERECORD` leaves a record out of painting until `CM_INVALIDATERECORD` names it, or names every record (a count of 0).
- **Stacking.** `CnrState::zorder` keeps the icon view's stacking: insertion puts records on top, `zOrder` `CMA_BOTTOM` at the bottom. The icon view draws, hit-tests and answers `CMA_ZORDER` by it.
- **The workspace rectangle.** `CM_QUERYVIEWPORTRECT(CMA_WORKSPACE)` gives it in workspace coordinates: the icon view's span of items and the work area, or the other views' content size.

### Warpine's choices (unverified)

- **Input.**
  - Shift+F10 replacing its `WM_CHAR`.
  - Three lines a wheel notch.
  - The wheel's `WM_VSCROLL` with mp1 0, and going to a scroll bar's owner.
  - Context menus only for button-2 clicks that start no drag.
- **Containers.**
  - `CN_HELP` / keyboard `CN_CONTEXTMENU` naming the cursored record.
  - No root-level tree lines.
  - The expander picture's size.
  - The default grid square.
  - `CMA_ARRANGESELECTED` as a flag in mp2.
  - Erased records coming back on `CM_INVALIDATERECORD`.
  - Selection not raising a record in the stacking.

## Consequences

- **Context menus and the wheel.** Programs get context menus from button 2 and Shift+F10, and the wheel scrolls containers, list views and clients that handle `WM_VSCROLL`, on both desktops.
- **The protocol.** Its version is 7, so clients and servers must be from the same build, as before.
- **Containers.** The container's remaining open items are owner background painting (`CA_OWNERPAINTBACKGROUND`), records shared between containers, lazy drag and keeping an edit when the focus leaves.
- **Tests:**
  - `pm_container.rs`: tree lines, moves, pictures and their hits, erasing, stacking, grids, the workspace, context menu and help;
  - `pm_input.rs`: Shift+F10 and wheel routing;
  - `proto.rs`, `desktop_events.rs`, `gui_script.rs`: the wheel message;
  - `samples/pm_cnrgrid_test`: 24 checks, with a golden picture.
