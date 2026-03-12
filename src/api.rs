pub mod doscalls {
    pub fn dos_write(fd: u32, buf: &[u8]) -> Result<u32, &'static str> {
        if fd == 1 || fd == 2 {
            let s = std::str::from_utf8(buf).unwrap_or("<invalid utf8>");
            print!("{}", s);
            Ok(buf.len() as u32)
        } else {
            Err("Unsupported file descriptor")
        }
    }

    pub fn dos_exit(_action: u32, result: u32) -> ! {
        std::process::exit(result as i32);
    }
}

// Bridges for OS/2 calling convention
pub mod bridges {
    use super::doscalls;

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn DosWrite(fd: u32, buf_ptr: *const u8, len: u32, actual_ptr: *mut u32) -> u32 {
        let buf = unsafe { std::slice::from_raw_parts(buf_ptr, len as usize) };
        match doscalls::dos_write(fd, buf) {
            Ok(actual) => {
                if !actual_ptr.is_null() { 
                    unsafe { *actual_ptr = actual; }
                }
                0
            },
            Err(_) => 1,
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn DosExit(action: u32, result: u32) -> ! {
        doscalls::dos_exit(action, result);
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn DosQuerySysInfo(_start: u32, _last: u32, _buf: *mut u8, _len: u32) -> u32 {
        0
    }

    // A helper that matches the return address signature
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn WarpineExitThunk() -> ! {
        // OS/2 expects us to call DosExit(1, EAX)
        // For now, just exit success.
        doscalls::dos_exit(1, 0);
    }
}
