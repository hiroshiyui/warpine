// SPDX-License-Identifier: GPL-3.0-or-later
pub mod lx;
pub mod ne;
pub mod loader;
pub mod api;
pub mod gui;
pub mod desktop;

use std::env;
use std::sync::Arc;
use log::{info, debug};
use loader::MutexExt;

/// Initialise the tracing/logging stack.
///
/// `WARPINE_TRACE` controls the output format:
///
/// | Value          | Format                                                  |
/// |----------------|---------------------------------------------------------|
/// | unset / `0`    | Default pretty format (similar to env_logger)           |
/// | `strace` / `1` | Compact span-events format — strace-like API call log   |
/// | `json`         | JSON lines — one object per event/span for tooling      |
///
/// `RUST_LOG` controls the level filter in all modes (default: `info`).
fn init_logging() {
    use tracing_subscriber::{fmt, EnvFilter, prelude::*};

    // tracing_subscriber::registry().init() automatically initialises the
    // log → tracing bridge (tracing_log::LogTracer) when the tracing-log
    // feature is active, so all existing log:: calls in other modules are
    // forwarded to the tracing subscriber without any extra setup here.

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let trace_mode = std::env::var("WARPINE_TRACE").unwrap_or_default();
    match trace_mode.as_str() {
        "json" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer()
                    .with_writer(std::io::stderr)
                    .json()
                    .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::CLOSE))
                .init();
        }
        v if !v.is_empty() && v != "0" && v != "false" => {
            // strace-like: compact format with span entry/exit events visible
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer()
                    .with_writer(std::io::stderr)
                    .compact()
                    .with_target(false)
                    .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::CLOSE))
                .init();
        }
        _ => {
            // Default: no span events, just log messages (mirrors env_logger)
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
                .init();
        }
    }
}

/// Detect executable format by reading the signature at the NE/LX header offset.
fn detect_format(path: &str) -> Result<ExeFormat, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}", e))?;
    let mut mz = [0u8; 64];
    f.read_exact(&mut mz).map_err(|e| format!("{}", e))?;
    if &mz[0..2] != b"MZ" {
        return Err("Not a DOS/OS2 executable (missing MZ header)".into());
    }
    let e_lfanew = u32::from_le_bytes([mz[60], mz[61], mz[62], mz[63]]);
    use std::io::Seek;
    f.seek(std::io::SeekFrom::Start(e_lfanew as u64)).map_err(|e| format!("{}", e))?;
    let mut sig = [0u8; 2];
    f.read_exact(&mut sig).map_err(|e| format!("{}", e))?;
    match &sig {
        b"NE" => Ok(ExeFormat::NE),
        b"LX" | b"LE" => Ok(ExeFormat::LX),
        _ => Err(format!("Unknown executable format: {:?}", sig)),
    }
}

enum ExeFormat { LX, NE }

/// The switch that prints [`license_notices`].
const LICENSES_SWITCH: &str = "--licenses";

/// Warpine's licence and the licences of the fonts built into the binary
/// (ADR-0076). The SIL Open Font License asks that every copy of the fonts
/// carry their copyright notice and the licence, readable by the user; a
/// `warpine` binary passed on alone carries them here.
fn license_notices() -> String {
    const SEPARATOR: &str = "\n----------------------------------------------------------------------\n\n";
    [
        concat!(
            "Warpine ", env!("CARGO_PKG_VERSION"), "\n\n",
            "Warpine is free software: you can redistribute it and/or modify it under\n",
            "the terms of the GNU General Public License as published by the Free\n",
            "Software Foundation, either version 3 of the License, or (at your option)\n",
            "any later version. It comes with NO WARRANTY. The licence text is in the\n",
            "LICENSE file of its source, or at <https://www.gnu.org/licenses/>.\n\n",
            "Built into this program: the IBM Plex fonts and GNU Unifont, under the\n",
            "notices below.\n",
        ),
        concat!("IBM Plex\n\n", include_str!("../vendor/ibm-plex/LICENSE.txt")),
        include_str!("../vendor/unifont/LICENSE.txt"),
        concat!("GNU Unifont: SIL Open Font License 1.1\n\n", include_str!("../vendor/unifont/OFL-1.1.txt")),
    ]
    .join(SEPARATOR)
}

