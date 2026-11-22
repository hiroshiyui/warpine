# ADR-0118: Themes, and rounded frames shaped where frames are composited

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

Warpine is an OS/2-API-compatible platform, not a clone (CLAUDE.md, the project goal), so how it draws what it draws itself — frame chrome, built-in controls, menus — is its own to design. Its look so far follows the bevelled style of the 1990s. The user asked for modern design, starting with rounded corners.

Two constraints shape how:

- **Programs must not notice.** Window sizes, client rectangles, `SV_*` metrics, frame insets and hit areas are API; programs lay themselves out from them. A theme may change pels only.
- **Frames are rectangles everywhere they are kept.** Client surfaces, chrome pixmaps and the pictures a process shares with the desktop server (ADR-0052) are rectangular and opaque (`0x00RRGGBB`). Three compositors put them on the screen: the software compositor (the golden pictures' reference), the layered compositor (textures, real displays), and the desktop server, the default, which composites the clients' pictures in a buffer.

Rounded, antialiased corners and a soft shadow need what lies *below* a frame, which only the compositor has.

## Decision

1. **Themes** (`gui/theme.rs`): `classic` (the look so far, the default, the one golden pictures hold) and `modern`, chosen per session with `WARPINE_THEME`. A theme changes pels only. The design values live in one place: `theme.rs` (colours) and `frame_shape.rs` (radius 8, shadow 12 pels moved down 3, darkness 0x58 of 255).
2. **Modern chrome:** a thin edge line instead of the bevelled border (same border width), flat title-bar pictograms in the title's text colour instead of raised buttons; title colours stay the system colours (`WinSetSysColors`).
3. **Rounded frames are shaped where they are composited** (`gui/frame_shape.rs`), from one distance-to-a-rounded-rectangle function, so every compositor draws the same pels:
   - a desktop buffer (software compositor, desktop server): the shadow darkens what is below before the frame is drawn; the pels the corners will cover are kept, the frame is drawn, and they are mixed back by the corners' coverage. A rounded frame hides frames below it only away from its corners; its extent and damage include the shadow;
   - textures (layered compositor): a rounded frame's texture carries its coverage as alpha and is drawn blended; a shadow picture, made once, is drawn in nine stretched slices under it.
4. **The desktop server is told, not configured:** `Item::rounded` (protocol version 11) marks a picture to shape; the server shapes the bounds of its painted parts. The server reads no theme of its own.
5. A maximized frame is drawn square without a shadow; pull-downs and pop-up menus are not shaped yet.

## Alternatives considered

- **Alpha in the shared pixmaps.** Every client surface and upload would have to carry alpha, and the server would blend everything — a larger, costlier change for the same result.
- **Stair-step corners as painted parts** (no alpha, no protocol change). Jagged corners look worse than square ones.
- **Rounding the client area too.** Programs draw into their client's corners; clipping them would change what a program's own pels look like.

## Consequences

- Golden pictures stay byte-identical in the classic theme; the modern theme has its own (`pm_dialog_modern`), and one test checks that the software and layered compositors draw the very pels the desktop server does.
- Pointer hit testing is unchanged: the bare pels of a corner still belong to the frame.
- **Addendum (2026-11-23):** built-in controls and menus. `control_box` draws a rounded box (radius 3, corners left unpainted so what is behind shows: a control cannot read the pels below it); a focused push button gets an accent focus ring, a focused entry field an accent edge, the push button under the pointer a lighter face (`WindowManager::hover_control`). Pull-downs and pop-up menus are rounded and shadowed like frames: the software compositor draws a frame's pull-down after the frame with its own shadow, the layered compositor gives pull-down and pop-up textures alpha, and the desktop server shapes them from `Item::rounded`.
- Next steps (TODOs): scroll bars and sliders; then `modern` as the default, `classic` kept.
