pub mod lx;
pub mod loader;
pub mod api;

use std::env;

fn main() {
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

    println!("Successfully parsed LX file: {}", file_path);
    println!("  CPU Type:    {:?}", lx_file.header.cpu_type);
    println!("  OS Type:     {:?}", lx_file.header.os_type);
    println!("  Module Flags: 0x{:08X}", lx_file.header.module_flags);
    println!("  Entry EIP:   0x{:08X} (Object {})", lx_file.header.eip, lx_file.header.eip_object);
    println!("  Entry ESP:   0x{:08X} (Object {})", lx_file.header.esp, lx_file.header.esp_object);
    println!("  Page Size:   {}", lx_file.header.page_size);

    println!("\nObject Table ({} objects):", lx_file.object_table.len());
    println!("  # | Base Addr  | Virt Size  | Flags      | Pages");
    println!(" ---|------------|------------|------------|-------");
    for (i, obj) in lx_file.object_table.iter().enumerate() {
        println!(
            "  {:>1} | 0x{:08X} | 0x{:08X} | 0x{:08X} | {}",
            i + 1,
            obj.base_address,
            obj.size,
            obj.flags,
            obj.page_count
        );
    }

    println!("\nObject Page Map ({} pages):", lx_file.page_map.len());
    println!("  # | Offset   | Size | Flags");
    println!(" ---|----------|------|-------");
    for (i, page) in lx_file.page_map.iter().enumerate() {
        println!(
            "  {:>1} | 0x{:08X} | {:>4} | 0x{:04X}",
            i + 1,
            page.data_offset,
            page.data_size,
            page.flags
        );
    }

    println!("\nInitializing loader...");

    let mut loader = loader::Loader::new();
    if let Err(e) = loader.load(&lx_file) {
        eprintln!("Failed to load executable: {}", e);
        std::process::exit(1);
    }

    println!("Executing loaded program... (stub)");
    
    // Simulate program running and then calling DosExit
    api::doscalls::dos_exit(1, 0);
}
