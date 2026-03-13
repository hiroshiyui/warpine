// SPDX-License-Identifier: GPL-3.0-only
pub mod doscalls {
    pub fn dos_write(fd: u32, buf: &[u8]) -> Result<u32, &'static str> {
        if fd == 1 || fd == 2 {
            let s = std::str::from_utf8(buf).unwrap_or("<invalid utf8>");
            print!("{}", s);
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            Ok(buf.len() as u32)
        } else {
            Err("Unsupported file descriptor")
        }
    }
}
