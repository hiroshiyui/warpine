// build.rs — Code generation and linker setup.
//
// 1. SDL2 library path: on Debian/Ubuntu the SDL2 shared library lives in
//    /usr/lib/x86_64-linux-gnu/ which rust-lld does not search by default.
//    We query pkg-config for the library directory and emit the appropriate
//    cargo:rustc-link-search directive.
//
// 2. GNU Unifont SBCS glyph table: parses vendor/unifont/unifont.hex and
//    emits $OUT_DIR/font_unifont_sbcs.rs containing a sorted
//    `pub static UNIFONT_SBCS: &[(u32, [u8; 16])]` array of (Unicode
//    codepoint, 8×16 glyph) pairs for all half-width (8-pixel-wide) entries.
//    Only 8-wide entries (32 hex chars of pixel data) are included; 16-wide
//    CJK entries are skipped (reserved for Phase B DBCS support).

fn main() {
    if let Ok(lib_dir) = pkg_config_libdir("sdl2") {
        println!("cargo:rustc-link-search=native={}", lib_dir);
    }

    generate_unifont_sbcs();
}

fn pkg_config_libdir(lib: &str) -> Result<String, std::io::Error> {
    let out = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", lib])
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(std::io::Error::other("pkg-config failed"))
    }
}

/// Parse vendor/unifont/unifont.hex and emit $OUT_DIR/font_unifont_sbcs.rs.
///
/// The Unifont hex format is `HHHH:XXXX…` where HHHH is the Unicode codepoint
/// in hex and XXXX… is either 32 hex chars (8×16 half-width glyph = 16 bytes)
/// or 64 hex chars (16×16 full-width glyph = 32 bytes).  Only half-width
/// entries are included in the generated table.
fn generate_unifont_sbcs() {
    let hex_path = "vendor/unifont/unifont.hex";
    println!("cargo:rerun-if-changed={}", hex_path);

    let content = std::fs::read_to_string(hex_path)
        .unwrap_or_default(); // tolerate absence during `cargo publish`

    let mut entries: Vec<(u32, [u8; 16])> = Vec::with_capacity(8192);

    for line in content.lines() {
        let Some((cp_str, data_str)) = line.split_once(':') else { continue };
        if data_str.len() != 32 { continue; } // skip 16×16 wide glyphs
        let Ok(cp) = u32::from_str_radix(cp_str, 16) else { continue };

        let bytes = data_str.as_bytes();
        let mut glyph = [0u8; 16];
        let mut ok = true;
        for (i, chunk) in bytes.chunks(2).enumerate() {
            let Ok(hi) = hex_nibble(chunk[0]) else { ok = false; break };
            let Ok(lo) = hex_nibble(chunk[1]) else { ok = false; break };
            glyph[i] = (hi << 4) | lo;
        }
        if ok {
            entries.push((cp, glyph));
        }
    }

    entries.sort_unstable_by_key(|&(cp, _)| cp);

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = format!("{}/font_unifont_sbcs.rs", out_dir);

    let mut s = String::with_capacity(entries.len() * 64);
    s.push_str("pub static UNIFONT_SBCS: &[(u32, [u8; 16])] = &[\n");
    for (cp, g) in &entries {
        s.push_str(&format!(
            "    (0x{:04X}, [{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}]),\n",
            cp,
            g[0],  g[1],  g[2],  g[3],
            g[4],  g[5],  g[6],  g[7],
            g[8],  g[9],  g[10], g[11],
            g[12], g[13], g[14], g[15],
        ));
    }
    s.push_str("];\n");

    std::fs::write(&out_path, s).expect("write font_unifont_sbcs.rs");
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}
