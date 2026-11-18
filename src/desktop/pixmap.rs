// SPDX-License-Identifier: GPL-3.0-only

//! Shared pixel buffers between a Warpine process and the desktop server
//! (ADR-0052).
//!
//! A [`SharedPixmap`] is a sealed memfd holding `width × height` XRGB pels,
//! top row first. The client creates it, maps it read-write and draws a
//! frame's finished picture into it; the server receives the descriptor
//! over the desktop socket, checks it (a sealed memfd of exactly the
//! announced size) and maps it read-only.
//!
//! Every access copies through bounds-checked methods; no reference into the
//! mapping is ever handed out. The other process may write while the server
//! reads: a torn read only shows a half-updated picture for one composite,
//! and the size cannot change under either side (the memfd is sealed against
//! shrinking and growing), so no access can fault.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

/// Largest pixmap side, as for drawing surfaces (`gui::surface::MAX_SURFACE_DIM`).
pub const MAX_PIXMAP_DIM: u32 = 8192;

/// The seals a pixmap carries: its size is fixed and no further seals can
/// change that.
const PIXMAP_SEALS: libc::c_int = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;

/// A pixel buffer in a sealed memfd, mapped into this process.
pub struct SharedPixmap {
    fd: OwnedFd,
    ptr: *mut u32,
    width: u32,
    height: u32,
    /// Bytes mapped (`width * height * 4`).
    len: usize,
    writable: bool,
}

// SAFETY: the mapping belongs to this value alone and is only reached
// through methods that copy within its bounds; moving it to another thread
// or sharing it is no different from sharing a `Vec<u32>` behind `&self`
// methods that copy (writes go through `&self` like the other process's
// writes do; readers tolerate them).
unsafe impl Send for SharedPixmap {}
// SAFETY: see `Send`; every method copies through raw pointers within the
// checked bounds and never creates references into the mapping.
unsafe impl Sync for SharedPixmap {}

/// Bytes of a `width × height` pixmap, or `None` if a side is zero or too
/// large.
fn byte_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > MAX_PIXMAP_DIM || height > MAX_PIXMAP_DIM {
        return None;
    }
    (width as usize).checked_mul(height as usize)?.checked_mul(4)
}

fn invalid(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string())
}

impl SharedPixmap {
    /// A new, black `width × height` pixmap, mapped read-write.
    pub fn create(width: u32, height: u32) -> std::io::Result<SharedPixmap> {
        let len = byte_len(width, height).ok_or_else(|| invalid("pixmap size out of range"))?;
        // SAFETY: the name is a NUL-terminated literal; memfd_create has no
        // other preconditions.
        let raw = unsafe {
            libc::memfd_create(c"warpine-pixmap".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
        };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: memfd_create returned a new descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: plain syscalls on the descriptor owned above.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: as above.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, PIXMAP_SEALS) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Self::map(fd, width, height, len, true)
    }

