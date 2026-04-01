// SPDX-License-Identifier: GPL-3.0-only
//
// UCONV.DLL — Unicode conversion API (OS/2 ULS subset).
//
// Implements the core UniXxx APIs used by OS/2 applications to convert between
// codepage-encoded byte strings and UCS-2 (UTF-16LE) Unicode (UniChar = USHORT).
//
// # Handle semantics
// `UniCreateUconvObject` parses a UCS-2 codepage name (e.g. L"IBM-850"),
// allocates a UCONV_OBJECT handle, and records the codepage in `UconvManager`.
// `UniUconvToUcs`/`UniUconvFromUcs` look up the codepage by handle and delegate
// to `codepage::cp_decode`/`cp_encode`.
//
// # Return codes (from OS/2 <unidef.h>)
//   ULS_SUCCESS     = 0
//   ULS_INVALID     = 1  — null or invalid argument
//   ULS_BADOBJECT   = 2  — invalid UCONV_OBJECT handle
//   ULS_BUFFERFULL  = 8  — output buffer too small
//   ULS_INVALID_CPID = 10 — unrecognised codepage identifier

use std::collections::HashMap;
use tracing::debug;
use super::codepage::{cp_decode, cp_encode};
use super::MutexExt;

// ── Return codes ──────────────────────────────────────────────────────────────

pub const ULS_SUCCESS:      u32 = 0;
pub const ULS_INVALID:      u32 = 1;
pub const ULS_BADOBJECT:    u32 = 2;
pub const ULS_BUFFERFULL:   u32 = 8;
pub const ULS_INVALID_CPID: u32 = 10;

/// Internal sentinel: conversion object created for UTF-8 (no OS/2 codepage number).
const CP_UTF8: u32 = 0xFFFF_FFFE;

// ── UconvManager ─────────────────────────────────────────────────────────────

/// Tracks live UCONV_OBJECT handles → codepage mappings.
pub struct UconvManager {
    objects: HashMap<u32, u32>, // UCONV_OBJECT handle → codepage (CP_UTF8 for UTF-8)
    next_handle: u32,
}

impl Default for UconvManager {
    fn default() -> Self { Self::new() }
}

impl UconvManager {
    pub fn new() -> Self {
        Self { objects: HashMap::new(), next_handle: 0x8001 }
    }

    /// Allocate a new handle for `cp` and return it.
    pub fn create(&mut self, cp: u32) -> u32 {
        let h = self.next_handle;
        self.next_handle = (self.next_handle + 1) | 0x8000;
        self.objects.insert(h, cp);
        h
    }

    /// Look up the codepage for a handle.  Returns `None` for invalid handles.
    pub fn lookup(&self, h: u32) -> Option<u32> {
        self.objects.get(&h).copied()
    }

    /// Free a handle.  Returns `true` if found and removed.
    pub fn free(&mut self, h: u32) -> bool {
        self.objects.remove(&h).is_some()
    }
}

// ── API implementations ───────────────────────────────────────────────────────

impl super::Loader {
    // ── UniCreateUconvObject (ordinal 1) ──────────────────────────────────────
    //
    //   APIRET UniCreateUconvObject(UniChar *ucsName, UCONV_OBJECT *puobj)
    //
    //   args[0] = ucsName  — ptr to UCS-2 (null-terminated) codepage name,
    //                        e.g. L"IBM-850" or L"UTF-8"
    //   args[1] = puobj    — ptr to UCONV_OBJECT (u32) to receive the handle

    pub fn uni_create_uconv_object(&self, ucs_name_ptr: u32, p_uobj: u32) -> u32 {
        if ucs_name_ptr == 0 || p_uobj == 0 { return ULS_INVALID; }

        let name = self.read_guest_ucs2(ucs_name_ptr);
        debug!("UniCreateUconvObject name={:?}", name);

        let cp = match parse_uconv_name(&name) {
            Some(cp) => cp,
            None => {
                debug!("  unrecognised name {:?}", name);
                return ULS_INVALID_CPID;
            }
        };

        let handle = self.shared.uconv_mgr.lock_or_recover().create(cp);
        let _ = self.guest_write::<u32>(p_uobj, handle);
        debug!("  handle=0x{:X} cp={}", handle, cp);
        ULS_SUCCESS
    }

    // ── UniFreeUconvObject (ordinal 2) ────────────────────────────────────────
    //
    //   APIRET UniFreeUconvObject(UCONV_OBJECT uobj)
    //
    //   args[0] = uobj — handle returned by UniCreateUconvObject

