# ADR-0046: Container icon view, filtering and direct editing: one filter chain, edits written back through the owner's buffer

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_container.rs` (`icon_item`, `arrange_icons`, `icon_extent`, `picture_fields`, `cnr_start_filter`, `container_filter_done`, `cnr_open_edit`, `cnr_close_edit`, `container_realloc_done`), `src/loader/pm_hook.rs` (`ChainThen::ContainerFilter`), `src/loader/pm_owner.rs` (`OwnerCallKind::ContainerEdit`), `src/loader/pm_valueset.rs` (`paint_icon_handle`, `paint_bitmap_handle`), `src/loader/pm_msgbox.rs` (`object_pels`); `samples/pm_cnricon_test`; `<pmstddlg.h>`; IBM *PM Programming Reference* (`CM_FILTER`, `CM_ARRANGE`, `CM_QUERYRECORDFROMRECT`, `CM_QUERYRECORDRECT`, `CM_OPENEDIT`, `CM_CLOSEEDIT`, `CN_BEGINEDIT`, `CN_REALLOCPSZ`, `CN_ENDEDIT`, `CNREDITDATA`, `CNRINFO`) and *PM Programming Guide* ("Container Controls": icon view, filtering, direct editing), as on EDM/2; ADR-0026, ADR-0043, ADR-0044

## Context

Container stage 2b adds three things. Each one needs a choice the earlier container records (ADR-0043, ADR-0044) did not make.

- **Filtering.** `CM_FILTER` asks the application's `BOOL pfnFilter(PRECORDCORE, PVOID pStorage)` about each record before the message returns. Records it rejects get `CRA_FILTERED` and are hidden. A NULL function shows everything again.
- **Direct editing.** The user edits a record's text in an MLE. On save, the owner hears `CN_REALLOCPSZ` and answers TRUE when the string `*ppszText` points to has room for the new text. The container then copies the text there and sends `CN_ENDEDIT`. On discard, only `CN_ENDEDIT` is sent. The text lives in the application's memory (ADR-0043), so the write-back targets a guest address the owner chose.
- **The icon view.** Records sit at `RECORDCORE.ptlIcon` in workspace coordinates. `CNRINFO.ptlOrigin` is the work area's origin. `CM_ARRANGE` and `CCS_AUTOPOSITION` place the records from the upper left. The reference does not say:
  - which point of the item `ptlIcon` names;
  - the arrangement's spacing;
  - how far the work area scrolls.

## Decision

- **A filter is one call chain, one step per record** (ADR-0026).
  - The answers do not decide which record is asked next, unlike a sort's (ADR-0044), so every call is known in advance.
  - `CnrState::filter` keeps the job id and the records asked about.
  - `ChainThen::ContainerFilter` gives the chain's `results` back in the same order. Answers are applied only to records still inserted, and are matched to records before any removed ones are dropped. A job that a newer filter replaced does nothing.
  - Every inserted record is asked, children included.
  - `CMA_FILTER` in `CM_QUERYRECORD` passes over filtered records. Its meaning comes from the header, not from the reference.
- **An edit is an MLE child of the container (`CID_MLE`) and three owner calls.**
  - The notifications are `OwnerCallKind::ContainerEdit` with a `CNREDITDATA`, sent before the API returns like the other `CN_*` notifications.
  - `ppszText` is the address of the record's own `PSZ` field: `pszIcon` / `pszText` / `pszName` / `pszTree` for the view, or `offStruct` in the details view.
  - On save, the container keeps the new text encoded in the window's codepage and sends `CN_REALLOCPSZ` with `cbText` counting the NUL (the reference does not say). When that owner call finishes with TRUE, the container reads `*ppszText` *after* the owner's answer, so it uses the buffer the owner just provided, and writes at most `cbText` bytes there with bounds-checked guest writes. Then it sends `CN_ENDEDIT`.
  - On discard (Esc, Alt+click, scrolling), it sends `CN_ENDEDIT` with `cbText` 0.
  - `CCS_READONLY`, `CRA_RECORDREADONLY`, `CFA_FIREADONLY` and non-`CFA_STRING` columns refuse an edit.
- **Icon-view geometry is Warpine's where the reference is silent.**
  - `ptlIcon` is the icon's lower-left corner. The text lines are centred under the icon.
  - The work area's bottom-left corner shows the workspace point `ptlOrigin`.
  - `CM_ARRANGE` places items left to right from the upper left with `ARRANGE_GAP` (8 pels) between them, wrapping rows, and resets `ptlOrigin`.
  - The vertical scroll bar scrolls by pels over the items' span together with the work area at the origin. Scrolling moves `ptlOrigin`, and `CN_SCROLL` reports the pels.
  - The drawing order (z-order) is the item order.
- **Pictures come from one place.**
  - Icons are pointer handles: `WinLoadPointer` images, or Warpine's own pictograms for the system icons (`SPTR_FILE`, `SPTR_FOLDER`, `SPTR_MULTFILE`, `SPTR_PROGRAM`, `SPTR_APPICON` join the message-box ones).
  - Bitmaps are GPI bitmaps.
  - Both are drawn stretched by the helpers the value set already used.
  - `CA_DRAWBITMAP` without `CA_DRAWICON` picks bitmaps, and `CV_MINI` picks the mini handles, falling back to the normal ones.

## Consequences

- A filter over n records costs n guest calls in one chain; the results vector is n words.
- Editing works in every view through the mouse (Alt + button 1) and `CM_OPENEDIT` / `CM_CLOSEEDIT`. It writes only into the buffer the owner hands back, and only when the owner says TRUE.
- Still missing:
  - editing the container title and column headings;
  - saving when the focus moves away without a click in the container;
  - `CM_ERASERECORD`;
  - `RECORDINSERT.zOrder` and `CMA_ZORDER` as a separate order;
  - the grid arrangements;
  - horizontal scrolling of the icon view.
- Unverified:
  - `ptlIcon`'s reference point;
  - the arrangement spacing;
  - `cbText` counting the NUL;
  - `CN_REALLOCPSZ` sent also for unchanged text;
  - the filter asking about child records;
  - `CMA_FILTER`'s meaning;
  - the tree view's default icon size (16 pels here);
  - mini handles falling back to the normal ones.
