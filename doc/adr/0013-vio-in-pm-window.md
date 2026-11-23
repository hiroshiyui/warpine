# ADR-0013: Run text-mode (VIO) programs in a PM window by default

- **Status:** Accepted
- **Date:** 2026-11-17
- **Sources:** commits 3705715 (plan), 928a428 (VIO window); developer guide (VIO programs in a PM window); `src/gui/vio_window.rs`

## Context

On OS/2 the session type is chosen when a program starts: a window-compatible VIO program can run full screen or in a window on the PM desktop. The program only states what it allows, in its LX module flags. The sources found do not say which type `SSF_TYPE_DEFAULT` gives a window-compatible program.

## Decision

Warpine runs a VIO program in a window on the PM desktop (`#VioWindow` frame), unless `WARPINE_VIO=fullscreen` asks for the full-screen text window or the program's module flags say it is incompatible with PM windowing. Alt+Home switches between the two. Without a display, or with `WARPINE_HEADLESS=1`, a VIO program runs headless.

## Consequences

- Text-mode programs share the desktop with PM programs and get a system menu, scroll bars, Mark / Copy / Paste and resizing.
- The window default is Warpine's choice and is recorded as unverified.

## Addendum (2026-11-23): the rest of the system menu

The public sources show the items of an OS/2 window's system menu (GG24-3732) and describe Mark, Copy, Copy All and Paste (GG24-3731), but not what Help, Font Size, Scroll and Hide do in detail. Warpine gives them its own design rather than leaving them greyed:

- **Font Size** opens a list of cell sizes (`VIO_FONT_SIZES`: 8 × 8, 8 × 12, 8 × 14, 8 × 16, 12 × 24, 16 × 32) as a pop-up, not a dialog. Every size is drawn from the 8 × 16 Unifont glyphs (`render_cells_sized`: a pel shrunk from several is set if any of them is; enlarged, it takes the pel under it). The frame is fitted to the whole screen in the new cells, as after a mode change. The choice lasts for the session.
- **Scroll** is a mode like Mark ("Scrolling: " in the title): the arrows, Page Up / Down, Home / End and Ctrl+Home / Ctrl+End scroll the view instead of reaching the program; Esc or Scroll again leaves it.
- **Keyboard marking.** In Mark mode the same keys move a one-cell mark, starting at the text cursor; with Shift they extend it from its anchor; Enter copies it. A mark dragged with the mouse past the view's edge scrolls the view on, a cell every 60 ms.
- **Hide** hides the frame. The VIO frame has a switch-list entry, so the window list (Ctrl+Esc, or the new Window list item) shows it and brings it back, from this program or from another one on the session's desktop. With the window hidden and nothing else of the program active, Ctrl+Esc still opens the window list.
- **Help** lists the window's keys in a pop-up panel.

All of this is recorded as unverified. The PM frame's system menu enables Window list too: `WinDefWindowProc` has shown the window list since the switch list was added.
