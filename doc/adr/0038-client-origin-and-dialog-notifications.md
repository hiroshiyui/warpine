# ADR-0038: Draw client windows at their place in the frame surface; send dialog notifications through call chains

- **Status:** Accepted; extends ADR-0011 and ADR-0026
- **Date:** 2026-11-18
- **Sources:** batch 6; `src/loader/pm_types.rs` (`DrawTarget::Client`, `client_view`, `target_view`), `src/gui/surface.rs` (`DrawOp::translate`), `src/loader/pm_frame.rs`, `src/loader/pm_scrollbar.rs`, `src/loader/pm_stddlg.rs` (`StdSend`, `std_flush`, `ChainThen::StdDialog`), `src/loader/pm_hook.rs` (`CallChain::results`, `ChainEnd::ModalLoop`); `samples/pm_frame_scroll_test`, `samples/pm_modal_filter_test`; ADR-0011, ADR-0026, ADR-0029

## Context

Every window of a frame draws into the frame's one surface (ADR-0011). A client presentation space drew from the surface origin, which was right only while the client sat at the frame's corner. Frame scroll bars move the client up or left, so the client's drawing landed in the wrong place, and the scroll bars themselves could not be clicked.

The standard dialogs (ADR-0029) run their logic on the host, but must ask a subclassing dialog procedure synchronously about filtering, validation and errors. The modal loops also had to call message-filter hooks.

## Decision

- **A client presentation space draws through `DrawTarget::Client(frame)`:** its operations are translated to the client's place in the frame surface and clipped to the client.
  - `GpiQueryPel` and `GpiBitBlt` follow the same origin.
  - A frame without frame controls draws exactly as before.
  - Pointer hits are computed in surface coordinates and include the frame's own controls, which tell the client through `WM_VSCROLL` / `WM_HSCROLL`.
- **Host dialog logic queues its questions:** `FDM_*` / `FNTM_*` become `StdSend`s, and the default dialog procedure flushes them as one call chain (`ChainThen::StdDialog`) before it returns. `CallChain::results` collects every answer, and the dialog acts on them.
- **The modal loop runs `HK_MSGFILTER` hooks** for each message through a call chain (`ChainEnd::ModalLoop`), with `MSGF_DIALOGBOX` or `MSGF_MESSAGEBOX`; a hook returning TRUE swallows the message.

## Consequences

- Programs with frame scroll bars draw correctly and can be scrolled with the mouse. Subclassed standard dialogs and message-filter hooks behave as in PM.
- The per-window origin in the draw path is the base for presentation spaces of other child windows (`doc/TODOs.md`).
- Call chains (ADR-0026) are now the single way host logic waits for guest answers, including inside the modal loop. `MSGF_TRACK` is not produced, because frames are tracked by the GUI thread rather than a guest loop.
