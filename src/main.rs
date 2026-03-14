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

fn main() {
    env_logger::init();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <os2_executable>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];

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
    *shared.exe_name.lock_or_recover() = file_path.clone();

    if is_pm {
        // PM app: create GUI event loop and run VCPU in background
        let event_loop = winit::event_loop::EventLoop::<()>::with_user_event()
            .build()
            .expect("Failed to create event loop");
        let (gui_sender, gui_rx) = gui::create_gui_channel(&event_loop);

        // Store the sender in the window manager
        shared.window_mgr.lock_or_recover().gui_tx = Some(gui_sender);

        // Launch VCPU thread
        let loader = Arc::new(loader);
        loader.clone().setup_and_spawn_vcpu(&lx_file);

        // Run GUI event loop on main thread
        let mut app = gui::GUIApp::new(shared.clone(), gui_rx);
        event_loop.run_app(&mut app).expect("Event loop failed");

        // Cleanup: signal shutdown, stop timers, and exit
        shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
        shared.window_mgr.lock_or_recover().stop_all_timers();
        let code = shared.exit_code.load(std::sync::atomic::Ordering::Relaxed);
        std::process::exit(code);
    } else {
        // CLI app: run directly
        loader.setup_and_run_cli(&lx_file);
    }
}
