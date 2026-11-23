# ADR-0117: Keyboard menu mode taken at accelerator translation

- **Status:** Accepted
- **Date:** 2026-11-23
- **Sources:**
  - PM Reference, `WM_SYSCOMMAND`: `SC_APPMENU` "sends MM_STARTMENUMODE to the FID_MENU control"; `MM_STARTMENUMODE` (fShowSubmenu, fResumeMenu) and `MM_ENDMENUMODE`.
  - The CUA keys: F10 to the menu bar, F1 for help, Alt toggling the menu bar.
  - Open Watcom's `pmwin.h` for the values (`SC_APPMENU` 0x8006, `MM_STARTMENUMODE` 0x0185, `MM_ENDMENUMODE` 0x0186).

## Context

A frame's menu bar (`FID_MENU`) took only the mouse: Warpine's menu window has no message queue of its own and its pull-downs are frame chrome the renderer draws, so the PM model — the menu window takes the focus and its window procedure reads the keys — has nowhere to run. Programs that are driven from the keyboard need F10 and Alt.

## Decision

1. **F10 is a system accelerator for `WM_SYSCOMMAND(SC_APPMENU)`**, F1 one for `WM_HELP`. `WinDefWindowProc` answers `SC_APPMENU` by putting the frame's `FID_MENU` into menu mode; `MM_STARTMENUMODE` / `MM_ENDMENUMODE` sent to the menu do the same. Alt pressed and released with no other key between becomes `SC_APPMENU` too (in menu mode, it leaves).
2. **Menu mode takes the keys where accelerators are translated** (`translate_queued_accelerator` → `pm_menubar::menu_mode_message`), when a message leaves the queue: a keystroke for the menu's frame drives the menu and is delivered as `WM_NULL`, or as the chosen item's `WM_COMMAND` / `WM_SYSCOMMAND` / `WM_HELP` to the client with `MPFROM2SHORT(CMDSRC_MENU, FALSE)`. The focus does not move. A button press elsewhere ends menu mode.
3. **The state is the menu window's**: `window_ulong[-2]` (bar item, pull-down row), beside the open pull-down in `[-1]` that the mouse code already uses; the renderer draws the highlight from both.
4. **The keys** are pure logic (`menu_key`): Left / Right along the bar (a dropped pull-down stays dropped), Down / Enter drop, Up / Down / Home / End in the pull-down, Enter or a mnemonic letter choose, Esc closes the pull-down and then leaves, F10 leaves. Separators and disabled items are skipped.

## Alternatives considered

- **Giving the menu window the focus**, as PM does. It would need a queue and a guest-visible focus change (`WM_SETFOCUS`, `EN_KILLFOCUS` of the control that had it), and the menu would still be drawn by the renderer.
- **Handling the keys on the GUI thread**, like the system menu's pop-up (`gui/popup_menu.rs`). The menu bar's items and their attributes live in the window manager and change under the guest (`MM_SETITEMATTR`); the queue is where the guest's own view of the keys is decided.

## Warpine's choices (unverified)

- F10 highlights the first item without dropping its pull-down; `MM_STARTMENUMODE`'s fShowSubmenu drops it; fResumeMenu is ignored.
- Alt's release, not its press, starts menu mode; Alt with any key between is not a toggle.
- Which items the arrows skip; that a typed mnemonic of a disabled item does nothing.
- F1's `WM_HELP` has mp1 = 0 and is posted to the active frame's client.
- Which identities `WM_INITMENU` / `WM_MENUEND` carry (the bar: `FID_MENU`; a pull-down: its bar item's id), and that pull-downs, which are no windows here, give the menu bar's handle.

## Consequences

- **Addendum (2026-11-23):** the client hears the menu bar, from the mouse and the keyboard alike, through owner calls (`MenuInit` / `MenuSelect` / `MenuEnd`): `WM_INITMENU`, `WM_MENUSELECT` — a chosen item's command is posted only on a TRUE reply, as the PM Reference says — and `WM_MENUEND`.

- Keyboard-only programs can reach their menus; `samples/pm_menukey_test` and a golden picture (`pm_menukey`) check it.
- A program that reads its queue with `WinPeekMsg` without removal sees the keys before menu mode takes them only when it removes them; a keystroke is taken once, when it is removed.
- **Tests:** `pm_menubar.rs` unit tests (the keys, the stored and drawn state, F10 / Alt / clicks on a live window manager), the system accelerator test, `gui_pm_menukey_test_*`.
- **Addendum (2026-11-23):** cascades. A pull-down row with a submenu opens it as a cascade (Right, Enter, its mnemonic, a click); its state is a third `window_ulong` slot (`[-3]`, `Cascade`) beside `[-1]` and `[-2]`, shared by the mouse and the keyboard, and `MenuMode` carries the cascade's highlighted row. One level is shown. The cascade is drawn as a second menu picture beside the pull-down in every compositor, placed inside the frame so the frame keeps taking its clicks.