    pub fn uni_free_uconv_object(&self, uobj: u32) -> u32 {
        let ok = self.shared.uconv_mgr.lock_or_recover().free(uobj);
        if ok { ULS_SUCCESS } else { ULS_BADOBJECT }
    }

    // ── UniUconvToUcs (ordinal 3) ─────────────────────────────────────────────
    //
    //   APIRET UniUconvToUcs(
    //       UCONV_OBJECT  uobj,
    //       void        **ppInBuf,       ← updated in place
    //       size_t       *pInBytesLeft,  ← updated in place
    //       UniChar     **ppOutBuf,      ← updated in place
    //       size_t       *pOutCharsLeft, ← updated in place
    //       size_t       *pNumSubs       ← updated in place (may be null)
    //   )
    //
    //   args[0] = uobj
    //   args[1] = ppInBuf       (ptr to ptr to input bytes)
    //   args[2] = pInBytesLeft  (ptr to u32 byte count)
    //   args[3] = ppOutBuf      (ptr to ptr to UCS-2 output)
    //   args[4] = pOutCharsLeft (ptr to u32 UniChar count)
    //   args[5] = pNumSubs      (ptr to u32, may be null)

    pub fn uni_uconv_to_ucs(
        &self,
        uobj: u32,
        pp_in: u32,   p_in_left: u32,
        pp_out: u32,  p_out_left: u32,
        p_subs: u32,
    ) -> u32 {
        let cp = match self.shared.uconv_mgr.lock_or_recover().lookup(uobj) {
            Some(cp) => cp,
            None => return ULS_BADOBJECT,
        };

        let in_ptr   = match self.guest_read::<u32>(pp_in)     { Some(v) => v, None => return ULS_INVALID };
        let in_left  = match self.guest_read::<u32>(p_in_left) { Some(v) => v, None => return ULS_INVALID };
        let out_ptr  = match self.guest_read::<u32>(pp_out)    { Some(v) => v, None => return ULS_INVALID };
        let out_left = match self.guest_read::<u32>(p_out_left){ Some(v) => v, None => return ULS_INVALID };

        debug!("UniUconvToUcs uobj=0x{:X} cp={} in=0x{:X} inLeft={} out=0x{:X} outLeft={}",
               uobj, cp, in_ptr, in_left, out_ptr, out_left);

        if in_ptr == 0 || out_ptr == 0 { return ULS_INVALID; }

        // Read input bytes from guest memory.
        let bytes: Vec<u8> = (0..in_left)
            .filter_map(|i| self.guest_read::<u8>(in_ptr + i))
            .collect();

        // Decode to internal UTF-8.
        let text = if cp == CP_UTF8 {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            cp_decode(&bytes, cp)
        };

        // Encode as UCS-2 (UTF-16LE, UniChar = u16).
        let ucs2: Vec<u16> = text.encode_utf16().collect();

        if ucs2.len() > out_left as usize {
            return ULS_BUFFERFULL;
        }

        for (i, &wc) in ucs2.iter().enumerate() {
            let _ = self.guest_write::<u16>(out_ptr + (i as u32 * 2), wc);
        }

        // Update caller's pointers and counts.
        let consumed = bytes.len() as u32;
        let produced = ucs2.len() as u32;
        let _ = self.guest_write::<u32>(pp_in,   in_ptr  + consumed);
        let _ = self.guest_write::<u32>(p_in_left, 0);
        let _ = self.guest_write::<u32>(pp_out,  out_ptr + produced * 2);
        let _ = self.guest_write::<u32>(p_out_left, out_left - produced);
        if p_subs != 0 { let _ = self.guest_write::<u32>(p_subs, 0); }

        ULS_SUCCESS
    }

    // ── UniUconvFromUcs (ordinal 4) ───────────────────────────────────────────
    //
    //   APIRET UniUconvFromUcs(
    //       UCONV_OBJECT  uobj,
    //       UniChar     **ppInBuf,       ← updated in place
    //       size_t       *pInCharsLeft,  ← updated in place
    //       void        **ppOutBuf,      ← updated in place
    //       size_t       *pOutBytesLeft, ← updated in place
    //       size_t       *pNumSubs       ← updated in place (may be null)
    //   )
    //
    //   args[0] = uobj
    //   args[1] = ppInBuf        (ptr to ptr to UCS-2 input)
    //   args[2] = pInCharsLeft   (ptr to u32 UniChar count)
    //   args[3] = ppOutBuf       (ptr to ptr to output bytes)
    //   args[4] = pOutBytesLeft  (ptr to u32 byte count)
    //   args[5] = pNumSubs       (ptr to u32, may be null)

