// SPDX-License-Identifier: GPL-3.0-or-later
pub mod doscalls {
    /// Write `buf`, text in codepage `cp`, to the host's standard output
    /// (`fd` 1) or error (2) as UTF-8. A failed write is an error: a reader
    /// that closed its pipe must reach the program as a broken pipe, not as
    /// bytes written. A double-byte character split across two writes is
    /// decoded in two halves.
    pub fn dos_write(fd: u32, buf: &[u8], cp: u32) -> Result<u32, std::io::ErrorKind> {
        if fd == 1 || fd == 2 {
            use std::io::Write;
            let out = utf8_of(buf, cp);
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

    /// `buf`, text in codepage `cp`, as UTF-8 for the host: ASCII as it is,
    /// the other bytes as that codepage's characters.
    pub fn utf8_of(buf: &[u8], cp: u32) -> String {
        crate::loader::codepage::cp_decode(buf, cp)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn host_output_follows_the_process_codepage() {
            assert_eq!(super::utf8_of(b"a\x82", 437), "aé");
            assert_eq!(super::utf8_of(b"\x9b", 437), "¢");
            assert_eq!(super::utf8_of(b"\x9b", 850), "ø", "not CP437's");
        }
    }
}
