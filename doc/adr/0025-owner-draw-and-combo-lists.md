# ADR-0025: Run owner-draw calls before the API returns, and drop combination-box lists inside the parent

- **Status:** Accepted; the owner-call queue also carries `WM_ERASEBACKGROUND` and `WS_SYNCPAINT` calls, and hooks use call chains (ADR-0026)
- **Date:** 2026-11-18
- **Sources:** commit f245cfc; `src/loader/pm_owner.rs`, `src/loader/pm_combo.rs`, `src/loader/vcpu.rs` (`FrameKind::OwnerDraw`), `src/loader/pm_types.rs` (`WindowManager::owner_calls`, `builtin_control_descendants`); IBM *Presentation Manager Programming Reference* (`WM_MEASUREITEM`, `WM_DRAWITEM`, `BN_PAINT`, `WC_COMBOBOX`)

## Context

Warpine's built-in controls (list boxes, buttons, entry fields …) are host code: they paint into their frame's surface on whatever thread is in an API at the time — `WinDispatchMsg` handling a click, `WinEndPaint` repainting the controls, `LM_INSERTITEM`. Owner-drawn controls (`LS_OWNERDRAW`, `BS_USERBUTTON`, `MIS_OWNERDRAW` menu items) must instead send `WM_MEASUREITEM`, `WM_DRAWITEM` or `WM_CONTROL(BN_PAINT)` to their owner synchronously, as `WinSendMsg` would, and the owner is guest code that may only run on its own thread with a presentation space.

Combination boxes drop a list below their entry field over the controls beneath it. In a single-surface-per-frame compositor (ADR-0011) a separate top-level window for the list would need its own surface and z-order handling.

## Decision

- **Owner calls are queued and made before the API returns.** Painting does at once what the host can (background, border) and queues the owner's part as `OwnerCall`s in `WindowManager::owner_calls`. When an API returns to a thread whose message queue owns the control (after `ApiResult::Normal`, and in the modal dialog loop), the vCPU runs the calls one after the other as guest callbacks (`FrameKind::OwnerDraw`) before the caller sees the API's result. A thread runs one chain at a time; calls queued meanwhile join it.
- **The owner draws on a canvas**: a bitmap presentation space the size of the control holding a copy of its pels, in the control's own coordinates; what the owner drew is copied back. This also keeps the owner's drawing inside its item.
- **A dropped combination-box list is a child window of the combination box**, painted after the other controls of its parent and hit first, clipped to the parent like any child. The combination box covers the entry field and the space the list drops into, as dialog templates size it, and is transparent to the pointer below the entry row while the list is hidden.

## Consequences

- Owner-drawn list boxes, user buttons and menu items work in dialogs and windows, and the owner always runs on its own thread, as in PM.
- The owner's drawing appears slightly later than the host's part of the paint (before the same API returns, so the program cannot observe the difference).
- `WM_MEASUREITEM` is sent on insertion only, and `WM_DRAWITEM` paints whole items (`fsStateOld` FALSE); finer-grained redraws (highlight changes in menus, deletion) are follow-ups.
- A dropped list cannot reach outside its parent window; whether PM's can is unverified. If it must, the list will need to become an overlay like pop-up menus.
