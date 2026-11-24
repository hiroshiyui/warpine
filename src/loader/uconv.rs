// SPDX-License-Identifier: GPL-3.0-or-later
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
// # Return codes (<ulserrno.h>: ULS_API_ERROR_BASE 0x20400 plus an index)
//   ULS_SUCCESS          0
//   ULS_ILLEGALSEQUENCE  0x20402 — a sequence the codepage cannot hold
//   ULS_INVALID          0x2040E — null or invalid argument, unknown codepage name
//   ULS_BADOBJECT        0x2040F — invalid UconvObject
//   ULS_BUFFERFULL       0x20412 — output buffer too small
//   ULS_BADATTR          0x20415 — UniSetUconvObject's attributes refused

use std::collections::HashMap;
use tracing::debug;
use super::codepage::{cp_decode, cp_encode};
use super::MutexExt;

// ── Return codes ──────────────────────────────────────────────────────────────

pub const ULS_SUCCESS:      u32 = 0;
const ULS_API_ERROR_BASE:   u32 = 0x0002_0400;
pub const ULS_ILLEGALSEQUENCE: u32 = ULS_API_ERROR_BASE | 2;
pub const ULS_INVALID:      u32 = ULS_API_ERROR_BASE | 14;
pub const ULS_BADOBJECT:    u32 = ULS_API_ERROR_BASE | 15;
pub const ULS_BUFFERFULL:   u32 = ULS_API_ERROR_BASE | 18;
pub const ULS_BADATTR:      u32 = ULS_API_ERROR_BASE | 21;

/// Internal sentinel: conversion object created for UTF-8 (no OS/2 codepage number).
const CP_UTF8: u32 = 0xFFFF_FFFE;

// ── UconvManager ─────────────────────────────────────────────────────────────

/// The attributes `UniSetUconvObject` may change (`uconv_attribute_t`);
/// Warpine records them for `UniQueryUconvObject`, its conversions do not
/// act on them yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UconvAttrs {
    pub options: u8,
    pub state: u8,
    /// `conv_endian_t`: source, target.
    pub endian: (u16, u16),
    pub displaymask: u32,
    pub converttype: u32,
    pub subchar: Vec<u8>,
    pub subuni: Vec<u16>,
}

/// `UCONV_OPTION_SUBSTITUTE_FROM_UNICODE`: the default substitution.
const UCONV_OPTION_SUBSTITUTE_FROM_UNICODE: u8 = 1;
/// `DSPMASK_DATA`: every control character is data.
const DSPMASK_DATA: u32 = 0xFFFF_FFFF;
/// The byte `cp_encode` puts for a character the codepage lacks, and the
/// Unicode replacement character.
const DEFAULT_SUBCHAR: u8 = b'?';
const DEFAULT_SUBUNI: u16 = 0xFFFD;

impl Default for UconvAttrs {
    fn default() -> Self {
        UconvAttrs {
            options: UCONV_OPTION_SUBSTITUTE_FROM_UNICODE, state: 0, endian: (0, 0),
            displaymask: DSPMASK_DATA, converttype: 0,
            subchar: vec![DEFAULT_SUBCHAR], subuni: vec![DEFAULT_SUBUNI],
        }
    }
}

/// Tracks live UCONV_OBJECT handles → codepage mappings.
pub struct UconvManager {
    objects: HashMap<u32, u32>, // UCONV_OBJECT handle → codepage (CP_UTF8 for UTF-8)
    attrs: HashMap<u32, UconvAttrs>,
    next_handle: u32,
}

impl Default for UconvManager {
    fn default() -> Self { Self::new() }
}

impl UconvManager {
    pub fn new() -> Self {
        Self { objects: HashMap::new(), attrs: HashMap::new(), next_handle: 0x8001 }
    }

    /// Allocate a new handle for `cp` and return it.
    pub fn create(&mut self, cp: u32) -> u32 {
        let h = self.next_handle;
        self.next_handle = (self.next_handle + 1) | 0x8000;
        self.objects.insert(h, cp);
        self.attrs.insert(h, UconvAttrs::default());
        h
    }

    /// Look up the codepage for a handle.  Returns `None` for invalid handles.
    pub fn lookup(&self, h: u32) -> Option<u32> {
        self.objects.get(&h).copied()
    }

    /// Free a handle.  Returns `true` if found and removed.
    pub fn free(&mut self, h: u32) -> bool {
        self.attrs.remove(&h);
        self.objects.remove(&h).is_some()
    }

