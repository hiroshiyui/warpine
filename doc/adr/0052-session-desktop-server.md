# ADR-0052: One desktop per session: a desktop server process, with Warpine processes as its clients

- **Status:** Accepted (phases A and B implemented: ADR-0053, ADR-0054, ADR-0055; phase C but drag and drop: ADR-0056); amends ADR-0020's "each Warpine process shows its own PM desktop window"; ADR-0030 stayed until phase C
- **Date:** 2026-11-19
- **Sources:**
  - Desktop link and server: `src/desktop/` (`pixmap.rs`, `proto.rs`, `transport.rs`, `model.rs`, `client.rs`), `src/gui/desktop_server.rs`, `src/gui/desktop_events.rs`, `src/gui/sdl_renderer/compositor/remote.rs`
  - Client side of the renderer: `src/gui/sdl_renderer.rs` (`Host`, `pump_remote`), `src/loader/pm_input.rs` (acknowledgements), `src/loader/pm_types.rs` (handle slots)
  - Sample `samples/pm_twoproc_test`
  - Earlier decisions: ADR-0020, ADR-0030, ADR-0032, ADR-0049, ADR-0051
  - IBM *PM Programming Guide*, "Mouse and Keyboard Input" ("System Message Queue", "Capturing Mouse Input")

## Context

Every OS/2 process is a Warpine host process of its own (ADR-0020), and each one that used PM opened its own SDL window with a private desktop. A program started with `WinStartApp` or `DosExecPgm` appeared in a second desktop window. Window handles, Z-order, activation and focus stopped at the process boundary, and so did the switch list, the system atom table and DDE (ADR-0030). The Workplace Shell, which starts programs and must share a desktop with them, cannot be built that way.

## Decision

### Desktop server process

The desktop of a session belongs to a separate process: the same binary run as `warpine --desktop-server <session dir>`. It runs no guest code and uses no KVM.

- **Start-up.** The first PM process of a session starts it (`DesktopLink::open`). It does so under `desktop.lock` in the session directory (`WARPINE_THREAD_DIR`, mode 0700, ADR-0032).
  - The lock is an exclusive `flock`, valid only while the path still names the locked inode.
  - A client holds the lock through connect and handshake. A server about to stop takes it too, so a starting client cannot be left connecting to a server that is going away.
- **Clients.** Every PM process of the session connects as a client.
  - `WARPINE_DESKTOP=local` keeps the desktop in the process, as before. So does a session without a shared directory, or a server that cannot be reached.
  - Headless runs and unit tests have no server.
- **Server output.** The server's standard handles go to `/dev/null`. Its log goes to `WARPINE_DESKTOP_LOG` if set.

### Who draws what

- **Each program draws its own windows.** That includes the title bars, borders, buttons and menu bars: the renderer's existing chrome, so frame tracking, system menus and pop-up menus also stay in the program's GUI thread.
  - This is closer to PM than the server-side decoration of the draft: PM frame controls are windows of the program.
  - It also keeps the server small, which is its attack surface.
- **The server** owns the desktop window and its compositor, the host pointer, mouse capture and warps, the input-method area and the host clipboard.
- **The renderer's two hosts.** `SdlRenderer` shows its desktop through a `Host`:
  - `Local`: its own SDL window, as before;
  - `Remote`: a `RemoteDesktop`.
  - Everything that decides what to show — scenes, chrome, damage, input handling — is shared by both.

### Frame pictures in shared memory

- **Pixmaps.** `RemoteDesktop` renders each visible frame into a `SharedPixmap`, a memfd sealed against resizing, with the same `draw_placed_frame` and partial updates as the layered compositor. Each open pull-down and the pop-up menu get pixmaps of their own.
- **Passing them over.** A new pixmap's memfd goes to the server once (`NewPixmap` with `SCM_RIGHTS`).
- **What is sent.** An `Update` carries:
  - the pictures back to front, with their places and painted parts;
  - the damaged pixmap areas;
  - the client's active frame;
  - the frames it raised.
- **No pixel copies.** Pels never travel through the socket.
- **On the server.** It maps each pixmap read-only after checking the seals and the exact size. Its `PixmapCompositor` is the software compositor fed from pixmaps, so the server's desktop matches the in-process one pel for pel: the golden-image tests pass either way.
- **Where `unsafe` lives.** `desktop::pixmap` holds it, behind bounds-checked row copies. `src/gui` stays `#![forbid(unsafe_code)]`.

### Protocol and its security

- **Transport.** A `SOCK_SEQPACKET` Unix socket, `desktop.sock` in the session directory, mode 0600.
  - It is reached through `/proc/self/fd/<dir>/desktop.sock`, so a deep session directory does not hit the 108-byte socket-path limit.
  - The server accepts only peers of its own uid (`SO_PEERCRED`).
- **Messages.** One message per packet, at most 128 KB, little-endian, encoded by hand.
- **Strict decoding.** Every count, length, coordinate (±2^24) and enumerated value is checked. A short or extended packet, an unknown tag, invalid UTF-8 or an unexpected descriptor is an error.
- **Hostile clients.** The server treats every client as hostile:
  - a client may show and raise only frames of its own slot;
  - pixmaps are limited to 1024 per client;
  - a malformed message ends that client's connection only.
