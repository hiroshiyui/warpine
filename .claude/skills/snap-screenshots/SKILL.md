---
name: snap-screenshots
description: Capture fresh screenshots of Warpine running the sample OS/2 applications and embed them in README.md as a "Screenshots" section that shows what already works and what is still work in progress.
---

When capturing screenshots for the README, always follow these steps:

1. **Build first** — run `cargo build`. Screenshots must come from the current tree, never from an old binary. If the build fails, stop and report it.

2. **Decide the shot list** — one screenshot per thing worth showing. Start from the default list below, then adjust it to the current state of the project:
   - read `doc/TODOs.md` (open items and the "Completed" table) and the `[Unreleased]` section of `CHANGELOG.md` to see what is finished and what is in progress;
   - add a shot for any newly finished, visible feature that has a sample; drop a shot whose feature was removed;
   - if the user named specific features or samples in the request, those take priority.

   | Name | Sample | Steps before capture | Shows |
   |------|--------|----------------------|-------|
   | `pm_font_test` | `samples/pm_font_test/pm_font_test.exe` | none | IBM Plex typography: families, styles, sizes, CJK text |
   | `pm_shapes_test` | `samples/pm_shapes_test/pm_shapes_test.exe` | none | GPI arcs, patterns, line widths |
   | `pm_controls_typing` | `samples/pm_controls_test/pm_controls_test.exe` | `700 click 230 300;900 text  Hello, Wörld!;1300 key home;1400 text >>;1600 click 200 470;1800 key enter;1900 text three` | Built-in controls after typing (the sample quits about 3 s in, so the default capture at last step + 1.5 s misses it: run the session by hand with `2500 dump`, as `gui_typing_into_controls_matches_golden` does, and trim it like `snap.sh`) |
   | `pm_list_test` | `samples/pm_list_test/pm_list_test.exe` | the `LIST_TEST_STEPS` of `tests/integration.rs` | List selection modes, focus mark, MLE word wrap |
   | `pm_buttons` | `samples/pm_button_test/pm_button_test.exe` | the `BUTTON_TEST_STEPS` of `tests/integration.rs` | Check boxes, radio buttons, a highlighted push button |
   | `pm_dialog_keyboard` | `samples/pm_dialog_test/pm_dialog_test.exe` | `1500 key ctrl+d;2500 text Warp;2800 key tab;3000 key down;3200 key down` | Keyboard-driven modal dialog |
   | `pm_msgbox` | `samples/pm_msgbox_test/pm_msgbox_test.exe` | the `MSGBOX_TEST_STEPS` of `tests/integration.rs` | Message box with an icon over its disabled owner |
   | `pm_minmax` | `samples/pm_minmax_test/pm_minmax_test.exe` | `1500 dblclick 300 42;2000 dblclick 300 8;2500 key alt+f9` | A minimized frame in the icon area, the hide button, activation handed on |
   | `4os2_window` | `samples/4os2/4os2.exe` | `2500 text ver;2700 key enter;3500 text dir /w;3800 key enter` | A VIO program in a window on the PM desktop (needs a pseudo-terminal: capture it as described under step 3 without `WARPINE_VIO`, and trim it like `snap.sh`) |
   | `vio_system_menu` | `samples/4os2/4os2.exe` | `2000 text ver;2200 key enter;3000 drag 675 453 400 300;4000 key alt+space` | The VIO window resized with scroll bars and its system menu open (pseudo-terminal capture, trimmed) |

   Keep the list to what the README can show (about six to eight images): every shot captured in this run is embedded in step 5, so leave out a shot here rather than capture it and not use it.

   Click coordinates are desktop pixels (origin top-left). The default frame is 640×480 with its top-left corner at (32, 32) and a 20-px title bar; if window placement or menu geometry has changed, work out new coordinates from a plain capture before scripting clicks. The script format is documented in `src/gui/gui_script.rs`.