    /// The attributes of a handle.
    pub fn attrs(&self, h: u32) -> Option<&UconvAttrs> {
        self.attrs.get(&h)
    }

    /// Replace the attributes of a handle; false for an unknown one.
    pub fn set_attrs(&mut self, h: u32, a: UconvAttrs) -> bool {
        self.attrs.get_mut(&h).map(|slot| *slot = a).is_some()
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
                return ULS_INVALID;
            }
        };

        let handle = self.shared.uconv_mgr.lock_or_recover().create(cp);
        let _ = self.guest_write::<u32>(p_uobj, handle);
        debug!("  handle=0x{:X} cp={}", handle, cp);
        ULS_SUCCESS
    }

    // ── UniFreeUconvObject (ordinal 4) ────────────────────────────────────────
    //
    //   APIRET UniFreeUconvObject(UCONV_OBJECT uobj)
    //
    //   args[0] = uobj — handle returned by UniCreateUconvObject

    pub fn uni_free_uconv_object(&self, uobj: u32) -> u32 {
        let ok = self.shared.uconv_mgr.lock_or_recover().free(uobj);
        if ok { ULS_SUCCESS } else { ULS_BADOBJECT }
    }

    // ── UniUconvToUcs (ordinal 2) ─────────────────────────────────────────────
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
            .map_while(|i| self.guest_read::<u8>(in_ptr.checked_add(i)?))
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

    // ── UniUconvFromUcs (ordinal 3) ───────────────────────────────────────────
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

    // ── UniMapCpToUcsCp (ordinal 10)──────────────────────────────────────────
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

    // ── UniStrToUcs (ordinal 12) ──────────────────────────────────────────────
    //
    //   int UniStrToUcs(UconvObject uobj, UniChar *ucsbuf, char *str, int size)
    //
    // The NUL-terminated `str`, its NUL included, as UniChars in `ucsbuf`
    // (room for `size` UniChars); a short buffer gets what fits, cut before
    // the character that would not, and `ULS_BUFFERFULL`.

    pub fn uni_str_to_ucs(&self, uobj: u32, ucsbuf: u32, str_ptr: u32, size: u32) -> u32 {
        let Some(cp) = self.shared.uconv_mgr.lock_or_recover().lookup(uobj) else { return ULS_BADOBJECT };
        if ucsbuf == 0 || str_ptr == 0 { return ULS_INVALID; }
        let Some(bytes) = self.uconv_nul_string(str_ptr) else { return ULS_INVALID };
        let text = if cp == CP_UTF8 { String::from_utf8_lossy(&bytes).into_owned() } else { cp_decode(&bytes, cp) };
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        let room = size as usize;
        let fits = units.len() <= room;
        for (i, u) in units.iter().take(room).enumerate() {
            if self.guest_write::<u16>(ucsbuf + 2 * i as u32, *u).is_none() { return ULS_INVALID; }
        }
        debug!("UniStrToUcs uobj=0x{uobj:X} cp={cp} -> {} UniChars", units.len());
        if fits { ULS_SUCCESS } else { ULS_BUFFERFULL }
    }

    // ── UniStrFromUcs (ordinal 11) ────────────────────────────────────────────
    //
    //   int UniStrFromUcs(UconvObject uobj, char *outbuf, UniChar *ucsstr, int size)
    //
    // The NUL-terminated `ucsstr`, its NUL included, in the object's codepage
    // (`size` bytes of room), always substituting what the codepage lacks; a
    // short buffer gets the characters that fit whole and `ULS_BUFFERFULL`.

    pub fn uni_str_from_ucs(&self, uobj: u32, outbuf: u32, ucsstr: u32, size: u32) -> u32 {
        let Some(cp) = self.shared.uconv_mgr.lock_or_recover().lookup(uobj) else { return ULS_BADOBJECT };
        if outbuf == 0 || ucsstr == 0 { return ULS_INVALID; }
        let Some(units) = self.uconv_nul_ucs2(ucsstr) else { return ULS_INVALID };
        let mut out = Vec::new();
        let mut full = false;
        for ch in char::decode_utf16(units.iter().copied().chain(std::iter::once(0))) {
            let ch = ch.unwrap_or(char::REPLACEMENT_CHARACTER);
            let mut tmp = [0u8; 4];
            let piece: Vec<u8> = if cp == CP_UTF8 { ch.encode_utf8(&mut tmp).as_bytes().to_vec() } else { cp_encode(ch.encode_utf8(&mut tmp), cp) };
            if out.len() + piece.len() > size as usize { full = true; break; }
            out.extend_from_slice(&piece);
        }
        if self.guest_write_bytes(outbuf, &out).is_none() { return ULS_INVALID; }
        if full { ULS_BUFFERFULL } else { ULS_SUCCESS }
    }

    // ── UniQueryUconvObject (ordinal 7) / UniSetUconvObject (ordinal 8) ───────
    //
    //   int UniQueryUconvObject(UconvObject uobj, uconv_attribute_t *attr,
    //       size_t size, char first[256], char other[256], udcrange_t udcrange[32])
    //   int UniSetUconvObject(UconvObject uobj, uconv_attribute_t *attr)
    //
    // A NULL `attr`, `first`, `other` or `udcrange` is not filled; `size`
    // must hold the version-0 structure.

    pub fn uni_query_uconv_object(&self, uobj: u32, attr: u32, size: u32, first: u32, other: u32, udcrange: u32) -> u32 {
        let (cp, attrs) = {
            let mgr = self.shared.uconv_mgr.lock_or_recover();
            match (mgr.lookup(uobj), mgr.attrs(uobj)) {
                (Some(cp), Some(a)) => (cp, a.clone()),
                _ => return ULS_BADOBJECT,
            }
        };
        if attr != 0 {
            if (size as usize) < UCONV_ATTR_SIZE { return ULS_INVALID; }
            if self.guest_write_bytes(attr, &attribute_bytes(cp, &attrs)).is_none() { return ULS_INVALID; }
        }
        let (lead, trail) = byte_tables(cp);
        for (ptr, table) in [(first, &lead[..]), (other, &trail[..])] {
            if ptr != 0 && self.guest_write_bytes(ptr, table).is_none() { return ULS_INVALID; }
        }
        // No user-defined character ranges: all 32 empty.
        if udcrange != 0 && self.guest_write_bytes(udcrange, &[0u8; UDCRANGE_BYTES]).is_none() { return ULS_INVALID; }
        ULS_SUCCESS
    }

    pub fn uni_set_uconv_object(&self, uobj: u32, attr: u32) -> u32 {
        if self.shared.uconv_mgr.lock_or_recover().lookup(uobj).is_none() { return ULS_BADOBJECT; }
        let Some(bytes) = self.guest_read_bytes(attr, UCONV_ATTR_SIZE) else { return ULS_INVALID };
        let Some(attrs) = parse_attribute(&bytes) else { return ULS_BADATTR };
        self.shared.uconv_mgr.lock_or_recover().set_attrs(uobj, attrs);
        ULS_SUCCESS
    }

    /// The bytes of the NUL-terminated string at `p` (without its NUL), at
    /// most `MAX_UNI_STRING` of them.
    fn uconv_nul_string(&self, p: u32) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for i in 0..MAX_UNI_STRING {
            match self.guest_read::<u8>(p.checked_add(i)?)? { 0 => return Some(out), b => out.push(b) }
        }
        None
    }

    /// The UniChars of the NUL-terminated UCS-2 string at `p` (without its NUL).
    fn uconv_nul_ucs2(&self, p: u32) -> Option<Vec<u16>> {
        let mut out = Vec::new();
        for i in 0..MAX_UNI_STRING {
            match self.guest_read::<u16>(p.checked_add(2 * i)?)? { 0 => return Some(out), u => out.push(u) }
        }
        None
    }

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