- **The other direction.** The client decodes the server's messages just as strictly. A bad message or a lost connection ends its PM program, as closing the desktop does.

### Input across programs

- **One event at a time.** The server holds the session's input queue and hands out one event at a time:
  - a pointer event goes to the client whose picture is topmost under the pointer, to a client holding a pressed button, or to one that asked for capture;
  - a key or typed text goes to the client of the active frame;
  - an open pop-up menu's client gets everything.
- **Acknowledgement when read.** The client handles the event as its own window's event, and the input events it queues carry the server's token (`WindowManager::input_tag`). The server hears `Ack` once the program has read the last of them (`pop_input`), or at once if the GUI thread used the event itself.
- **PM's strict order across programs.** Only after the `Ack` does the server route the next event. So a key typed after a click goes to the program the click activated (ADR-0051, now across processes).
- **Moves.** Pointer moves waiting in the server are merged.
- **Drags.** A drag (`DrgDrag`) takes over the pointer input still queued, as its loop is fed by the drag channel.
- **Stalled programs.** A client that does not read an event within `INPUT_STALL` (5 s, Warpine's choice) is marked unresponsive, and input goes past it until it answers again.

### Activation and stacking

- **Activation.** Each client reports its active frame with every `Update` and `Ack`. A report that changes to a frame makes it the session's active window. The client that held activation before gets `Deactivate`: it clears its active frame, posts `WM_ACTIVATE` FALSE and redraws the title bar.
  - Unchanged reports are ignored, so a client that has not seen its `Deactivate` yet cannot take activation back.
- **Stacking.** Frames of all clients are stacked in one order (`DesktopModel`).
  - A client's frames keep its own Z-order in the slots they hold.
  - A frame it raised (`WindowManager::raised_frames`) goes to the top of the desktop, and a new frame appears on top.

### Window handles

- **Slots.** The server gives each client the lowest free slot.
  - The client's window handles become `(slot + 1) << 20 | local`: up to 1023 slots, with local numbers reused within the slot's range once free.
  - The reserved handles (`HWND_DESKTOP` …) stay below `1 << 20`.
- **Uniqueness.** Handles are therefore unique in the session and name their process. Calls on another program's windows are phase B.

### Lifetime and failure

- **Ending.** The server ends:
  - when the desktop window is closed or a GUI script quits: clients are sent `Quit` and have 5 s to end;
  - when no client connected within 10 s of its start;
  - `DESKTOP_LINGER` (2 s) after its last client left, so a parent that ends right after starting a child does not take the desktop down.
- **Cleanup.** When it stops, the server removes its socket and lock file and tries to remove the session directory, which is empty once the session's last process left.
- **A crashed server** ends the session's PM programs, much as X clients end with their X server. Reconnecting is not attempted: every window and its state would have to be replayed.

### GUI scripts

- **Where they run.** A scripted session (`WARPINE_GUI_SCRIPT`) runs in the server, which inherits the environment.
- **Captures.** A `dump` waits until queued input has been read (at most 3 s), then asks every client to send what it shows (`Flush` / `FlushDone`) before capturing.
- **Clients.** A client of a scripted session blinks no caret, runs no drag-scroll timer and warps no pointer, as in-process scripted sessions do.

### Scope of phase A

- **What is shared.** PM programs share the desktop; `WinStartApp` and `DosExecPgm` children appear on the same desktop.
- **What stays local.** Text-mode programs keep their own window: the VIO window, and full screen with Alt+Home, which depends on owning the display.
- **Layered compositor.** It remains for in-process desktops; the server composites in software.

### Phases

| Phase | Scope |
|---|---|
| A (done) | The server and the split: frames of every PM process on one desktop, input in order across programs, activation and stacking across programs |
| B (done) | Calls on other programs' windows: `WinPostMsg`, `WinQueryWindowProcess`, `WinIsWindow`, queries on other programs' windows, Z-order across programs (ADR-0053), synchronous `WinSendMsg` (ADR-0054), window lists, broadcasts, `WM_MINMAXFRAME` and send hooks across programs (ADR-0055) |
| C (done but drag and drop) | Shared shell services: one switch list and Ctrl+Esc window list, the system atom table, DDE between programs (ADR-0056, superseding ADR-0030); drag and drop between programs still open |

## Consequences

- **What works.** PM programs of a session share one desktop:
  - keys and clicks reach them in the order typed;
  - activation and stacking work between them;
  - each program's window handles are unique in the session.
- **Proof.** `pm_twoproc_test` shows two programs with input in order across them. Every golden-image test passes through the server.
- **Costs:**
  - Every PM run gains a second host process.
  - Each input event costs a round trip, and a program reads it before the next is routed.
  - Each composite costs one update message, and changed pels are copied once into the pixmap.
- **New failure modes:**
  - a program that does not read its input delays everyone's input for up to 5 s;
  - a server crash ends the session's PM programs.
- **Not yet across programs:** drags, the clipboard as PM data (the host clipboard is shared), DDE, atoms, the switch list and cross-process window calls.

### Unverified

- **Input stalls on OS/2.** How OS/2 itself handled a program that stopped reading input: later fix packs are said to have added a timeout; no public source was found.
- **Handle format.** The format of PM window handles is not known from public sources; Warpine's slot scheme is its own.