    pub fn uni_uconv_from_ucs(
        &self,
        uobj: u32,
        pp_in: u32,   p_in_left: u32,
        pp_out: u32,  p_out_left: u32,
        p_subs: u32,
    ) -> u32 {
        let cp = match self.shared.uconv_mgr.lock_or_recover().lookup(uobj) {
            Some(cp) => cp,
            None => return ULS_BADOBJECT,
        };

        let in_ptr   = match self.guest_read::<u32>(pp_in)     { Some(v) => v, None => return ULS_INVALID };
        let in_left  = match self.guest_read::<u32>(p_in_left) { Some(v) => v, None => return ULS_INVALID };
        let out_ptr  = match self.guest_read::<u32>(pp_out)    { Some(v) => v, None => return ULS_INVALID };
        let out_left = match self.guest_read::<u32>(p_out_left){ Some(v) => v, None => return ULS_INVALID };

        debug!("UniUconvFromUcs uobj=0x{:X} cp={} in=0x{:X} inLeft={} out=0x{:X} outLeft={}",
               uobj, cp, in_ptr, in_left, out_ptr, out_left);

        if in_ptr == 0 || out_ptr == 0 { return ULS_INVALID; }

        // Read UCS-2 (u16 LE) input from guest memory.
        let wchars: Vec<u16> = (0..in_left)
            .filter_map(|i| self.guest_read::<u16>(in_ptr + i * 2))
            .collect();

        // Decode UCS-2 to internal UTF-8.
        let text = String::from_utf16_lossy(&wchars);

        // Encode to codepage bytes.
        let bytes = if cp == CP_UTF8 {
            text.into_bytes()
        } else {
            cp_encode(&text, cp)
        };

        if bytes.len() > out_left as usize {
            return ULS_BUFFERFULL;
        }

        self.guest_write_bytes(out_ptr, &bytes);

        let consumed = wchars.len() as u32;
        let produced = bytes.len() as u32;
        let _ = self.guest_write::<u32>(pp_in,   in_ptr  + consumed * 2);
        let _ = self.guest_write::<u32>(p_in_left, 0);
        let _ = self.guest_write::<u32>(pp_out,  out_ptr + produced);
        let _ = self.guest_write::<u32>(p_out_left, out_left - produced);
        if p_subs != 0 { let _ = self.guest_write::<u32>(p_subs, 0); }

        ULS_SUCCESS
    }

    // ── UniMapCpToUcsCp (ordinal 6) ───────────────────────────────────────────
    //
    //   APIRET UniMapCpToUcsCp(ULONG ulCodePage, UniChar *ucsCodePage, size_t n)
    //
    //   args[0] = ulCodePage   — OS/2 codepage number (e.g. 850)
    //   args[1] = ucsCodePage  — ptr to UCS-2 output buffer
    //   args[2] = n            — max UniChars in output buffer (including NUL)