// ── uconv_attribute_t ────────────────────────────────────────────────────────

/// The longest string the string calls read, in units.
const MAX_UNI_STRING: u32 = 64 * 1024;
/// `uconv_attribute_t` version 0 (<uconv.h>): version, the four lengths,
/// esid, options, state, endian (source, target), displaymask, converttype,
/// subchar_len, subuni_len, subchar[16], subuni[8].
pub const UCONV_ATTR_SIZE: usize = 60;
const MAX_SUBCHAR: usize = 16;
const MAX_SUBUNI: usize = 8;
/// `udcrange_t udcrange[32]`, two USHORTs each.
const UDCRANGE_BYTES: usize = 32 * 4;
/// The `conv_endian_t` values (`ENDIAN_SYSTEM`, `ENDIAN_BIG`, `ENDIAN_LITTLE`).
const ENDIANS: [u16; 3] = [0x0000, 0xFEFF, 0xFFFE];
/// The `options` values (`UCONV_OPTION_SUBSTITUTE_*`, 0 for none).
const MAX_OPTION: u8 = 3;
/// Encoding scheme ids (`enum uconv_esid`).
const ESID_SBCS_PC: u16 = 0x3100;
const ESID_MBCS_PC: u16 = 0x3300;
const ESID_UTF_8: u16 = 0x7807;