fn main() {
    init_logging();
    // The pipe a Warpine parent gave for this process's exit code (ADR-0090).
    loader::exit_report::adopt_from_env();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <os2_executable> [--gdb <port>]", args[0]);
        eprintln!("       {} --compat         Print API compatibility report", args[0]);
        eprintln!("       {} {LICENSES_SWITCH}       Print the licences of Warpine and its built-in fonts", args[0]);
        std::process::exit(1);
    }

    // The session's desktop server (ADR-0052), started by the first PM
    // process of a session: `warpine --desktop-server <session dir>`.
    if args[1] == desktop::client::SERVER_SWITCH {
        let Some(dir) = args.get(2) else {
            eprintln!("{} needs the session directory", desktop::client::SERVER_SWITCH);
            std::process::exit(1);
        };
        std::process::exit(gui::desktop_server::run(std::path::Path::new(dir)));
    }

    if args[1] == LICENSES_SWITCH {
        print!("{}", license_notices());
        return;
    }

    if args[1] == "--compat" {
        print!("{}", loader::api_registry::compat_report());
        return;
    }

    // A process started past the nesting limit runs no guest code
    // (loader::inherit, process nesting guard).
    let depth = loader::inherit::own_depth();
    if !loader::inherit::depth_allowed(depth) {
        eprintln!("warpine: process nesting limit ({}) exceeded; not starting {}",
                  loader::inherit::MAX_PROCESS_DEPTH, args[1]);
        std::process::exit(loader::inherit::EXIT_NESTING_LIMIT);
    }

    // Parse optional --gdb <port> flag (position-independent).
    let mut gdb_port: Option<u16> = None;
    let mut remaining_args: Vec<&str> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--gdb" {
            i += 1;
            if i >= args.len() {
                eprintln!("--gdb requires a port number");
                std::process::exit(1);
            }
            gdb_port = Some(args[i].parse::<u16>().unwrap_or_else(|_| {
                eprintln!("Invalid GDB port: {}", args[i]);
                std::process::exit(1);
            }));
        } else {
            remaining_args.push(&args[i]);
        }
        i += 1;
    }

    if remaining_args.is_empty() {
        eprintln!("Usage: {} <os2_executable> [--gdb <port>]", args[0]);
        std::process::exit(1);
    }

    let file_path = remaining_args[0];

    // Intercept CMD.EXE / OS2SHELL.EXE directly from the command line.
    // When stdout is a real terminal and WARPINE_HEADLESS is not set, open an
    // SDL 640×400 text window just like a normal CLI app.  Otherwise fall back
    // to terminal/headless mode.
    {
        let base = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path)
            .to_ascii_uppercase();
        if base == "CMD.EXE" || base == "OS2SHELL.EXE" {
            use std::io::IsTerminal;
            use std::sync::Arc;

            let headless = headless_requested(std::env::var_os("WARPINE_HEADLESS").is_some(),
                                              std::io::stdout().is_terminal());

            let host_args: Vec<String> = remaining_args[1..].iter()
                .map(|s| s.to_string()).collect();

            if headless {
                let mut loader = loader::Loader::new();
                join_thread_session(&loader);
                let args: Vec<&str> = host_args.iter().map(|s| s.as_str()).collect();
                loader.run_builtin_cmd_main(&args);
            } else {
                let loader = Arc::new(loader::Loader::new());
                join_thread_session(&loader);
                let shared = loader.get_shared();
                *shared.exe_name.lock_or_recover() = "CMD.EXE".to_string();
                // CMD.EXE runs windowed or full screen on OS/2.
                run_vio_session(shared, "Warpine — CMD.EXE", lx::header::AppType::WindowCompatible, move || {
                    std::thread::spawn(move || loader.run_builtin_cmd_sdl(host_args));
                });
            }
            return;
        }
    }

    let format = match detect_format(file_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to detect executable format '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };

    match format {
        ExeFormat::NE => run_ne(file_path, &remaining_args[1..]),
        ExeFormat::LX => run_lx(file_path, gdb_port, &remaining_args[1..]),
    }
}

