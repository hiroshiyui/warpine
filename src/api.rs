// SPDX-License-Identifier: GPL-3.0-only
pub mod doscalls {
    pub fn dos_write(fd: u32, buf: &[u8]) -> Result<u32, &'static str> {
        if fd == 1 || fd == 2 {
            use std::io::Write;
            // Convert CP437 bytes to UTF-8 for terminal output.
            // ASCII bytes (0x00–0x7F) pass through unchanged.
            // High bytes (0x80–0xFF) are CP437 glyphs (box-drawing, accented, etc.).
            let mut out = String::with_capacity(buf.len());
            for &b in buf {
                out.push(crate::loader::console::cp437_to_char(b));
            }
            let target: &mut dyn Write = if fd == 2 {
                &mut std::io::stderr()
            } else {
                &mut std::io::stdout()
            };
            let _ = target.write_all(out.as_bytes());
            let _ = target.flush();
            Ok(buf.len() as u32)
        } else {
            Err("Unsupported file descriptor")
        }
    }
}