/// The DBCS lead-byte ranges of `cp` (empty for an SBCS codepage).
fn dbcs_leads(cp: u32) -> &'static [(u8, u8)] {
    if cp == CP_UTF8 { &[] } else { super::locale::dbcs_lead_ranges(cp) }
}

/// `UniQueryUconvObject`'s `first` and `other` tables: each byte's
/// character length as a first byte, and whether it can follow one.
pub fn byte_tables(cp: u32) -> ([u8; 256], [u8; 256]) {
    let mut first = [1u8; 256];
    let mut other = [0u8; 256];
    if cp == CP_UTF8 {
        for b in 0..256usize {
            first[b] = match b { 0x00..=0x7F => 1, 0xC2..=0xDF => 2, 0xE0..=0xEF => 3, 0xF0..=0xF4 => 4, _ => 0 };
            other[b] = u8::from((0x80..=0xBF).contains(&b));
        }
    } else {
        for &(lo, hi) in dbcs_leads(cp) {
            for b in lo..=hi { first[b as usize] = 2; }
        }
        if !dbcs_leads(cp).is_empty() {
            other[0x40..=0xFE].fill(1);
        }
    }
    (first, other)
}

/// A `uconv_attribute_t` for an object of codepage `cp` with `a`.
pub fn attribute_bytes(cp: u32, a: &UconvAttrs) -> [u8; UCONV_ATTR_SIZE] {
    let dbcs = !dbcs_leads(cp).is_empty();
    let (mb_max, esid) = if cp == CP_UTF8 { (3, ESID_UTF_8) } else if dbcs { (2, ESID_MBCS_PC) } else { (1, ESID_SBCS_PC) };
    let mut b = [0u8; UCONV_ATTR_SIZE];
    b[4] = 1; // mb_min_len
    b[5] = mb_max;
    b[6] = 1; // usc_min_len
    b[7] = 1; // usc_max_len
    b[8..10].copy_from_slice(&esid.to_le_bytes());
    b[10] = a.options;
    b[11] = a.state;
    b[12..14].copy_from_slice(&a.endian.0.to_le_bytes());
    b[14..16].copy_from_slice(&a.endian.1.to_le_bytes());
    b[16..20].copy_from_slice(&a.displaymask.to_le_bytes());
    b[20..24].copy_from_slice(&a.converttype.to_le_bytes());
    b[24..26].copy_from_slice(&(a.subchar.len() as u16).to_le_bytes());
    b[26..28].copy_from_slice(&(a.subuni.len() as u16).to_le_bytes());
    b[28..28 + a.subchar.len()].copy_from_slice(&a.subchar);
    for (i, u) in a.subuni.iter().enumerate() { b[44 + 2 * i..46 + 2 * i].copy_from_slice(&u.to_le_bytes()); }
    b
}

