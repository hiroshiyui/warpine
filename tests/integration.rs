// SPDX-License-Identifier: GPL-3.0-only
//
// End-to-end integration tests: run OS/2 sample binaries, capture stdout
// and exit code, and assert against known-good output.
//
// Requires /dev/kvm.  Tests are skipped (pass silently) when KVM is absent,
// so the suite stays green in restricted CI environments.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Return the path to the compiled warpine binary.  Cargo sets
/// CARGO_BIN_EXE_warpine automatically when building integration tests.
fn warpine() -> &'static str {
    env!("CARGO_BIN_EXE_warpine")
}

/// Return `true` if KVM is accessible on this machine.
fn kvm_available() -> bool {
    Path::new("/dev/kvm").exists()
}

/// Run `sample` (path relative to workspace root) with WARPINE_HEADLESS=1,
/// wait up to `timeout` seconds, and return `(stdout, exit_code)`.
///
/// Panics if the process cannot be spawned or times out.
fn run_sample(sample: &str, timeout_secs: u64) -> (String, i32) {
    let mut child = Command::new(warpine())
        .arg(sample)
        .env("WARPINE_HEADLESS", "1")
        // Suppress tracing/log noise on stderr; we only care about stdout.
        .env("RUST_LOG", "error")
        // End of input at once: a sample that waits for a key (audio_test's
        // "Press ENTER") must not block on the test runner's own stdin.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn warpine for {sample}: {e}"));

    // Poll until the process exits or the deadline passes.
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => {
                let out = child.stdout.take().unwrap();
                use std::io::Read;
                let mut buf = String::new();
                // stdout handle is a pipe; read it to EOF
                std::io::BufReader::new(out).read_to_string(&mut buf).ok();
                let code = status.code().unwrap_or(-1);
                return (buf, code);
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("{sample} did not complete within {timeout_secs}s");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// hello: DosWrite to stdout, clean exit.
#[test]
fn test_hello_world() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/hello/hello.exe", 15);
    assert_eq!(code, 0, "hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from Warpine OS/2 Environment!"),
        "unexpected output:\n{out}"
    );
}

/// alloc_test: DosAllocMem / DosFreeMem round-trip.
#[test]
fn test_alloc_test() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/alloc_test/alloc_test.exe", 15);
    assert_eq!(code, 0, "alloc_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Alloc succeeded"),        "missing 'Alloc succeeded':\n{out}");
    assert!(out.contains("Data in allocated memory"), "missing data check:\n{out}");
    assert!(out.contains("Free succeeded"),          "missing 'Free succeeded':\n{out}");
}

/// nls_test: DosQueryCp, DosQueryCtryInfo, DosGetDateTime, DosMapCase.
#[test]
fn test_nls() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/nls_test/nls_test.exe", 15);
    assert_eq!(code, 0, "nls_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("All tests PASSED!"), "NLS tests failed:\n{out}");
    // Spot-check individual API results
    assert!(out.contains("DosQueryCp returns 0 OK"),       "DosQueryCp:\n{out}");
    assert!(out.contains("DosQueryCtryInfo returns 0 OK"), "DosQueryCtryInfo:\n{out}");
    assert!(out.contains("DosMapCase converts to HELLO OK"), "DosMapCase:\n{out}");
}

/// audio_test: DosBeep and mciSendString through MDM loaded at run time
/// (`open waveaudio alias …` opens the device by name). Runs headless, so
/// no sound is played and DosBeep returns at once.
#[test]
fn test_audio() {
    if !kvm_available() || !Path::new("samples/audio_test/audio_test.exe").exists() { return; }
    let (out, code) = run_sample("samples/audio_test/audio_test.exe", 15);
    assert!(out.contains("Failed: 0"), "audio_test failed (exit {code}):\n{out}");
}