    /// Map a pixmap another process created, read-only, after checking that
    /// `fd` is a memfd sealed against resizing whose size is exactly that
    /// of a `width × height` pixmap.
    pub fn open(fd: OwnedFd, width: u32, height: u32) -> std::io::Result<SharedPixmap> {
        let len = byte_len(width, height).ok_or_else(|| invalid("pixmap size out of range"))?;
        // SAFETY: plain fcntl on a descriptor this function owns.
        let seals = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) };
        let needed = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW;
        if seals < 0 || seals & needed != needed {
            return Err(invalid("pixmap is not a sealed memfd"));
        }
        let size = std::fs::File::from(fd.try_clone()?).metadata()?.len();
        if size != len as u64 {
            return Err(invalid("pixmap size does not match"));
        }
        Self::map(fd, width, height, len, false)
    }

    fn map(fd: OwnedFd, width: u32, height: u32, len: usize, writable: bool) -> std::io::Result<SharedPixmap> {
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ };
        // SAFETY: a fresh shared mapping of `len` bytes of a file at least
        // that long (its size was set or checked above and is sealed); the
        // kernel picks the address, so no existing memory is replaced.
        let raw = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if raw == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(SharedPixmap { fd, ptr: raw as *mut u32, width, height, len, writable })
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }

    /// The memfd, to pass to the other process.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// The index of pel `(x, y)` if a run of `n` pels starting there lies
    /// inside one row of the pixmap.
    fn span(&self, x: i64, y: i64, n: usize) -> Option<usize> {
        let (w, h) = (self.width as i64, self.height as i64);
        if x < 0 || y < 0 || y >= h || x.checked_add(n as i64)? > w {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// Copy `pels` into row `y` from column `x` on. Fails (copying nothing)
    /// if the run does not fit in the row or the pixmap is read-only.
    pub fn write_row(&self, x: i64, y: i64, pels: &[u32]) -> bool {
        if !self.writable { return false; }
        let Some(start) = self.span(x, y, pels.len()) else { return false };
        // SAFETY: `span` checked that `start .. start + len` lies inside the
        // mapping, which is writable and lives as long as `self`; `pels` is
        // ordinary memory of this process, so the two cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(pels.as_ptr(), self.ptr.add(start), pels.len()); }
        true
    }

    /// Copy `out.len()` pels of row `y` from column `x` on into `out`.
    /// Fails (leaving `out` alone) if the run does not fit in the row.
    pub fn read_row(&self, x: i64, y: i64, out: &mut [u32]) -> bool {
        let Some(start) = self.span(x, y, out.len()) else { return false };
        // SAFETY: as in `write_row`; the source may be written by the other
        // process meanwhile, which can only make the copied pels stale.
        unsafe { std::ptr::copy_nonoverlapping(self.ptr.add(start), out.as_mut_ptr(), out.len()); }
        true
    }
}

impl Drop for SharedPixmap {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping made in `map`, which nothing
        // references any more.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len); }
    }
}

impl std::fmt::Debug for SharedPixmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedPixmap({}x{}, {})", self.width, self.height, if self.writable { "rw" } else { "ro" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixmap_is_shared_with_a_read_only_mapping_of_its_descriptor() {
        let p = SharedPixmap::create(4, 3).unwrap();
        assert!(p.write_row(1, 2, &[7, 8, 9]));
        let fd = p.fd().try_clone_to_owned().unwrap();
        let q = SharedPixmap::open(fd, 4, 3).unwrap();
        let mut row = [0u32; 4];
        assert!(q.read_row(0, 2, &mut row));
        assert_eq!(row, [0, 7, 8, 9]);
        // Later writes show through the other mapping.
        assert!(p.write_row(0, 0, &[5]));
        let mut one = [0u32; 1];
        assert!(q.read_row(0, 0, &mut one));
        assert_eq!(one, [5]);
        // The read-only side cannot write.
        assert!(!q.write_row(0, 0, &[1]));
    }

    #[test]
    fn runs_outside_a_row_are_refused() {
        let p = SharedPixmap::create(4, 3).unwrap();
        assert!(!p.write_row(2, 0, &[1, 2, 3]), "past the right edge");
        assert!(!p.write_row(-1, 0, &[1]));
        assert!(!p.write_row(0, 3, &[1]), "below the last row");
        let mut out = [9u32; 5];
        assert!(!p.read_row(0, 0, &mut out), "longer than a row");
        assert_eq!(out, [9; 5]);
        assert!(p.read_row(0, 2, &mut out[..4]));
    }

    #[test]
    fn sizes_out_of_range_are_refused() {
        assert!(SharedPixmap::create(0, 1).is_err());
        assert!(SharedPixmap::create(MAX_PIXMAP_DIM + 1, 1).is_err());
    }

    #[test]
    fn only_a_sealed_memfd_of_the_announced_size_is_opened() {
        let p = SharedPixmap::create(4, 3).unwrap();
        let fd = || p.fd().try_clone_to_owned().unwrap();
        assert!(SharedPixmap::open(fd(), 4, 4).is_err(), "wrong size");
        assert!(SharedPixmap::open(fd(), 4, 3).is_ok());
        // Not a memfd at all: no seals.
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(SharedPixmap::open(OwnedFd::from(file), 1, 1).is_err());
    }
}
