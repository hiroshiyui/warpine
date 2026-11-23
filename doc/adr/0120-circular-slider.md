# ADR-0120: The circular slider, with its look and behaviour Warpine's own

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

`WC_CIRCULARSLIDER` (class `0xFFFF0041`, from PMCTLS; `WinRegisterCircularSlider` is PMCTLS ordinal 24 in Open Watcom's `os2386.lib`) was the one standard PM control class Warpine lacked. The public copies of the PM Reference (EDM2) document only its messages: `CSM_SETRANGE` / `CSM_QUERYRANGE`, `CSM_SETVALUE` / `CSM_QUERYVALUE`, `CSM_QUERYRADIUS`, `CSM_SETINCREMENT` / `CSM_QUERYINCREMENT` and `CSM_SETBITMAPDATA` (`CSBITMAPDATA`), each returning TRUE or FALSE. The control's look, its keys, the `CSS_*` styles and the `CSN_*` notifications' parameters are not described; the constants come from Open Watcom's `pmstddlg.h`.

## Decision

1. **A built-in control** (`loader/pm_circslider.rs`), like the slider: its state (`CircState`: range, value, step, tick spacing, held part, the `CSBITMAPDATA` handles) on the window, made on first use; messages answered by `circslider_message`, painted by `paint_circslider`. The class is always available; `WinRegisterCircularSlider` returns TRUE.
2. **The documented messages as documented**, with Warpine's checks: a range whose low end is not below its high end, a step of 0 and a value outside the range are refused (FALSE).
3. **The rest is Warpine's choice, marked unverified** (module documentation, `doc/unverified.md`):
   - the look: the window text under a round dial, the value in its middle, a knob near its rim, tick marks every tick increment around it, a − and a + button at its lower corners;
   - defaults: range 0–100, value 0, a step of 1, a tick every 10;
   - the dial sweeps 270° clockwise from lower left, `CSS_360` a whole turn from the top; `CSS_POINTSELECT` moves the value to a press, without it only a drag turns the dial; `CSS_MIDPOINT` lengthens the middle tick; `CSS_PROPORTIONALTICKS` makes ticks a sixth of the radius; `CSS_CIRCULARVALUE` (with `CSS_360`) wraps steps past an end;
   - keys: Left / Down and Right / Up step, Page Down / Page Up step by the tick increment, Home / End go to the ends;
   - notifications: `WM_CONTROL` with `CSN_CHANGED` (mp2 = the value, a SHORT) after a change by the user or `CSM_SETVALUE`, `CSN_TRACKING` while the dial is dragged, `CSN_SETFOCUS` (mp2 = TRUE / FALSE) when it gains or loses the focus.
4. **Not done:** `CSN_QUERYBACKGROUNDCOLOR` is not sent, the `CSM_SETBITMAPDATA` pictures are kept but not drawn, held buttons do not repeat.

## Consequences

- Programs using a circular slider run, and their dials can be turned; a program that relies on PM's exact layout or on undocumented notification details may differ.
- Tested by unit tests (geometry, steps, angles), `pm_circslider_test` (30 checks, scripted keys, clicks and a drag) and golden pictures in both themes.