/// thread_test: DosCreateThread / DosWaitThread.
#[test]
fn test_thread() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/thread_test/thread_test.exe", 15);
    assert_eq!(code, 0, "thread_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Hello from child thread!"), "thread output:\n{out}");
}

/// pipe_test: DosCreatePipe, DosWrite, DosRead.
#[test]
fn test_pipe() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/pipe_test/pipe_test.exe", 15);
    assert_eq!(code, 0, "pipe_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("DosCreatePipe rc=0"), "pipe create:\n{out}");
    assert!(out.contains("Read from pipe: 'Pipe Test Data'"), "pipe read:\n{out}");
}

/// mutex_test: DosCreateMutexSem, recursive locking, inter-thread handoff.
#[test]
fn test_mutex() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/mutex_test/mutex_test.exe", 15);
    assert_eq!(code, 0, "mutex_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Recursive locks OK"), "recursive lock:\n{out}");
    assert!(out.contains("Child: Got mutex!"),  "child lock:\n{out}");
}

/// queue_test: DosCreateQueue, DosWriteQueue, DosReadQueue.
#[test]
fn test_queue() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/queue_test/queue_test.exe", 15);
    assert_eq!(code, 0, "queue_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("DosCreateQueue rc=0"), "queue create:\n{out}");
    assert!(out.contains("DosReadQueue rc=0"),   "queue read:\n{out}");
    assert!(out.contains("msg='Queue Message'"), "queue message:\n{out}");
}

/// ne_hello: 16-bit NE format DosWrite + DosExit — pure assembly, no CRT.
#[test]
fn test_ne_hello() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/ne_hello/ne_hello.exe", 15);
    assert_eq!(code, 0, "ne_hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from NE (16-bit OS/2)!"),
        "unexpected output:\n{out}"
    );
}

/// thunk_test: TIB/PIB layout, DosGetInfoBlocks, DosQuerySysInfo.
#[test]
fn test_thunk() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/thunk_test/thunk_test.exe", 15);
    assert_eq!(code, 0, "thunk_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("All tests PASSED!"), "thunk tests failed:\n{out}");
    assert!(out.contains("DosGetInfoBlocks returns 0 OK"), "DosGetInfoBlocks:\n{out}");
    assert!(out.contains("DosQuerySysInfo returns 0 OK"),  "DosQuerySysInfo:\n{out}");
}

/// rust_hello: Rust guest binary built with the warpine-os2 crate family.
///
/// Skipped gracefully when:
/// - KVM is unavailable
/// - `lx-link` has not been built yet (`target/debug/lx-link` absent)
/// - The nightly build of `rust_hello` fails (toolchain or dependency issue)
#[test]
fn test_rust_hello() {
    if !kvm_available() { return; }

    let workspace_root = env!("CARGO_MANIFEST_DIR");

    // lx-link must be pre-built; skip rather than fail if it is missing.
    let lx_link = format!("{workspace_root}/target/debug/lx-link");
    if !Path::new(&lx_link).exists() {
        return;
    }

    // Build rust_hello with the nightly Rust guest toolchain.
    let target_json = format!("{workspace_root}/targets/i686-warpine-os2.json");
    let rust_hello_dir = format!("{workspace_root}/samples/rust_hello");

    // Prepend the debug directory to PATH so lx-link is found at link time.
    let debug_dir = format!("{workspace_root}/target/debug");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{debug_dir}:{existing_path}");

    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "-Z", "build-std=core,alloc",
            "-Z", "build-std-features=compiler-builtins-mem",
            "-Z", "json-target-spec",
            "--target", &target_json,
        ])
        .current_dir(&rust_hello_dir)
        .env("PATH", &new_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // If nightly is unavailable or the build fails for any reason, skip.
    let ok = match status {
        Ok(s) => s.success(),
        Err(_) => false,
    };
    if !ok {
        return;
    }

    let exe = "samples/rust_hello/target/i686-warpine-os2/debug/rust_hello.exe";
    let (out, code) = run_sample(exe, 15);
    assert_eq!(code, 0, "rust_hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from Rust on Warpine!"),
        "unexpected output:\n{out}"
    );
}

