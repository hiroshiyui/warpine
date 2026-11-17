# ADR-0010: Use SDL3 through the safe sdl3-rs API only

- **Status:** Accepted
- **Date:** 2026-11-16
- **Sources:** commit ae4b065 ("migrate to SDL3 with no unsafe code; play audio on the main thread"); `CLAUDE.md` (Prerequisites); `src/gui/`

## Context

Warpine's display, input and audio go through SDL. SDL2 is in maintenance mode, while SDL3 brings SDL_GPU (planned GPU compositing), audio streams and better input-method APIs. Warpine's SDL2 front end used unsafe code (the `unsafe_textures` feature, a raw byte view, raw user events, a direct `SDL_CaptureMouse` call), and memory safety is the project's top priority after hypervisor isolation.

## Decision

Warpine uses SDL 3 through the safe `sdl3` crate API only (found through pkg-config). `src/gui` and `mmpm.rs` are `#![forbid(unsafe_code)]`.

- SDL3 audio objects are not `Send`, so audio is played by the main thread's loop (`SdlAudio`), and API threads talk to it through the thread-safe `AudioService`.
- What only the GUI thread can do (clipboard, mouse capture, pointer shapes, pointer warping) is requested through `GUIMessage`.

## Consequences

- No `unsafe` in the front end; the compiler enforces it.
- Some SDL features are out of reach until the safe API offers them.
- `libsdl3-dev` is a build requirement.
