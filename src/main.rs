pub mod lx;
pub mod loader;
pub mod api;

use std::env;
use std::fs::File;
use std::io::Read;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <os2_executable>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    let mut file = match File::open(file_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Failed to open file '{}': {}", file_path, e);
            std::process::exit(1);
        }
    };

    let mut data = Vec::new();
    if let Err(e) = file.read_to_end(&mut data) {
        eprintln!("Failed to read file: {}", e);
        std::process::exit(1);
    }

    // Phase 1: Try to parse LX executable
    let lx_file = match lx::LxFile::parse(&data) {
        Ok(lx) => lx,
        Err(e) => {
            eprintln!("Failed to parse LX executable: {}", e);
            std::process::exit(1);
        }
    };

    println!("Successfully parsed LX file. Initializing loader...");

    let mut loader = loader::Loader::new();
    if let Err(e) = loader.load(&lx_file) {
        eprintln!("Failed to load executable: {}", e);
        std::process::exit(1);
    }

    println!("Executing loaded program... (stub)");
    
    // Simulate program running and then calling DosExit
    api::doscalls::dos_exit(1, 0);
}