/// Enter the session's `THREADS=` accounting with thread 1, or end with
/// `ERROR_NO_PROC_SLOTS` before any guest code runs when it has no slot left
/// (`loader::session_threads`).
fn join_thread_session(loader: &loader::Loader) {
    if !loader.join_thread_session() {
        eprintln!("warpine: the session's THREADS= limit is reached; not starting the program");
        std::process::exit(loader::session_threads::EXIT_NO_THREAD_SLOT);
    }
}

/// The guest's arguments from the host command line (`warpine prog.exe
/// a b` → `a b`), unless a parent Warpine process passed its own
/// (`DosExecPgm`, `loader::inherit`).
fn set_host_args(loader: &loader::Loader, args: &[&str]) {
    if !loader::inherit::StartupInfo::from_parent() {
        loader.get_shared().startup.lock_or_recover().args = args.join(" ").into_bytes();
    }
}

fn run_ne(file_path: &str, args: &[&str]) -> ! {
    let ne_file = match ne::NeFile::open(file_path) {
        Ok(ne) => ne,
        Err(e) => {
            eprintln!("Failed to parse NE executable '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };
    info!("Successfully parsed NE file: {} ({} segments, {} modules)",
        file_path, ne_file.segment_table.len(), ne_file.imported_modules.len());
    for (i, seg) in ne_file.segment_table.iter().enumerate() {
        debug!("  Segment {}: {} bytes, {} alloc, {}",
            i + 1, seg.actual_data_length(), seg.actual_min_alloc(),
            if seg.is_code() { "CODE" } else { "DATA" });
    }

    let mut loader = loader::Loader::new();
    join_thread_session(&loader);
    set_host_args(&loader, args);
    if let Err(e) = loader.load_ne(&ne_file, file_path) {
        eprintln!("Failed to load NE executable: {}", e);
        std::process::exit(1);
    }
    loader.setup_and_run_ne_cli(&ne_file)
}

/// Connect a PM program to the session's desktop server (ADR-0052),
/// starting the server if none runs. Its window handles get the slot the
/// server gives, and input the server sends is acknowledged as it is read.
/// `None` keeps the desktop in this process: `WARPINE_DESKTOP=local`, no
/// shared session directory, or no server could be reached.
fn desktop_link(shared: &Arc<loader::SharedState>) -> Option<Arc<desktop::client::DesktopLink>> {
    if !desktop::client::server_wanted() {
        return None;
    }
    let dir = shared.thread_slots.lock_or_recover().session()
        .filter(|s| s.is_shared())
        .map(|s| s.dir().path().clone())?;
    match desktop::client::DesktopLink::open(&dir) {
        Ok(link) => {
            let mut wm = shared.window_mgr.lock_or_recover();
            wm.set_handle_slot(link.slot);
            // The session's switch list (ADR-0056).
            wm.session_link = Some(link.clone());
            let l = link.clone();
            wm.input_ack = Some(Arc::new(move |token, active| {
                l.send(&desktop::proto::ClientMsg::Ack { token, active }, None);
            }));
            // Calls on other programs' windows go through it (phase B).
            let _ = shared.desktop_link.set(link.clone());
            Some(link)
        }
        Err(e) => {
            eprintln!("warpine: no desktop server ({e}); the desktop stays in this process");
            None
        }
    }
}

/// Whether a VIO program runs headless (host terminal / plain output): when
/// `WARPINE_HEADLESS` is set, or stdout is not a terminal (a program started
/// by `DosExecPgm` with its output piped back).
fn headless_requested(env_set: bool, stdout_is_terminal: bool) -> bool {
    env_set || !stdout_is_terminal
}

/// Run a VIO program on the SDL front end: `start` launches its guest
/// thread once the display exists. A windowed session (the default) puts
/// the text screen in a frame on the PM desktop; a full-screen one
/// (`WARPINE_VIO=fullscreen`, or a program that is not window-compatible)
/// uses the full-screen text display. Alt+Home switches a window-compatible
/// program between the two (`gui::vio_switch`): the current display's loop
/// returns, its window is parked and the other display opens on the same SDL
/// context, while the guest, its VIO buffer and keyboard queue carry on.
/// Never returns.
fn run_vio_session(shared: Arc<loader::SharedState>, title: &str, app_type: lx::header::AppType,
                   start: impl FnOnce()) -> ! {
    use std::sync::atomic::Ordering;
    use gui::vio_window::{choose_display, VioDisplay, VIO_ENV};
    use gui::vio_switch::{alt_home_target, take_switch_request, Parked};
    shared.console_mgr.lock_or_recover().enable_sdl_mode();
    shared.use_sdl_text.store(true, Ordering::Relaxed);

    let env = std::env::var(VIO_ENV).ok();
    let (mut display, warning) = choose_display(app_type, env.as_deref());
    if let Some(warning) = warning {
        eprintln!("warpine: {warning}");
    }
    shared.vio_switchable.store(alt_home_target(app_type, display).is_some(), Ordering::Relaxed);
    let sdl = sdl3::init().expect("Failed to initialise SDL");
    // The PM side's channel outlives each of its renderers.
    let (gui_sender, gui_rx) = gui::create_gui_channel();
    shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);
    let mut parked = Parked::default();
    let mut start = Some(start);
    loop {
        // The renderer (and its audio output) exists before the guest runs.
        match display {
            VioDisplay::FullScreen => {
                let mut renderer = gui::SdlTextRenderer::open(&sdl, title, &shared.audio, &mut parked);
                if let Some(start) = start.take() { start(); }
                gui::run_text_loop(&mut renderer, shared.clone());
                renderer.park(&mut parked);
            }
            VioDisplay::Window => {
                let mut renderer = gui::SdlRenderer::open(&sdl, &shared.audio, &mut parked);
                renderer.attach_vio_window(&shared, title);
                if let Some(start) = start.take() { start(); }
                gui::run_pm_loop(&mut renderer, shared.clone(), &gui_rx);
                renderer.park(&mut parked);
            }
        }
        let switch = take_switch_request(&shared);
        match alt_home_target(app_type, display) {
            Some(next) if switch && !shared.exit_requested.load(Ordering::Relaxed) => display = next,
            _ => break,
        }
    }

    shared.exit_requested.store(true, Ordering::Relaxed);
    shared.kbd_cond.notify_all();
    shared.mou_cond.notify_all();
    shared.npipes.lock_or_recover().close_all();
    shared.leave_thread_session();
    shared.console_mgr.lock_or_recover().disable_raw_mode();
    let code = shared.exit_code.load(Ordering::Relaxed);
    loader::exit_report::report(code as u32);
    std::process::exit(code);
}

fn run_lx(file_path: &str, gdb_port: Option<u16>, args: &[&str]) {
    // Phase 1: Try to open and parse LX executable
    let lx_file = match lx::LxFile::open(file_path) {
        Ok(lx) => lx,
        Err(e) => {
            eprintln!("Failed to parse LX executable '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };

    info!("Successfully parsed LX file: {}", file_path);
    debug!("  CPU Type:    {:?}", lx_file.header.cpu_type);
    debug!("  OS Type:     {:?}", lx_file.header.os_type);
    debug!("  Module Flags: 0x{:08X}", lx_file.header.module_flags);
    debug!("  Entry EIP:   0x{:08X} (Object {})", lx_file.header.eip, lx_file.header.eip_object);
    debug!("  Entry ESP:   0x{:08X} (Object {})", lx_file.header.esp, lx_file.header.esp_object);
    debug!("  Page Size:   {}", lx_file.header.page_size);

    debug!("\nObject Table ({} objects):", lx_file.object_table.len());
    debug!("  # | Base Addr  | Virt Size  | Flags      | Pages");
    debug!(" ---|------------|------------|------------|-------");
    for (i, obj) in lx_file.object_table.iter().enumerate() {
        debug!(
            "  {:>1} | 0x{:08X} | 0x{:08X} | 0x{:08X} | {}",
            i + 1,
            obj.base_address,
            obj.size,
            obj.flags,
            obj.page_count
        );
    }

    debug!("\nObject Page Map ({} pages):", lx_file.page_map.len());
    debug!("  # | Offset   | Size | Flags");
    debug!(" ---|----------|------|-------");
    for (i, page) in lx_file.page_map.iter().enumerate() {
        debug!(
            "  {:>1} | 0x{:08X} | {:>4} | 0x{:04X}",
            i + 1,
            page.data_offset,
            page.data_size,
            page.flags
        );
    }

    debug!("\nImported Modules:");
    for (i, name) in lx_file.imported_modules.iter().enumerate() {
        debug!("  {:>2} | {}", i + 1, name);
    }

    debug!("\nFixup Record Table ({} pages):", lx_file.fixup_records_by_page.len());
    for (p, records) in lx_file.fixup_records_by_page.iter().enumerate() {
        if records.is_empty() { continue; }
        debug!("  Page {}:", p + 1);
        for (i, record) in records.iter().enumerate() {
            let target_desc = match &record.target {
                lx::header::FixupTarget::Internal { object_num, target_offset } => {
                    format!("Internal: Obj {} + 0x{:08X}", object_num, target_offset)
                },
                lx::header::FixupTarget::ExternalOrdinal { module_ordinal, proc_ordinal } => {
                    let mod_name = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1))
                        .map(|s| s.as_str()).unwrap_or("?");
                    format!("Import: {}.#{}", mod_name, proc_ordinal)
                },
                lx::header::FixupTarget::ExternalName { module_ordinal, proc_name_offset } => {
                    let mod_name = lx_file.imported_modules.get((*module_ordinal as usize).wrapping_sub(1))
                        .map(|s| s.as_str()).unwrap_or("?");
                    let proc_name = lx_file.get_proc_name(*proc_name_offset).unwrap_or_else(|| "?".to_string());
                    format!("Import: {}.{}", mod_name, proc_name)
                },
                lx::header::FixupTarget::InternalEntry { entry_ordinal } => {
                    format!("Internal Entry: #{}", entry_ordinal)
                }
            };

            debug!(
                "    {:>2} | Offsets: {:?} | Type: 0x{:02X} | Target: {}",
                i + 1,
                record.source_offsets,
                record.source_type,
                target_desc
            );
        }
    }

    info!("Initializing KVM loader...");

    let mut loader = loader::Loader::new();
    join_thread_session(&loader);
    set_host_args(&loader, args);

    // Attach GDB stub if --gdb was requested.
    // Must be done before get_shared() so Arc::get_mut() succeeds inside
    // set_gdb_state().
    if let Some(port) = gdb_port {
        let gdb_state = Arc::new(loader::gdb_stub::GdbState::new());
        loader.set_gdb_state(gdb_state);
        loader::gdb_stub::launch_gdb_stub(loader.get_shared(), port);
    }

    let shared = loader.get_shared();
    let is_pm = loader.is_pm_app(&lx_file);

    // Set first: the DLLs the program imports are looked for beside it.
    *shared.exe_name.lock_or_recover() = file_path.to_string();
    if let Err(e) = loader.load(&lx_file, file_path) {
        eprintln!("Failed to load executable: {}", e);
        std::process::exit(1);
    }

    if is_pm {
        // PM app: SDL must be initialised on the main thread.
        let sdl = sdl3::init().expect("Failed to initialise SDL");
        let (gui_sender, gui_rx) = gui::create_gui_channel();

        // Store the sender in the window manager
        shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        // The renderer (and its audio output) exists before the guest runs:
        // on the session's desktop server, else in a window of its own.
        let mut renderer = match desktop_link(&shared) {
            Some(link) => gui::SdlRenderer::new_remote(&sdl, &shared.audio, link),
            None => gui::SdlRenderer::new(&sdl, &shared.audio),
        };

        // Launch VCPU thread; other programs' calls on this one's windows
        // are answered from its window manager (ADR-0052 phase B).
        let loader = Arc::new(loader);
        if let Some(link) = shared.desktop_link.get() {
            link.set_responder(loader.clone());
        }
        loader.clone().setup_and_spawn_vcpu(&lx_file);

        // Run the GUI event loop on the main thread (returns when done).
        gui::run_pm_loop(&mut renderer, shared.clone(), &gui_rx);

        // Cleanup: signal shutdown, stop timers, reset terminal, and exit
        shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
        shared.window_mgr.lock_or_recover().stop_all_timers();
        shared.npipes.lock_or_recover().close_all();
        shared.leave_thread_session();
        shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        loader::exit_report::report(code as u32);
        std::process::exit(code);
    } else {
        // CLI app: use SDL text-mode window only when stdout is a real terminal.
        // When stdout is piped (e.g. spawned as a child by DosExecPgm), fall
        // back to headless mode automatically — no env var needed.
        use std::io::IsTerminal;
        let headless = headless_requested(std::env::var_os("WARPINE_HEADLESS").is_some(),
                                          std::io::stdout().is_terminal());
        if headless {
            // Headless / terminal mode: run directly (never returns).
            loader.setup_and_run_cli(&lx_file);
        } else {
            // A VIO program on a terminal: in a window on the PM desktop or
            // in the separate text window, as the module flags and
            // WARPINE_VIO say; the VCPU runs on a worker thread.
            let exe_title = file_path.rsplit('/').next().unwrap_or(file_path);
            let title = format!("Warpine — {exe_title}");
            let app_type = lx_file.header.app_type();
            let loader = Arc::new(loader);
            run_vio_session(shared, &title, app_type, move || {
                loader.clone().setup_and_spawn_vcpu(&lx_file);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{headless_requested, license_notices};

    /// `--licenses` carries Warpine's licence and each built-in font's
    /// copyright notice and licence text in full.
    #[test]
    fn license_notices_carry_every_font_notice_and_licence() {
        let text = license_notices();
        assert!(text.contains("GNU General Public License"));
        assert!(text.contains("either version 3 of the License, or (at your option)"));
        assert!(text.contains("Copyright © 2017 IBM Corp. with Reserved Font Name \"Plex\""));
        assert!(text.contains("Roman Czyborra created the original GNU Unifont"));
        assert!(text.contains("GNU font embedding exception"));
        // The OFL in full, once for each font, to its last clause.
        assert_eq!(text.matches("SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007").count(), 2);
        assert_eq!(text.matches("OTHER DEALINGS IN THE FONT SOFTWARE.").count(), 2);
    }

    /// WARPINE_HEADLESS forces headless mode even on a terminal; a piped
    /// stdout is headless anyway.
    #[test]
    fn headless_when_asked_or_without_a_terminal() {
        assert!(!headless_requested(false, true));
        assert!(headless_requested(true, true));
        assert!(headless_requested(false, false));
        assert!(headless_requested(true, false));
    }
}
