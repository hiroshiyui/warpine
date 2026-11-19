# ADR-0065: Container details panes, horizontal scrolling, owner drawing through owner calls, title and heading edits; hidden built-in windows do not paint

- **Status:** Accepted; extends ADR-0043, ADR-0046 and ADR-0025
- **Date:** 2026-11-19
- **Sources:**
  - The code: `src/loader/pm_container.rs` (`Pane`, `detail_panes`, `split_x`, `Snapshot::bars` / `panes` / `columns` / `split_rect`, `cnr_hscroll`, `cnr_split_press` / `cnr_split_drag`, `paint_container_parts`, `container_item_rect`, `title_edit_target`), `src/loader/pm_owner.rs` (`OwnerCallKind::ContainerDrawItem`), `src/loader/vcpu.rs` (the `hItem` pointer), `src/loader/mod.rs` (built-in `WM_PAINT`).
  - Sample: `samples/pm_cnrdetail_test`.
  - IBM *PM Programming Reference*, "Container Controls" (`CNRINFO.pFieldInfoLast` / `xVertSplitbar`, `CA_OWNERDRAW`, `CFA_OWNER`, `WM_DRAWITEM` in containers, `CNREDITDATA`, `CN_SCROLL`); the Warp 4 Toolkit's `<pmstddlg.h>` (`CNRDRAWITEMINFO`, the `CID_*` ids, `CA_TITLEREADONLY`, `CFA_FITITLEREADONLY`).
  - Earlier decisions: ADR-0025 (owner calls), ADR-0043 (records in guest memory), ADR-0046 (direct editing).

## Context

The container's details view drew all its columns in one band. That band was cut off at the container's edge, and the following were missing:
- horizontal scrolling;
- the split bar, which divides the columns into two panes that scroll on their own (`pFieldInfoLast`, `xVertSplitbar`);
- owner drawing;
- editing the container title and column headings.

## Decision

### Panes and horizontal scrolling

- **The model.** The details view is one or two `Pane`s, each with its columns, their total width and its own scroll offset (`CnrState::hscroll`).
- **Pure layout.** `detail_panes` / `split_x` are pure functions:
  - the split bar is `SPLIT_W` pels wide;
  - each pane is at least `MIN_PANE` wide;
  - `xVertSplitbar` not above 0 puts the bar in the middle (Warpine's choice).
- **Columns.** Columns are measured once per snapshot (`Snapshot::cols`) and placed per pane, so each is drawn cut to its pane.
- **Horizontal scroll bars.**
  - `CID_HSCROLL` (left or only pane) and `CID_RIGHTHSCROLL` are children made with the container.
  - They are shown when a pane's columns, or in the icon view the workspace, are wider than the space.
  - A horizontal bar takes height from the rows, which may then need the vertical bar (`Snapshot::bars` decides in two passes).
- **The icon view** scrolls across by moving `ptlOrigin.x`.
- **Notifications.** Scrolling sends `CN_SCROLL` with `CMA_HORIZONTAL`.
- **Messages.**
  - `CM_SCROLLWINDOW(CMA_HORIZONTAL)` scrolls the left pane.
  - `CM_QUERYVIEWPORTRECT` answers for the right pane when mp2's high word asks for it.
- **The split bar is dragged** with button 1, capturing the pointer through `pressed_control` as a slider does. Its place is kept in `CNRINFO.xVertSplitbar`, which `CM_QUERYCNRINFO` reports.

### Owner drawing

- **What the owner draws.**
  - `CA_OWNERDRAW`: every record's icon and text (`CMA_ICON` / `CMA_TEXT` parts).
  - `CFA_OWNER`: that column's cells.
- **How.** It draws through owner calls (ADR-0025): the paint fills the item's background and queues `ContainerDrawItem`. On the owner's thread, before the API returns, the owner gets:
  - `WM_DRAWITEM` with an `OWNERITEM` on a canvas PS;
  - in it, `hItem` pointing to a `CNRDRAWITEMINFO` placed right after the `OWNERITEM` on the guest stack. `vcpu.rs` writes that address, since the block's place is known only there.
- **The answer.**
  - TRUE keeps the owner's picture.
  - FALSE makes the container paint that one part itself (`paint_container_item`).

### Editing titles and headings

- **The container title.**
  - It opens on `CM_OPENEDIT` with `pRecord` NULL and id `CID_CNRTITLEWND`, or on Alt+click on the title band.
  - The title's `PSZ` lives host-side in `CNRINFO`, so the edit's `ppszText` is a guest cell carved from the container's heap (`ItemKind::Cell`, made once).
  - After `CN_REALLOCPSZ` answers TRUE, the text is copied where the cell points, and that becomes the title. So an owner may hand over a new buffer.
- **A column heading.**
  - It opens with id `CID_LEFTCOLTITLEWND` / `CID_RIGHTCOLTITLEWND` and `pFieldInfo`, or on Alt+click on the heading.
  - Its `ppszText` is the field info's `pTitleData`.
- **When they cannot be edited.** `CA_TITLEREADONLY`, `CFA_FITITLEREADONLY`, a bitmap heading, a part not shown, and `CCS_READONLY` refuse an edit.
- **Record cells** in the right pane report `CID_RIGHTDVWND`.

### Hidden built-in windows

- **A hidden built-in control (`WC_*`) paints nothing on `WM_PAINT`.** A scroll bar hidden after its paint was queued kept drawing its stale picture over the container. Warpine's own pseudo-windows, such as `#Menu`, which are never marked visible, are left out of this rule.
- **Sync before painting.** The container syncs its scroll bars before it paints, not after.

### Warpine's choices (unverified)

- The split bar's default place, width and the narrowest pane.
- The step of a horizontal arrow (16 pels).
- `CM_SCROLLWINDOW(CMA_HORIZONTAL)` acting on the left pane.
- `idItem` = `CMA_ICON` / `CMA_TEXT`, `fsState` = selected and `fsAttribute` = `flRecordAttr` in the `OWNERITEM`, the `OWNERITEM`'s `hwnd` being the container.
- The cell holding the title while it is edited.
- Owner drawing being asked at every repaint.

## Consequences

- **Wide details views.** Details views wider than the container can be read, and split views work as the PM Guide describes.
- **Owner-drawn containers.** Owner-drawn containers show their owner's picture.
- **Editable titles and headings.**
- **Hidden built-in controls.** No hidden built-in control draws any more. One unit test that painted a never-shown control now shows it.
- **Tests:**
  - `pm_container.rs`: panes and split, horizontal scrolling in both views, the split drag, owner drawing and its FALSE answer, title and heading edits, a hidden scroll bar not painting;
  - `samples/pm_cnrdetail_test`: 28 checks, with a golden picture.
