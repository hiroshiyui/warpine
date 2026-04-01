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
//    CJK entries are handled by (3).
//
// 3. GNU Unifont wide glyph table (DBCS Phase B5): parses the same
//    vendor/unifont/unifont.hex file and emits:
//      - $OUT_DIR/font_unifont_wide.bin — sorted packed (u32_le, [u8;32])
//        entries for all 16×16 full-width glyphs (~49,804 entries, ~1.8 MB).
//      - $OUT_DIR/font_unifont_wide.rs — Rust source that includes the binary
//        and exposes `pub fn get_glyph_dbcs(ch: char) -> [u8; 32]` via
//        binary search.

fn main() {
    if let Ok(lib_dir) = pkg_config_libdir("sdl2") {
        println!("cargo:rustc-link-search=native={}", lib_dir);
    }

    generate_unifont_sbcs();
    generate_unifont_wide();
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

/// Parse vendor/unifont/unifont.hex and emit font_unifont_wide.bin + font_unifont_wide.rs.
///
/// Only 16×16 entries (64 hex chars of pixel data = 32 bytes) are included.
/// The binary layout per entry is: `[u8; 4]` codepoint (little-endian) + `[u8; 32]` glyph.
/// Entries are sorted by codepoint for O(log N) binary search at runtime.
fn generate_unifont_wide() {
    let hex_path = "vendor/unifont/unifont.hex";
    // rerun-if-changed already emitted by generate_unifont_sbcs for the same file.

    let content = std::fs::read_to_string(hex_path)
        .unwrap_or_default(); // tolerate absence during `cargo publish`

    let mut entries: Vec<(u32, [u8; 32])> = Vec::with_capacity(50_000);

    for line in content.lines() {
        let Some((cp_str, data_str)) = line.split_once(':') else { continue };
        if data_str.len() != 64 { continue; } // only 16×16 wide glyphs
        let Ok(cp) = u32::from_str_radix(cp_str, 16) else { continue };

        let bytes = data_str.as_bytes();
        let mut glyph = [0u8; 32];
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

    // Write the binary blob: sorted (u32_le codepoint, [u8;32] glyph) pairs.
    const ENTRY_SIZE: usize = 36; // 4 + 32
    let mut bin: Vec<u8> = Vec::with_capacity(entries.len() * ENTRY_SIZE);
    for (cp, glyph) in &entries {
        bin.extend_from_slice(&cp.to_le_bytes());
        bin.extend_from_slice(glyph);
    }
    let bin_path = format!("{}/font_unifont_wide.bin", out_dir);
    std::fs::write(&bin_path, &bin).expect("write font_unifont_wide.bin");

    // Write the Rust wrapper that exposes get_glyph_dbcs().
    let rs = r#"// Auto-generated by build.rs — do not edit.
static WIDE_BIN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font_unifont_wide.bin"));

/// Look up a 16×16 Unifont glyph for a wide (DBCS) Unicode character.
///
/// Returns a 32-byte array: 16 rows × 2 bytes per row (left 8 pixels, right 8 pixels).
/// Returns `[0u8; 32]` (blank) if the codepoint is not present in the table.
pub fn get_glyph_dbcs(ch: char) -> [u8; 32] {
    const ENTRY_SIZE: usize = 36; // 4 bytes codepoint + 32 bytes glyph
    let target = ch as u32;
    let n = WIDE_BIN.len() / ENTRY_SIZE;
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let off = mid * ENTRY_SIZE;
        let cp = u32::from_le_bytes([
            WIDE_BIN[off],
            WIDE_BIN[off + 1],
            WIDE_BIN[off + 2],
            WIDE_BIN[off + 3],
        ]);
        match cp.cmp(&target) {
            std::cmp::Ordering::Equal => {
                let mut g = [0u8; 32];
                g.copy_from_slice(&WIDE_BIN[off + 4..off + 36]);
                return g;
            }
            std::cmp::Ordering::Less    => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    [0u8; 32] // codepoint not found in Unifont wide table
}
"#;
    let rs_path = format!("{}/font_unifont_wide.rs", out_dir);
    std::fs::write(&rs_path, rs).expect("write font_unifont_wide.rs");
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}
