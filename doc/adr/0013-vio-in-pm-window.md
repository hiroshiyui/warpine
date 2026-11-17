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