3. **Capture** — for each shot run the helper next to this file, from the repository root:
   ```bash
   .claude/skills/snap-screenshots/snap.sh <name> <sample.exe> "<steps>"
   ```
   It runs the sample on SDL's dummy video driver (no display needed), replays the steps, dumps the desktop, trims the desktop background and writes `doc/screenshots/<name>.png`. Capture independent shots in parallel.
   - If a sample binary is missing, build it (`./vendor/setup_watcom.sh` once, then `make -C samples/<name>`). If it cannot be built, skip that shot and say so in the final report — do not reuse an old image under a new claim.
   - Text-mode (VIO) programs such as 4OS2 open only on a terminal, so `snap.sh` cannot run them; capture them under a pseudo-terminal with the same GUI scripts. By default they run in a window on the PM desktop (dump the desktop and trim it like `snap.sh`); the README's hero image is the full-screen text window (`WARPINE_VIO=fullscreen`), converted without trimming (the frame is a fixed 640 × 400):
     ```bash
     printf '2500 text ver\n2700 key enter\n3500 text dir /w\n3800 key enter\n5500 dump /ABS/PATH/4os2.ppm\n5600 quit\n' > s.script
     SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy RUST_LOG=error WARPINE_VIO=fullscreen WARPINE_GUI_SCRIPT=s.script \
       script -qec "timeout 60 target/debug/warpine samples/4os2/4os2.exe" /dev/null
     magick /ABS/PATH/4os2.ppm -strip doc/screenshots/4os2.png
     ```
     Run it from the repository root. `doc/screenshots/4os2.png` is the README's hero image (outside the screenshot markers). Recapture it only when asked or when text mode changed visibly, and when you do, update that image's alt text.

4. **Look at every image before using it** — open each PNG with the Read tool. Reject and investigate any that is blank, shows only the desktop, is cut off, or does not show what its caption will claim (for example a menu that did not open because a click missed). A screenshot is evidence: the caption must describe what is actually visible.

5. **Update `README.md`** — maintain one section delimited by these exact marker comments, placed after the introductory paragraph and existing hero image and before "Key Features". Replace everything between the markers on each run; create the markers if they are missing; never touch the rest of the README.
   ```markdown
   <!-- screenshots:begin -->
   ## Screenshots

   ### Working today

   | | |
   |---|---|
   | ![alt text](doc/screenshots/NAME.png) | ![alt text](doc/screenshots/NAME.png) |
   | **Short title.** One or two sentences on what the picture shows. | … |

   ### Work in progress

   - **Feature** — what works so far and what is still missing.

   _Screenshots captured from commit `<short hash>` on <YYYY-MM-DD>._
   <!-- screenshots:end -->
   ```
   Rules for the content:
   - **Every screenshot captured in this run is embedded** — a new or updated image is never left out of the README. An image whose name is already in the section replaces that entry (rewrite its alt text and caption to match the new picture); a new name gets a new entry, in Working today or, if it shows a gap, Work in progress.
   - **Working today** holds only features that are visible in a screenshot captured in this run (or the kept text-mode image). Give every image descriptive alt text.
   - **Work in progress** is a short list taken from the open items in `doc/TODOs.md` that a reader would care about. Where a screenshot shows the gap (for example a control that displays text but cannot be edited yet), embed it there with a caption that says plainly what is missing. Do not present unfinished features as done, and do not list long-term roadmap items (SOM/WPS, 64-bit XE) as in progress.
   - Keep it brief: about six to eight images (choose fewer shots in step 2 rather than dropping captured ones) and at most six work-in-progress bullets.
   - Use relative paths so the images render on GitHub. Take the commit hash from `git rev-parse --short HEAD`; if the working tree has uncommitted changes, say "uncommitted changes on top of `<hash>`".

6. **Remove stale images** — delete PNGs in `doc/screenshots/` that the README no longer references, except text-mode images kept under step 3. Check with `grep -o 'doc/screenshots/[^)]*' README.md`.

7. **Report** — list the screenshots captured (each one embedded, new or replacing which entry), any shot that was skipped and why, and the work-in-progress items written. Do not commit or push; leave that to the user (`/commit-and-push`).