// ── GUI regression tests ─────────────────────────────────────────────────────
//
// These drive a PM sample with a scripted session (`WARPINE_GUI_SCRIPT`, see
// `src/gui/gui_script.rs`) on SDL's dummy video driver, so no display is
// needed, and compare the dumped desktop against a stored reference.
//
// The reference is not an image but a *tile signature*: the average colour of
// every 16×16-pixel tile. It is a few kilobytes of text, survives 1-pixel
// anti-aliasing differences, and a mismatch names the tiles that changed, so
// a failure says where on the desktop to look.
//
// After an intentional rendering change, regenerate the references with
//   WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_
// and review the diff of `tests/golden/*.sig`.

const TILE: usize = 16;
/// Largest per-channel difference of a tile average that still counts as equal.
const TILE_TOLERANCE: i32 = 6;

/// Run `sample` under a GUI script and return the desktop it dumps.
/// `steps` are script lines; `{dump}` is replaced by the capture path.
/// Returns `(width, height, rgb bytes)`.
fn run_gui_sample(sample: &str, steps: &str) -> (usize, usize, Vec<u8>) {
    run_gui_sample_dumps(sample, steps, 1).remove(0)
}

/// Like [`run_gui_sample`] for a script that captures the desktop `count`
/// times: `{dump0}`, `{dump1}`, … name the capture paths (`{dump}` is
/// `{dump0}`), and the captures are returned in that order.
fn run_gui_sample_dumps(sample: &str, steps: &str, count: usize) -> Vec<(usize, usize, Vec<u8>)> {
    let dir = tempfile::tempdir().expect("tempdir");
    let dumps: Vec<_> = (0..count).map(|i| dir.path().join(format!("desktop{i}.ppm"))).collect();
    let script = dir.path().join("session.script");
    let mut text = steps.replace("{dump}", "{dump0}");
    for (i, dump) in dumps.iter().enumerate() {
        text = text.replace(&format!("{{dump{i}}}"), dump.to_str().unwrap());
    }
    std::fs::write(&script, text).unwrap();
    run_gui_script(sample, &script, false);

    dumps.iter().map(|dump| {
        let ppm = std::fs::read(dump)
            .unwrap_or_else(|e| panic!("{sample} wrote no desktop dump {}: {e}", dump.display()));
        parse_ppm(&ppm)
    }).collect()
}