    pub fn uni_map_cp_to_ucs_cp(&self, cp: u32, out_ptr: u32, max_chars: u32) -> u32 {
        if out_ptr == 0 { return ULS_INVALID; }

        let name = cp_to_ibm_name(cp);
        let ucs2: Vec<u16> = name.encode_utf16().collect();

        if (ucs2.len() + 1) as u32 > max_chars { return ULS_BUFFERFULL; }

        for (i, &wc) in ucs2.iter().enumerate() {
            let _ = self.guest_write::<u16>(out_ptr + (i as u32 * 2), wc);
        }
        let _ = self.guest_write::<u16>(out_ptr + (ucs2.len() as u32 * 2), 0);
        ULS_SUCCESS
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Read a null-terminated UCS-2 string from guest memory.
    fn read_guest_ucs2(&self, ptr: u32) -> String {
        let mut wchars = Vec::new();
        let mut off = ptr;
        loop {
            match self.guest_read::<u16>(off) {
                Some(0) | None => break,
                Some(wc) => { wchars.push(wc); off += 2; }
            }
            if wchars.len() > 512 { break; } // safety limit
        }
        String::from_utf16_lossy(&wchars)
    }
}

// ── Free-standing helpers ─────────────────────────────────────────────────────

/// Parse a UCS-2 codepage name (already decoded to `&str`) into a warpine
/// codepage number.  Returns `None` for unrecognised names.
///
/// Accepted forms (all case-insensitive):
///   - `"IBM-437"`, `"IBM-850"`, … (IBM-NNN / IBM-NNNN)
///   - `"IBM437"`, `"IBM850"`, …
///   - `"CP437"`, `"CP-437"`, …
///   - `"UTF-8"`, `"UTF8"` → CP_UTF8 sentinel
fn parse_uconv_name(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    let s = upper.trim();

    if s == "UTF-8" || s == "UTF8" {
        return Some(CP_UTF8);
    }

    // Strip "IBM-", "IBM", "CP-", or "CP" prefix, then parse the numeric suffix.
    let digits = if let Some(rest) = s.strip_prefix("IBM-") {
        rest
    } else if let Some(rest) = s.strip_prefix("IBM") {
        rest
    } else if let Some(rest) = s.strip_prefix("CP-") {
        rest
    } else if let Some(rest) = s.strip_prefix("CP") {
        rest
    } else {
        s
    };

    let cp: u32 = digits.trim().parse().ok()?;

    // Validate: only accept codepages warpine can actually convert.
    match cp {
        437 | 850 | 852 | 932 | 936 | 949 | 950 |
        1250 | 1251 | 1252 | 1253 | 1254 | 1255 | 1256 | 1257 | 1258 => Some(cp),
        _ => None,
    }
}

/// Map a warpine codepage number to its canonical IBM name string,
/// used as the UCS-2 codepage identifier in UniMapCpToUcsCp.
fn cp_to_ibm_name(cp: u32) -> String {
    if cp == CP_UTF8 { "UTF-8".to_string() } else { format!("IBM-{}", cp) }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Loader;

    /// Write a null-terminated UCS-2 string into guest memory at `ptr`.
    fn write_ucs2(loader: &Loader, ptr: u32, s: &str) {
        let wchars: Vec<u16> = s.encode_utf16().collect();
        for (i, &wc) in wchars.iter().enumerate() {
            loader.guest_write::<u16>(ptr + i as u32 * 2, wc);
        }
        // Null terminator
        loader.guest_write::<u16>(ptr + wchars.len() as u32 * 2, 0);
    }

    fn alloc(loader: &Loader, bytes: u32) -> u32 {
        loader.shared.mem_mgr.lock_or_recover().alloc(bytes).expect("alloc failed")
    }

    // ── UniCreateUconvObject ──────────────────────────────────────────────────

    #[test]
    fn test_uni_create_ibm850() {
        let loader = Loader::new_mock();
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-850");
        let h_buf = alloc(&loader, 4);

        let rc = loader.uni_create_uconv_object(name_buf, h_buf);
        assert_eq!(rc, ULS_SUCCESS);

        let handle = loader.guest_read::<u32>(h_buf).unwrap();
        assert!(handle != 0, "handle must be nonzero");

        let cp = loader.shared.uconv_mgr.lock_or_recover().lookup(handle);
        assert_eq!(cp, Some(850));
    }

    #[test]
    fn test_uni_create_utf8() {
        let loader = Loader::new_mock();
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "UTF-8");
        let h_buf = alloc(&loader, 4);

        assert_eq!(loader.uni_create_uconv_object(name_buf, h_buf), ULS_SUCCESS);
        let handle = loader.guest_read::<u32>(h_buf).unwrap();
        let cp = loader.shared.uconv_mgr.lock_or_recover().lookup(handle);
        assert_eq!(cp, Some(CP_UTF8));
    }

    #[test]
    fn test_uni_create_invalid_name_returns_error() {
        let loader = Loader::new_mock();
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "BOGUS-CPNAME");
        let h_buf = alloc(&loader, 4);