/// The settable attributes of a `uconv_attribute_t`, or `None` when they
/// are refused (`ULS_BADATTR`): a version other than 0, an unknown option
/// or endian, substitution strings longer than their arrays.
pub fn parse_attribute(b: &[u8]) -> Option<UconvAttrs> {
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if b.len() < UCONV_ATTR_SIZE || u32_at(0) != 0 { return None; }
    let (subchar_len, subuni_len) = (u16_at(24) as usize, u16_at(26) as usize);
    let endian = (u16_at(12), u16_at(14));
    if b[10] > MAX_OPTION || subchar_len > MAX_SUBCHAR || subuni_len > MAX_SUBUNI
        || !ENDIANS.contains(&endian.0) || !ENDIANS.contains(&endian.1) {
        return None;
    }
    Some(UconvAttrs {
        options: b[10],
        state: b[11],
        endian,
        displaymask: u32_at(16),
        converttype: u32_at(20),
        subchar: b[28..28 + subchar_len].to_vec(),
        subuni: (0..subuni_len).map(|i| u16_at(44 + 2 * i)).collect(),
    })
}

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

        assert_eq!(loader.uni_create_uconv_object(name_buf, h_buf), ULS_INVALID);
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

    fn object(loader: &Loader, name: &str) -> u32 {
        let name_buf = alloc(loader, 64);
        write_ucs2(loader, name_buf, name);
        let h_buf = alloc(loader, 4);
        assert_eq!(loader.uni_create_uconv_object(name_buf, h_buf), ULS_SUCCESS);
        loader.guest_read::<u32>(h_buf).unwrap()
    }

    /// The return codes are <ulserrno.h>'s.
    #[test]
    fn uls_codes_are_the_toolkits() {
        assert_eq!((ULS_INVALID, ULS_BADOBJECT, ULS_BUFFERFULL, ULS_BADATTR), (0x2040E, 0x2040F, 0x20412, 0x20415));
    }

    #[test]
    fn strings_convert_with_their_nul_and_stop_at_a_short_buffer() {
        let loader = Loader::new_mock();
        let h = object(&loader, "IBM-850");
        let src = alloc(&loader, 16);
        loader.guest_write_bytes(src, b"A\x82z\0").unwrap(); // 0x82 is é in CP850
        let ucs = alloc(&loader, 32);
        assert_eq!(loader.uni_str_to_ucs(h, ucs, src, 8), ULS_SUCCESS);
        let units: Vec<u16> = (0..4).map(|i| loader.guest_read::<u16>(ucs + 2 * i).unwrap()).collect();
        assert_eq!(units, [0x41, 0xE9, 0x7A, 0]);
        assert_eq!(loader.uni_str_to_ucs(h, ucs, src, 3), ULS_BUFFERFULL, "no room for the NUL");
        let out = alloc(&loader, 16);
        assert_eq!(loader.uni_str_from_ucs(h, out, ucs, 16), ULS_SUCCESS);
        assert_eq!(loader.guest_read_bytes(out, 4).unwrap(), b"A\x82z\0");
        assert_eq!(loader.uni_str_from_ucs(h, out, ucs, 2), ULS_BUFFERFULL);
        assert_eq!(loader.uni_str_from_ucs(0x1234, out, ucs, 16), ULS_BADOBJECT);
    }

    #[test]
    fn attributes_are_queried_and_set() {
        let loader = Loader::new_mock();
        let h = object(&loader, "IBM-932");
        let attr = alloc(&loader, UCONV_ATTR_SIZE as u32);
        let first = alloc(&loader, 256);
        assert_eq!(loader.uni_query_uconv_object(h, attr, UCONV_ATTR_SIZE as u32, first, 0, 0), ULS_SUCCESS);
        let b = loader.guest_read_bytes(attr, UCONV_ATTR_SIZE).unwrap();
        assert_eq!((b[4], b[5]), (1, 2), "mixed SBCS / DBCS");
        assert_eq!(loader.guest_read::<u8>(first + 0x81), Some(2), "a Shift-JIS lead byte");
        assert_eq!(loader.guest_read::<u8>(first + 0x41), Some(1));
        assert_eq!(loader.uni_query_uconv_object(h, attr, 8, 0, 0, 0), ULS_INVALID, "too small for version 0");
        // Set a substitution character and big-endian output, then read them back.
        let mut set = b.clone();
        set[14..16].copy_from_slice(&0xFEFFu16.to_le_bytes());
        set[24..26].copy_from_slice(&1u16.to_le_bytes());
        set[28] = b'#';
        loader.guest_write_bytes(attr, &set).unwrap();
        assert_eq!(loader.uni_set_uconv_object(h, attr), ULS_SUCCESS);
        assert_eq!(loader.uni_query_uconv_object(h, attr, UCONV_ATTR_SIZE as u32, 0, 0, 0), ULS_SUCCESS);
        let back = loader.guest_read_bytes(attr, UCONV_ATTR_SIZE).unwrap();
        assert_eq!((u16::from_le_bytes([back[14], back[15]]), back[28]), (0xFEFF, b'#'));
        set[12..14].copy_from_slice(&0x1234u16.to_le_bytes());
        loader.guest_write_bytes(attr, &set).unwrap();
        assert_eq!(loader.uni_set_uconv_object(h, attr), ULS_BADATTR, "an unknown endian");
        assert_eq!(byte_tables(CP_UTF8).0[0xE3], 3);
        assert_eq!(byte_tables(850).1[0x80], 0, "no trail bytes in an SBCS codepage");
    }
}
