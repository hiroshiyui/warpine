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
   | `pm_controls_test` | `samples/pm_controls_test/pm_controls_test.exe` | none | Built-in controls |
   | `pm_menu_pulldown` | `samples/pm_menu_dlg_test/pm_menu_dlg_test.exe` | `1500 click 52 62` | Menu bar with an open pull-down |
   | `pm_menu_about_dialog` | `samples/pm_menu_dlg_test/pm_menu_dlg_test.exe` | `1500 click 52 62;2500 click 62 80` | Modal dialog |
   | `pm_demo` | `samples/pm_demo/pm_demo.exe` | none | GPI drawing primitives, window chrome |

   Click coordinates are desktop pixels (origin top-left). The default frame is 640×480 with its top-left corner at (32, 32) and a 20-px title bar; if window placement or menu geometry has changed, work out new coordinates from a plain capture before scripting clicks. The script format is documented in `src/gui/gui_script.rs`.

3. **Capture** — for each shot run the helper next to this file, from the repository root:
   ```bash
   .claude/skills/snap-screenshots/snap.sh <name> <sample.exe> "<steps>"
   ```
   It runs the sample on SDL's dummy video driver (no display needed), replays the steps, dumps the desktop, trims the desktop background and writes `doc/screenshots/<name>.png`. Capture independent shots in parallel.
   - If a sample binary is missing, build it (`./vendor/setup_watcom.sh` once, then `make -C samples/<name>`). If it cannot be built, skip that shot and say so in the final report — do not reuse an old image under a new claim.
   - Text-mode (VIO) programs such as 4OS2 and the builtin CMD.EXE use a different renderer that the helper cannot drive. Keep the existing text-mode screenshot in `doc/screenshots/` unless the user supplies a new one; do not delete it.

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
   - **Working today** holds only features that are visible in a screenshot captured in this run (or the kept text-mode image). Give every image descriptive alt text.
   - **Work in progress** is a short list taken from the open items in `doc/TODOs.md` that a reader would care about. Where a screenshot shows the gap (for example a control that displays text but cannot be edited yet), embed it there with a caption that says plainly what is missing. Do not present unfinished features as done, and do not list long-term roadmap items (SOM/WPS, 64-bit XE) as in progress.
   - Keep it brief: at most about six images and six work-in-progress bullets.
   - Use relative paths so the images render on GitHub. Take the commit hash from `git rev-parse --short HEAD`; if the working tree has uncommitted changes, say "uncommitted changes on top of `<hash>`".

6. **Remove stale images** — delete PNGs in `doc/screenshots/` that the README no longer references, except text-mode images kept under step 3. Check with `grep -o 'doc/screenshots/[^)]*' README.md`.

7. **Report** — list the screenshots captured, any shot that was skipped and why, and the work-in-progress items written. Do not commit or push; leave that to the user (`/commit-and-push`).
