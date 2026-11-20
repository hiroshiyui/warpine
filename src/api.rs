// SPDX-License-Identifier: GPL-3.0-or-later
pub mod doscalls {
    /// Write `buf` to the host's standard output (`fd` 1) or error (2).
    /// A failed write is an error: a reader that closed its pipe must
    /// reach the program as a broken pipe, not as bytes written.
    pub fn dos_write(fd: u32, buf: &[u8]) -> Result<u32, std::io::ErrorKind> {
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
            target.write_all(out.as_bytes()).and_then(|()| target.flush()).map_err(|e| e.kind())?;
            Ok(buf.len() as u32)
        } else {
            Err(std::io::ErrorKind::InvalidInput)
        }
    }
}