        assert_eq!(loader.uni_create_uconv_object(name_buf, h_buf), ULS_INVALID_CPID);
    }

    #[test]
    fn test_uni_create_null_ptrs_return_invalid() {
        let loader = Loader::new_mock();
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-437");
        let h_buf = alloc(&loader, 4);

        assert_eq!(loader.uni_create_uconv_object(0, h_buf), ULS_INVALID);
        assert_eq!(loader.uni_create_uconv_object(name_buf, 0), ULS_INVALID);
    }

    // ── UniFreeUconvObject ────────────────────────────────────────────────────

    #[test]
    fn test_uni_free_success_and_double_free() {
        let loader = Loader::new_mock();
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-437");
        let h_buf = alloc(&loader, 4);
        loader.uni_create_uconv_object(name_buf, h_buf);
        let handle = loader.guest_read::<u32>(h_buf).unwrap();

        assert_eq!(loader.uni_free_uconv_object(handle), ULS_SUCCESS);
        // After free the handle is gone — double-free returns BADOBJECT.
        assert_eq!(loader.uni_free_uconv_object(handle), ULS_BADOBJECT);
    }

    #[test]
    fn test_uni_free_invalid_handle() {
        let loader = Loader::new_mock();
        assert_eq!(loader.uni_free_uconv_object(0xDEADBEEF), ULS_BADOBJECT);
    }

    // ── UniUconvToUcs ─────────────────────────────────────────────────────────

    #[test]
    fn test_uni_uconv_to_ucs_cp850() {
        let loader = Loader::new_mock();

        // Create CP850 conversion object.
        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-850");
        let h_buf = alloc(&loader, 4);
        loader.uni_create_uconv_object(name_buf, h_buf);
        let uobj = loader.guest_read::<u32>(h_buf).unwrap();

        // Input: "caf\x82" — 0x82 is 'é' in CP850.
        let in_data = alloc(&loader, 8);
        loader.guest_write_bytes(in_data, b"caf\x82");

        // Output buffer (4 UniChars = 8 bytes).
        let out_data = alloc(&loader, 16);

        // Set up double-pointer indirection buffers.
        let pp_in  = alloc(&loader, 4); loader.guest_write::<u32>(pp_in,  in_data);
        let in_left = alloc(&loader, 4); loader.guest_write::<u32>(in_left, 4);
        let pp_out = alloc(&loader, 4); loader.guest_write::<u32>(pp_out, out_data);
        let out_left = alloc(&loader, 4); loader.guest_write::<u32>(out_left, 8);

        let rc = loader.uni_uconv_to_ucs(uobj, pp_in, in_left, pp_out, out_left, 0);
        assert_eq!(rc, ULS_SUCCESS);

        // "café" in UCS-2 LE: 'c'=0x0063, 'a'=0x0061, 'f'=0x0066, 'é'=0x00E9.
        assert_eq!(loader.guest_read::<u16>(out_data),     Some(0x0063)); // 'c'
        assert_eq!(loader.guest_read::<u16>(out_data + 2), Some(0x0061)); // 'a'
        assert_eq!(loader.guest_read::<u16>(out_data + 4), Some(0x0066)); // 'f'
        assert_eq!(loader.guest_read::<u16>(out_data + 6), Some(0x00E9)); // 'é'

        // Remaining output chars updated: 8 - 4 = 4.
        assert_eq!(loader.guest_read::<u32>(out_left), Some(4));
        // Input fully consumed.
        assert_eq!(loader.guest_read::<u32>(in_left), Some(0));
    }

    #[test]
    fn test_uni_uconv_to_ucs_ascii_passthrough() {
        let loader = Loader::new_mock();

        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-437");
        let h_buf = alloc(&loader, 4);
        loader.uni_create_uconv_object(name_buf, h_buf);
        let uobj = loader.guest_read::<u32>(h_buf).unwrap();

        let in_data = alloc(&loader, 8);
        loader.guest_write_bytes(in_data, b"Hi");
        let out_data = alloc(&loader, 16);

        let pp_in   = alloc(&loader, 4); loader.guest_write::<u32>(pp_in,   in_data);
        let in_left = alloc(&loader, 4); loader.guest_write::<u32>(in_left,  2);
        let pp_out  = alloc(&loader, 4); loader.guest_write::<u32>(pp_out,  out_data);
        let out_left = alloc(&loader, 4); loader.guest_write::<u32>(out_left, 4);

        assert_eq!(loader.uni_uconv_to_ucs(uobj, pp_in, in_left, pp_out, out_left, 0), ULS_SUCCESS);
        assert_eq!(loader.guest_read::<u16>(out_data),     Some(b'H' as u16));
        assert_eq!(loader.guest_read::<u16>(out_data + 2), Some(b'i' as u16));
    }

    // ── UniUconvFromUcs ───────────────────────────────────────────────────────

    #[test]
    fn test_uni_uconv_from_ucs_cp850() {
        let loader = Loader::new_mock();

        let name_buf = alloc(&loader, 32);
        write_ucs2(&loader, name_buf, "IBM-850");
        let h_buf = alloc(&loader, 4);
        loader.uni_create_uconv_object(name_buf, h_buf);
        let uobj = loader.guest_read::<u32>(h_buf).unwrap();

        // Input UCS-2: "café" (4 UniChars).
        let in_data = alloc(&loader, 16);
        for (i, &wc) in [0x0063u16, 0x0061, 0x0066, 0x00E9].iter().enumerate() {
            loader.guest_write::<u16>(in_data + i as u32 * 2, wc);
        }

        let out_data = alloc(&loader, 8);

        let pp_in   = alloc(&loader, 4); loader.guest_write::<u32>(pp_in,   in_data);
        let in_left = alloc(&loader, 4); loader.guest_write::<u32>(in_left,  4);
        let pp_out  = alloc(&loader, 4); loader.guest_write::<u32>(pp_out,  out_data);
        let out_left = alloc(&loader, 4); loader.guest_write::<u32>(out_left, 8);

        assert_eq!(loader.uni_uconv_from_ucs(uobj, pp_in, in_left, pp_out, out_left, 0), ULS_SUCCESS);

        // "café" in CP850: 'c'=0x63, 'a'=0x61, 'f'=0x66, 'é'=0x82.
        assert_eq!(loader.guest_read::<u8>(out_data),     Some(0x63)); // 'c'
        assert_eq!(loader.guest_read::<u8>(out_data + 1), Some(0x61)); // 'a'
        assert_eq!(loader.guest_read::<u8>(out_data + 2), Some(0x66)); // 'f'
        assert_eq!(loader.guest_read::<u8>(out_data + 3), Some(0x82)); // 'é' in CP850
    }

    #[test]
    fn test_uni_uconv_from_ucs_bad_handle() {
        let loader = Loader::new_mock();
        let dummy = alloc(&loader, 16);
        loader.guest_write::<u32>(dummy, dummy + 4); // pp_in points into dummy area
        let in_left = alloc(&loader, 4); loader.guest_write::<u32>(in_left, 0);
        let pp_out  = alloc(&loader, 4); loader.guest_write::<u32>(pp_out, dummy + 8);
        let out_left = alloc(&loader, 4); loader.guest_write::<u32>(out_left, 8);

        assert_eq!(
            loader.uni_uconv_from_ucs(0xDEAD, dummy, in_left, pp_out, out_left, 0),
            ULS_BADOBJECT
        );
    }

    // ── UniMapCpToUcsCp ───────────────────────────────────────────────────────

    #[test]
    fn test_uni_map_cp_to_ucs_cp_850() {
        let loader = Loader::new_mock();
        let buf = alloc(&loader, 32);
        assert_eq!(loader.uni_map_cp_to_ucs_cp(850, buf, 16), ULS_SUCCESS);

        // Read back the UCS-2 string and decode.
        let mut wchars = Vec::new();
        for i in 0..16 {
            let wc = loader.guest_read::<u16>(buf + i * 2).unwrap();
            if wc == 0 { break; }
            wchars.push(wc);
        }
        let s = String::from_utf16_lossy(&wchars);
        assert_eq!(s, "IBM-850");
    }

    #[test]
    fn test_uni_map_cp_to_ucs_cp_null_buf() {
        let loader = Loader::new_mock();
        assert_eq!(loader.uni_map_cp_to_ucs_cp(437, 0, 16), ULS_INVALID);
    }

    // ── parse_uconv_name ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_uconv_name_ibm_forms() {
        assert_eq!(parse_uconv_name("IBM-850"),  Some(850));
        assert_eq!(parse_uconv_name("IBM850"),   Some(850));
        assert_eq!(parse_uconv_name("IBM-437"),  Some(437));
        assert_eq!(parse_uconv_name("ibm-1252"), Some(1252)); // case-insensitive
        assert_eq!(parse_uconv_name("CP850"),    Some(850));
        assert_eq!(parse_uconv_name("CP-437"),   Some(437));
    }

    #[test]
    fn test_parse_uconv_name_utf8() {
        assert_eq!(parse_uconv_name("UTF-8"),  Some(CP_UTF8));
        assert_eq!(parse_uconv_name("UTF8"),   Some(CP_UTF8));
        assert_eq!(parse_uconv_name("utf-8"),  Some(CP_UTF8));
    }

    #[test]
    fn test_parse_uconv_name_invalid() {
        assert_eq!(parse_uconv_name("BOGUS"),     None);
        assert_eq!(parse_uconv_name("IBM-999"),   None); // unsupported codepage
        assert_eq!(parse_uconv_name(""),           None);
        assert_eq!(parse_uconv_name("IBM-"),       None);
    }
}
