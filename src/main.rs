// SPDX-License-Identifier: GPL-3.0-only
pub mod lx;
pub mod ne;
pub mod loader;
pub mod api;
pub mod gui;
pub mod font8x16;

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
                    .json()
                    .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::CLOSE))
                .init();
        }
        v if !v.is_empty() && v != "0" && v != "false" => {
            // strace-like: compact format with span entry/exit events visible
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer()
                    .compact()
                    .with_target(false)
                    .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::CLOSE))
                .init();
        }
        _ => {
            // Default: no span events, just log messages (mirrors env_logger)
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(true))
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

fn main() {
    init_logging();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <os2_executable>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];

    let format = match detect_format(file_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to detect executable format '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };

    match format {
        ExeFormat::NE => run_ne(file_path),
        ExeFormat::LX => run_lx(file_path),
    }
}

fn run_ne(file_path: &str) -> ! {
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
    if let Err(e) = loader.load_ne(&ne_file, file_path) {
        eprintln!("Failed to load NE executable: {}", e);
        std::process::exit(1);
    }
    loader.setup_and_run_ne_cli(&ne_file)
}

fn run_lx(file_path: &str) {
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
    let shared = loader.get_shared();
    let is_pm = loader.is_pm_app(&lx_file);

    if let Err(e) = loader.load(&lx_file, file_path) {
        eprintln!("Failed to load executable: {}", e);
        std::process::exit(1);
    }
    *shared.exe_name.lock_or_recover() = file_path.to_string();

    if is_pm {
        // PM app: SDL2 must be initialised on the main thread.
        let sdl = sdl2::init().expect("Failed to initialise SDL2");
        let (gui_sender, gui_rx) = gui::create_gui_channel();

        // Store the sender in the window manager
        shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        // Launch VCPU thread
        let loader = Arc::new(loader);
        loader.clone().setup_and_spawn_vcpu(&lx_file);

        // Run SDL2 GUI event loop on the main thread (returns when done).
        let mut renderer = gui::Sdl2Renderer::new(&sdl);
        gui::run_pm_loop(&mut renderer, shared.clone(), gui_rx);

        // Cleanup: signal shutdown, stop timers, reset terminal, and exit
        shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
        shared.window_mgr.lock_or_recover().stop_all_timers();
        shared.console_mgr.lock_or_recover().disable_raw_mode();
        let code = shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        std::process::exit(code);
    } else {
        // CLI app: use SDL2 text-mode window unless WARPINE_HEADLESS is set.
        let headless = std::env::var("WARPINE_HEADLESS").map(|v| v != "0").unwrap_or(false);
        if headless {
            // Headless / terminal mode: run directly (never returns).
            loader.setup_and_run_cli(&lx_file);
        } else {
            // SDL2 text-mode: enable sdl2 mode on VioManager, then spawn VCPU
            // on a worker thread while the SDL2 text renderer runs on main.
            {
                let mut vio = shared.console_mgr.lock_or_recover();
                vio.enable_sdl2_mode();
            }
            shared.use_sdl2_text.store(true, std::sync::atomic::Ordering::Relaxed);

            let sdl = sdl2::init().expect("Failed to initialise SDL2");
            let exe_title = file_path.rsplit('/').next().unwrap_or(file_path);
            let title = format!("Warpine — {exe_title}");

            let loader = Arc::new(loader);
            loader.clone().setup_and_spawn_vcpu(&lx_file);

            let mut renderer = gui::Sdl2TextRenderer::new(&sdl, &title);
            gui::run_text_loop(&mut renderer, shared.clone());

            // Cleanup
            shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
            shared.kbd_cond.notify_all();
            shared.console_mgr.lock_or_recover().disable_raw_mode();
            let code = shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
            std::process::exit(code);
        }
    }
}
