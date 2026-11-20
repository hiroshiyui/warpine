# ADR-0089: WinTrackRect: a tracking loop on the calling thread

- **Status:** Accepted
- **Date:** 2026-11-20
- **Sources:**
  - IBM's *PM Programming Reference*: `WinTrackRect`, `WinShowTrackRect`, `TRACKINFO`, `WM_TRACKFRAME`, `WM_QUERYTRACKINFO`.
  - Open Watcom's `h/os2/pmwin.h`: the prototypes, `TRACKINFO` and the `TF_*` flags.
  - `doc/os2_ordinals.md`: PMWIN 890 `WinTrackRect`, 882 `WinShowTrackRect`.
  - The code:
    - `src/gui/frame_track.rs`: `KeyTrack`, keyboard moves and sizes of frames on the GUI thread;
    - `src/loader/pm_drag.rs`: `DrgDrag`'s loop on the calling thread (ADR-0045, ADR-0057);
    - `pm_remote.rs`: `run_sent` / `AfterSent`;
    - `pm_input.rs`: pointer capture;
    - `pm_win.rs`: `next_message`.
  - ADR-0086, which deferred these calls.

## Context

`WinTrackRect(hwnd, hps, pti)` lets the user move or size a rectangle with the pointer or the keyboard, and returns when the user accepts (`TRUE`) or cancels (`FALSE`).

`TRACKINFO` gives:
- the outline's border widths (`cxBorder` / `cyBorder`);
- a grid (`cxGrid` / `cyGrid`, with `TF_GRID`);
- the keyboard step (`cxKeyboard` / `cyKeyboard`);
- the rectangle, which is also the result (`rclTrack`);
- a boundary (`rclBoundary`, with `TF_ALLINBOUNDARY` / `TF_PARTINBOUNDARY`);
- a minimum and a maximum size;
- which edges move (`TF_LEFT` / `TF_TOP` / `TF_RIGHT` / `TF_BOTTOM`; `TF_MOVE` moves all four).

The call draws the outline on `hps`. `WinShowTrackRect(hwnd, fShow)` hides and shows the outline, so that a program can paint while tracking. Programs use it for:
- rubber-band selection in drawing programs;
- splitter bars;
- their own frame-like windows;
- PM's frame, which tracks with it when it answers `WM_TRACKFRAME`, after asking itself `WM_QUERYTRACKINFO` for the limits.

