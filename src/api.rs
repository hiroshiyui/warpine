pub mod doscalls {
    pub fn dos_write(fd: u32, buf: &[u8]) -> Result<u32, &'static str> {
        // Simple stub redirecting to native stdout/stderr
        if fd == 1 || fd == 2 {
            let s = std::str::from_utf8(buf).unwrap_or("<invalid utf8>");
            print!("{}", s);
            Ok(buf.len() as u32)
        } else {
            Err("Unsupported file descriptor for now")
        }
    }

    pub fn dos_exit(_action: u32, result: u32) -> ! {
        std::process::exit(result as i32);
    }
}