/// Run `sample` on SDL's dummy driver under the GUI script file `script` and
/// wait for it to exit (30 s at most). Returns its stdout when `capture` is
/// set, an empty string otherwise.
fn run_gui_script(sample: &str, script: &Path, capture: bool) -> String {
    let mut child = Command::new(warpine())
        .arg(sample)
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("WARPINE_GUI_SCRIPT", script)
        .env_remove("WARPINE_HEADLESS")
        .env_remove("WARPINE_DESKTOP_W")
        .env_remove("WARPINE_DESKTOP_H")
        .env("RUST_LOG", "error")
        .stdout(if capture { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn warpine for {sample}: {e}"));
    // Read stdout on a thread so a chatty sample cannot fill the pipe and block.
    let reader = child.stdout.take().map(|out| std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        std::io::BufReader::new(out).read_to_string(&mut buf).ok();
        buf
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while child.try_wait().expect("try_wait failed").is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{sample} did not finish its GUI script within 30s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    reader.map(|r| r.join().unwrap()).unwrap_or_default()
}

/// Colour of desktop pixel `(x, y)` (origin top-left) as `0xRRGGBB`.
fn gui_pixel(desktop: &(usize, usize, Vec<u8>), x: usize, y: usize) -> u32 {
    let (w, h, rgb) = desktop;
    assert!(x < *w && y < *h, "pixel ({x}, {y}) is outside the {w}x{h} desktop");
    let i = (y * w + x) * 3;
    (rgb[i] as u32) << 16 | (rgb[i + 1] as u32) << 8 | rgb[i + 2] as u32
}

/// Decode a binary PPM (P6, maxval 255) into `(width, height, rgb bytes)`.
fn parse_ppm(data: &[u8]) -> (usize, usize, Vec<u8>) {
    // Header: "P6\n<w> <h>\n255\n" — three newline-terminated lines.
    let mut pos = 0;
    let mut line = || {
        let end = pos + data[pos..].iter().position(|&b| b == b'\n').expect("truncated PPM header");
        let text = std::str::from_utf8(&data[pos..end]).expect("PPM header is not ASCII");
        pos = end + 1;
        text
    };
    assert_eq!(line(), "P6", "not a binary PPM");
    let dims: Vec<usize> = line().split_whitespace().map(|n| n.parse().unwrap()).collect();
    assert_eq!(line(), "255");
    let (w, h) = (dims[0], dims[1]);
    assert_eq!(data.len() - pos, w * h * 3, "PPM pixel data has the wrong length");
    (w, h, data[pos..].to_vec())
}

/// Average colour of each `TILE`×`TILE` tile, row by row.
fn tile_signature(w: usize, h: usize, rgb: &[u8]) -> Vec<Vec<[u8; 3]>> {
    (0..h.div_ceil(TILE)).map(|ty| {
        (0..w.div_ceil(TILE)).map(|tx| {
            let (mut sum, mut n) = ([0u32; 3], 0u32);
            for y in ty * TILE..((ty + 1) * TILE).min(h) {
                for x in tx * TILE..((tx + 1) * TILE).min(w) {
                    for c in 0..3 { sum[c] += rgb[(y * w + x) * 3 + c] as u32; }
                    n += 1;
                }
            }
            [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
        }).collect()
    }).collect()
}

fn signature_to_text(w: usize, h: usize, sig: &[Vec<[u8; 3]>]) -> String {
    let mut out = format!("{w} {h} {TILE}\n");
    for row in sig {
        for t in row { out.push_str(&format!("{:02x}{:02x}{:02x}", t[0], t[1], t[2])); }
        out.push('\n');
    }
    out
}

fn signature_from_text(text: &str) -> (usize, usize, Vec<Vec<[u8; 3]>>) {
    let mut lines = text.lines();
    let head: Vec<usize> = lines.next().unwrap_or("").split_whitespace()
        .map(|n| n.parse().expect("bad golden header")).collect();
    assert_eq!(head.get(2), Some(&TILE), "golden file uses a different tile size");
    let rows = lines.map(|l| {
        l.as_bytes().chunks(6).map(|c| {
            let v = u32::from_str_radix(std::str::from_utf8(c).unwrap(), 16).expect("bad golden tile");
            [(v >> 16) as u8, (v >> 8) as u8, v as u8]
        }).collect()
    }).collect();
    (head[0], head[1], rows)
}

/// Tiles whose averages differ by more than the tolerance, as `(tile x, tile y)`.
fn differing_tiles(a: &[Vec<[u8; 3]>], b: &[Vec<[u8; 3]>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (ty, (ra, rb)) in a.iter().zip(b).enumerate() {
        for (tx, (ta, tb)) in ra.iter().zip(rb).enumerate() {
            if (0..3).any(|c| (ta[c] as i32 - tb[c] as i32).abs() > TILE_TOLERANCE) {
                out.push((tx, ty));
            }
        }
    }
    out
}

/// Run `sample` under `steps` and compare its desktop with `tests/golden/<name>.sig`.
/// Skipped when KVM or the (git-ignored, Open Watcom-built) sample is missing.
fn check_gui_golden(name: &str, sample: &str, steps: &str) {
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping {name}: KVM or {sample} not available");
        return;
    }
    let (w, h, rgb) = run_gui_sample(sample, steps);
    let sig = tile_signature(w, h, &rgb);
    let golden_path = Path::new("tests/golden").join(format!("{name}.sig"));

    if std::env::var_os("WARPINE_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all("tests/golden").unwrap();
        std::fs::write(&golden_path, signature_to_text(w, h, &sig)).unwrap();
        eprintln!("updated {}", golden_path.display());
        return;
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| panic!(
        "missing {}; create it with WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_",
        golden_path.display()));
    let (gw, gh, gsig) = signature_from_text(&golden);
    assert_eq!((w, h), (gw, gh), "{name}: desktop size changed");
    let diff = differing_tiles(&sig, &gsig);
    assert!(diff.is_empty(),
        "{name}: {} of {} tiles differ from {} (first, as tile x,y of {TILE}px tiles: {:?}). \
         If the change is intended, regenerate with WARPINE_UPDATE_GOLDEN=1.",
        diff.len(), sig.len() * sig[0].len(), golden_path.display(),
        &diff[..diff.len().min(12)]);
}

/// The tile comparison itself: identical signatures match, a visible change
/// is reported at the right tile, and sub-tolerance noise is ignored.
#[test]
fn gui_tile_signature_detects_local_changes() {
    let (w, h) = (64usize, 48usize);
    let base = vec![200u8; w * h * 3];
    let sig = tile_signature(w, h, &base);
    assert_eq!((sig.len(), sig[0].len()), (3, 4));
    let text = signature_to_text(w, h, &sig);
    let (rw, rh, parsed) = signature_from_text(&text);
    assert_eq!((rw, rh), (w, h));
    assert!(differing_tiles(&sig, &parsed).is_empty());

    // Blacken an 8×8 block inside tile (2, 1): a quarter of the tile.
    let mut changed = base.clone();
    for y in 20..28 { for x in 36..44 { for c in 0..3 { changed[(y * w + x) * 3 + c] = 0; } } }
    assert_eq!(differing_tiles(&tile_signature(w, h, &changed), &sig), vec![(2, 1)]);

    // One slightly different pixel is below the tolerance.
    let mut noisy = base;
    noisy[0] = 190;
    assert!(differing_tiles(&tile_signature(w, h, &noisy), &sig).is_empty());
}

/// Fonts: every Plex family, style and size plus CJK text in four codepages.
#[test]
fn gui_pm_font_test_matches_golden() {
    check_gui_golden("pm_font_test", "samples/pm_font_test/pm_font_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// Built-in controls stay visible after the client paints its background.
#[test]
fn gui_pm_controls_test_matches_golden() {
    check_gui_golden("pm_controls_test", "samples/pm_controls_test/pm_controls_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// Menu bar from FCF_MENU, pull-down open under the File item.
#[test]
fn gui_menu_pulldown_matches_golden() {
    check_gui_golden("pm_menu_pulldown", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n3000 dump {dump}\n3100 quit\n");
}

/// File → About…: pull-down gone, modal dialog shown and active.
#[test]
fn gui_menu_about_dialog_matches_golden() {
    check_gui_golden("pm_menu_about_dialog", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n2500 click 62 80\n4000 dump {dump}\n4100 quit\n");
}

/// pm_bench: the self-benchmark runs every measurement, reports each with a
/// non-zero count and duration, and runs again when R is pressed. Only the
/// shape of the report is checked; the numbers depend on the machine.
#[test]
fn gui_pm_bench_reports_every_measurement() {
    let sample = "samples/pm_bench/pm_bench.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_pm_bench_reports_every_measurement: KVM or {sample} not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("session.script");
    // One run takes about 3 s; leave room for a loaded test machine.
    std::fs::write(&script, "6000 text r\n12000 quit\n").unwrap();
    let out = run_gui_script(sample, &script, true);

    assert_eq!(out.matches("pm_bench: done").count(), 2, "two complete runs:\n{out}");
    for name in ["GpiBox", "GpiLine", "GpiText", "WinSendMsg", "WinPostMsg", "Paint", "Timer"] {
        let lines: Vec<&str> = out.lines()
            .filter(|l| l.starts_with(&format!("pm_bench: {name}: "))).collect();
        assert_eq!(lines.len(), 2, "{name} reported once per run:\n{out}");
        for line in lines {
            // "pm_bench: <name>: <ops> ops in <ms> ms = ..."
            let words: Vec<&str> = line.split_whitespace().collect();
            let ops: u32 = words[2].parse().unwrap_or_else(|_| panic!("ops in {line:?}"));
            let ms: u32 = words[5].parse().unwrap_or_else(|_| panic!("ms in {line:?}"));
            assert!(ops > 0 && ms > 0, "{line}");
        }
    }
}

/// Run a self-checking sample that prints `<prefix>: <check>: ok` / `FAIL`
/// lines and `<prefix>: done <passed>/<total>`, and require every check to
/// pass. `min_checks` guards against a run that stopped early.
fn assert_sample_checks_pass(sample: &str, prefix: &str, min_checks: usize) {
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping {prefix}: KVM or {sample} not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("session.script");
    std::fs::write(&script, "5000 quit\n").unwrap();
    let out = run_gui_script(sample, &script, true);

    let done = format!("{prefix}: done");
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with(&format!("{prefix}: ")) && !l.starts_with(&done)).collect();
    assert!(checks.len() >= min_checks, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("{done} {n}/{n}", n = checks.len())), "{out}");
}

/// pm_pel_test: drawing is rasterised on the calling thread, so the guest
/// reads back with GpiQueryPel what GpiSetPel / GpiBox / GpiLine just drew,
/// and XOR / invert / leave-alone mixes combine with the pels already there.
#[test]
fn gui_pm_pel_test_reads_back_pels_and_mixes() {
    assert_sample_checks_pass("samples/pm_pel_test/pm_pel_test.exe", "pm_pel_test", 16);
}

/// pm_bitmap_test: a memory PS drawing into a bitmap, GpiBitBlt (1:1,
/// stretched, ROP_DSTINVERT), GpiQueryBitmapBits, GpiLoadBitmap from an
/// RT_BITMAP resource with WinDrawBitmap, logical colour tables, line types
/// and bitmap selection rules — checked from the guest side.
#[test]
fn gui_pm_bitmap_test_blits_bitmaps() {
    assert_sample_checks_pass("samples/pm_bitmap_test/pm_bitmap_test.exe", "pm_bitmap_test", 25);
}

/// pm_audio_test: DosBeep and MMPM/2 waveaudio played through the main-thread
/// SDL3 audio service (dummy audio driver): playback blocks for the file's
/// length with "wait", "notify" posts MM_MCINOTIFY, aliases and the
/// mciSendCommand parameter blocks follow the toolkit headers.
#[test]
fn gui_pm_audio_test_plays_through_the_audio_service() {
    assert_sample_checks_pass("samples/pm_audio_test/pm_audio_test.exe", "pm_audio_test", 17);
}

// Colours the compositor draws with (`src/gui/sdl_renderer.rs`).
const GUI_DESKTOP_BG: u32 = 0x40_8040;
const GUI_TITLE_ACTIVE: u32 = 0x00_0080;
const GUI_WHITE: u32 = 0xFF_FFFF;

/// `drag` on the title bar moves the frame: the default 640×480 frame sits at
/// desktop (32, 32) with a 20-px title bar, and is dragged by (+200, +200).
/// Checked on the pixels themselves, before and after, so the test fails if
/// the drag does nothing, lands elsewhere, or leaves the old image behind.
#[test]
fn gui_drag_title_bar_moves_window() {
    let sample = "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_drag_title_bar_moves_window: KVM or {sample} not available");
        return;
    }
    let dumps = run_gui_sample_dumps(sample,
        "1500 dump {dump0}\n1600 drag 300 42 500 242\n2600 dump {dump1}\n2700 quit\n", 2);
    let (before, after) = (&dumps[0], &dumps[1]);

    // Before: title bar across y 32..52, client below, bare desktop elsewhere.
    assert_eq!(gui_pixel(before, 600, 42), GUI_TITLE_ACTIVE, "title bar at its initial position");
    assert_eq!(gui_pixel(before, 600, 242), GUI_WHITE, "client area under the initial title bar");
    assert_eq!(gui_pixel(before, 800, 600), GUI_DESKTOP_BG, "desktop right of / below the frame");

    // After: the frame spans x 232..872 from y 232; its title bar is 200 px lower.
    assert_eq!(gui_pixel(after, 600, 242), GUI_TITLE_ACTIVE, "title bar at the new position");
    assert_eq!(gui_pixel(after, 800, 242), GUI_TITLE_ACTIVE, "title bar reaches the new right edge");
    assert_eq!(gui_pixel(after, 800, 600), GUI_WHITE, "client area moved with the frame");
    for (x, y) in [(600, 42), (100, 42), (100, 400), (600, 220), (220, 600)] {
        assert_eq!(gui_pixel(after, x, y), GUI_DESKTOP_BG,
            "({x}, {y}) should show the desktop once the frame has left");
    }
}

/// `drag` on a sizing border resizes the frame: the right edge of the
/// FCF_SIZEBORDER frame of pm_controls_test (x 32..672) is pulled 100 px to
/// the right, so title bar and client extend to x ≈ 772 and no further.
#[test]
fn gui_drag_sizing_border_resizes_window() {
    let sample = "samples/pm_controls_test/pm_controls_test.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_drag_sizing_border_resizes_window: KVM or {sample} not available");
        return;
    }
    let dumps = run_gui_sample_dumps(sample,
        "1500 dump {dump0}\n1600 drag 669 300 769 300\n2600 dump {dump1}\n2700 quit\n", 2);
    let (before, after) = (&dumps[0], &dumps[1]);

    assert_eq!(gui_pixel(before, 600, 42), GUI_TITLE_ACTIVE, "title bar before the resize");
    assert_eq!(gui_pixel(before, 700, 42), GUI_DESKTOP_BG, "desktop right of the frame");
    assert_eq!(gui_pixel(before, 720, 300), GUI_DESKTOP_BG, "desktop right of the frame");

    assert_eq!(gui_pixel(after, 700, 42), GUI_TITLE_ACTIVE, "title bar grew to the right");
    assert_ne!(gui_pixel(after, 720, 300), GUI_DESKTOP_BG, "frame now covers the widened area");
    assert_eq!(gui_pixel(after, 790, 300), GUI_DESKTOP_BG, "frame grew by the drag distance only");
    assert_eq!(gui_pixel(after, 790, 42), GUI_DESKTOP_BG, "frame grew by the drag distance only");
    // The left edge did not move.
    assert_eq!(gui_pixel(after, 20, 300), GUI_DESKTOP_BG, "left edge stayed in place");
    assert_eq!(gui_pixel(after, 200, 42), gui_pixel(before, 200, 42), "left part of the title bar unchanged");
}

/// Editing built-in controls end to end: click an entry field and type
/// (including a shifted and a non-ASCII character), Home then more text at the
/// start, then click the multi-line field and add a line. The sample ends
/// itself 3 s after start, so every step happens before that.
#[test]
fn gui_typing_into_controls_matches_golden() {
    check_gui_golden("pm_controls_typing", "samples/pm_controls_test/pm_controls_test.exe",
        "700 click 230 300\n900 text  Hello, W\u{f6}rld!\n1300 key home\n1400 text >>\n\
         1600 click 200 470\n1800 key enter\n1900 text three\n2500 dump {dump}\n2600 quit\n");
}