Warpine has none of this. Frames move and size live on the GUI thread (`frame_track.rs` for the keyboard, the renderer's drags for the pointer), with no `WM_TRACKFRAME` or `WM_QUERYTRACKINFO`.

The call blocks the calling thread, which is a PM thread with a message queue, until the tracking ends. Meanwhile:
- other programs may send that thread messages, which run its window procedures;
- pointer input may wait for a `WM_HITTEST` answer;
- the tracking needs the pointer even outside the window, and the keyboard.

## Options

1. **A loop on the calling thread fed by the thread's own queue (recommended).**
   - `WinTrackRect` captures the pointer for `hwnd` (as `WinSetCapture`), so moves and button releases anywhere reach the thread's queue through the routing that exists today (`pm_input.rs`), on the local desktop and the session's desktop server alike.
   - It loops on `next_message` and takes only pointer and keyboard input:
     - `WM_MOUSEMOVE` moves the edges;
     - releasing the button that is down accepts, and so does Enter;
     - Esc cancels;
     - the arrows step by `cxKeyboard` / `cyKeyboard`.
   - Other queued messages stay in the queue until the tracking ends.
   - Sent messages and `WM_HITTEST` asks run through `run_sent` / `hit_test` with a new `AfterSent::Track`, then the loop goes on, as `DrgDrag` does.
   - **The outline** is drawn on `hps` (on `hwnd` when `hps` is NULLHANDLE) with `Mix::Invert`, as `WinInvertRect` draws it now; drawing it twice removes it. `WinShowTrackRect` keeps a hide count per thread.
   - **The arithmetic** is one pure state machine shared with the frames' keyboard tracking, in a new `src/loader/track.rs`, to which `frame_track.rs`'s `KeyTrack` moves: it applies pointer moves to the moving edges, keyboard steps, the grid, the minimum and maximum size, and the boundary.
   - No change to the GUI thread or the desktop protocol.
2. **A GUI-thread feed, as `DrgDrag` takes its input.**
   - The GUI thread diverts pointer and key input into a tracking feed while a tracking runs (`DragFeed`'s pattern), and the API thread waits on it.
   - It works without capture, but it needs the feed in both desktop modes and changes to the renderer's input path, for no gain over capture, which the routing already honours.
3. **Track on the GUI thread, as frames do now, and block the API thread until it is done.**
   - The renderer draws the outline as an overlay above every surface. That is clean, with no XOR, and it can cross windows.
   - But the program's `hps` is never drawn on, `WinShowTrackRect` means nothing, and under the session's desktop server the overlay needs a protocol message.
   - It also differs most from what programs see on OS/2.

## Scope

**Phase 1**
- `WinTrackRect` and `WinShowTrackRect` for programs, as in option 1.
- `KeyTrack` moves to `track.rs`; the frames' own keyboard tracking keeps working through it.

**Phase 2: frames**
- `WM_TRACKFRAME` and `WM_QUERYTRACKINFO` for frames a program subclasses: Warpine sends them, and a frame procedure that handles `WM_TRACKFRAME` or changes the limits in `WM_QUERYTRACKINFO` takes over. Standard, unsubclassed frames keep their live drags on the GUI thread.
- The pointer taking over a keyboard tracking (TODOs).

## Decision

Option 1, phase 1 now; phase 2 later. With `hwnd` = `HWND_DESKTOP` and no `hps`, the tracking runs without an outline.

1. **`src/loader/track.rs`** (pure):
   - `KeyTrack`, moved from `gui/frame_track.rs`, which re-exports it;
   - `Tracker`: pointer moves and arrow steps change one offset from the start, which is applied to the moving edges, snapped to the grid, held to the size limits and kept in the boundary;
   - `TrackInfo`: the `TRACKINFO` as read.
2. **`src/loader/pm_track.rs`**: `WinTrackRect` (PMWIN 890) and `WinShowTrackRect` (882), from `pm_winutil.rs`'s dispatcher.
   - **Session.** `WindowManager::tracks` holds one session per vCPU. It is resumed when the call is made again after a message the thread ran (`AfterSent::Rerun`), and known by the call's return address and `TRACKINFO` pointer; a second, nested tracking fails.
   - **Input.** `Loader::take_message`, the part of `next_message` that returns the message itself, gives the input `WM_MOUSEMOVE` .. `WM_CHAR`. Other messages wait.
   - **Capture.** It goes to `hwnd`, or for the desktop to the thread's focus window, and back at the end.
   - **The outline** is four border strips that do not overlap, inverted on the PS.
   - **Cleanup.** A thread that ends while tracking gives the capture back (`end_thread`).

## Warpine's choices (unverified)

- **Messages:** other queued messages wait, not dispatched; `WM_QUIT` cancels and stays queued.
- **A nested tracking** fails with `PMERR_INVALID_PARAMETERS`; so does an unreadable `TRACKINFO`. A tracking in the desktop with no focus window of the thread fails with `PMERR_INVALID_HWND`.
- **Results:** `rclTrack` holds the starting rectangle after a cancel.
- **Ending a pointer tracking:** the release of the button down when the call began; with none, the release after the next press (any release first).
- **`TF_SETPOINTERPOS`** puts the pointer in the middle of a moving rectangle, else on the middle of the moving edges.
- **The arithmetic:**
  - the grid snaps the offset, towards 0;
  - a maximum size of 0 sets no limit;
  - a border of 0 is drawn 1 pel wide;
  - a keyboard step of 0 is 1;
  - `TF_PARTINBOUNDARY` keeps one pel of a moving rectangle inside;
  - `TF_STANDARD` and `TF_VALIDATETRACKRECT` are ignored.
- **`WinShowTrackRect`:** counted hides; FALSE (`PMERR_INVALID_HWND`) when no tracking runs in `hwnd`.
- **No outline** on the desktop without a PS.

## Consequences

- **Programs** that track rubber bands, splitters or their own frames run.
- **Frames' keyboard tracking** shares `KeyTrack` from `track.rs`, unchanged.
- **Coverage:** two more PMWIN entry points, 881 in all.
- **Left for later (TODOs):**
  - phase 2: `WM_TRACKFRAME` / `WM_QUERYTRACKINFO` for subclassed frames, and the pointer taking over a keyboard tracking;
  - an outline on the desktop.
- **Tests:**
  - `track.rs`: `TRACKINFO` parsing; moves, single edges, size limits, the grid, boundaries, the pointer start; extreme values; the moved `KeyTrack` tests.
  - `pm_track.rs`:
    - the outline strips;
    - the loop: the pointer and a release; keys and Enter, with other messages staying queued; Esc; `WM_QUIT`;
    - a nested call refused and the same call resumed;
    - `WinShowTrackRect`'s count;
    - an unreadable `TRACKINFO`.
  - `tests/integration.rs` `gui_pm_track_test_tracks_with_the_pointer_and_the_keyboard`: `pm_track_test`, 9 checks under a GUI script.
