// SPDX-License-Identifier: GPL-3.0-only
//
// Codepage conversion helpers for OS/2 string boundaries.
//
// Decodes guest byte strings (single-byte OS/2 codepages) to Rust String (UTF-8)
// and encodes Rust strings back to codepage byte vectors for writing to guest memory.
//
// DOS codepages (437, 850, 852) use embedded upper-half lookup tables: they are
// not part of the WHATWG Encoding Standard and therefore not in encoding_rs.
// Windows/DBCS codepages (1250–1258, 932, 949, 950) delegate to encoding_rs.
//
// All tables cover only bytes 0x80–0xFF; bytes 0x00–0x7F are pure ASCII and
// identical across all supported codepages.

// ── CP437 (PC US / OEM) upper-half table ─────────────────────────────────────
// Index 0 = byte 0x80, index 127 = byte 0xFF.
// Source: Unicode.org CP437.TXT mapping.
#[rustfmt::skip]
const CP437_UPPER: [char; 128] = [
    // 0x80-0x8F
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}',
    '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}',
    '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    // 0x90-0x9F
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}',
    '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}',
    '\u{00A3}', '\u{00A5}', '\u{20A7}', '\u{0192}',
    // 0xA0-0xAF
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}',
    '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{2310}', '\u{00AC}', '\u{00BD}',
    '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    // 0xB0-0xBF
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}',
    '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}',
    '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}',
    // 0xC0-0xCF
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}',
    '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}',
    '\u{2560}', '\u{2550}', '\u{256C}', '\u{2567}',
    // 0xD0-0xDF
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}',
    '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}',
    '\u{256A}', '\u{2518}', '\u{250C}', '\u{2588}',
    '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
    // 0xE0-0xEF
    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}',
    '\u{03A3}', '\u{03C3}', '\u{00B5}', '\u{03C4}',
    '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}',
    '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}',
    // 0xF0-0xFF
    '\u{2261}', '\u{00B1}', '\u{2265}', '\u{2264}',
    '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}',
    '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}',
    '\u{207F}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

// ── CP850 (Multilingual Latin-1) upper-half table ────────────────────────────
// Source: Unicode.org CP850.TXT mapping.
#[rustfmt::skip]
const CP850_UPPER: [char; 128] = [
    // 0x80-0x8F
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}',
    '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}',
    '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    // 0x90-0x9F
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}',
    '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00F8}',
    '\u{00A3}', '\u{00D8}', '\u{00D7}', '\u{0192}',
    // 0xA0-0xAF
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}',
    '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{00AE}', '\u{00AC}', '\u{00BD}',
    '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    // 0xB0-0xBF
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}',
    '\u{2524}', '\u{00C1}', '\u{00C2}', '\u{00C0}',
    '\u{00A9}', '\u{2563}', '\u{2551}', '\u{2557}',
    '\u{255D}', '\u{00A2}', '\u{00A5}', '\u{2510}',
    // 0xC0-0xCF
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}',
    '\u{2500}', '\u{253C}', '\u{00E3}', '\u{00C3}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}',
    '\u{2560}', '\u{2550}', '\u{256C}', '\u{00A4}',
    // 0xD0-0xDF
    '\u{00F0}', '\u{00D0}', '\u{00CA}', '\u{00CB}',
    '\u{00C8}', '\u{0131}', '\u{00CD}', '\u{00CE}',
    '\u{00CF}', '\u{2518}', '\u{250C}', '\u{2588}',
    '\u{2584}', '\u{00A6}', '\u{00CC}', '\u{2580}',
    // 0xE0-0xEF
    '\u{00D3}', '\u{00DF}', '\u{00D4}', '\u{00D2}',
    '\u{00F5}', '\u{00D5}', '\u{00B5}', '\u{00FE}',
    '\u{00DE}', '\u{00DA}', '\u{00DB}', '\u{00D9}',
    '\u{00FD}', '\u{00DD}', '\u{00AF}', '\u{00B4}',
    // 0xF0-0xFF
    '\u{00AD}', '\u{00B1}', '\u{2017}', '\u{00BE}',
    '\u{00B6}', '\u{00A7}', '\u{00F7}', '\u{00B8}',
    '\u{00B0}', '\u{00A8}', '\u{00B7}', '\u{00B9}',
    '\u{00B3}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

// ── CP852 (Central European / Latin-2) upper-half table ──────────────────────
// Source: Unicode.org CP852.TXT mapping.
#[rustfmt::skip]
const CP852_UPPER: [char; 128] = [
    // 0x80-0x8F
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}',
    '\u{00E4}', '\u{016F}', '\u{0107}', '\u{00E7}',
    '\u{0142}', '\u{00EB}', '\u{0150}', '\u{0151}',
    '\u{00EE}', '\u{0179}', '\u{00C4}', '\u{0106}',
    // 0x90-0x9F
    '\u{00C9}', '\u{0139}', '\u{013A}', '\u{00F4}',
    '\u{00F6}', '\u{013D}', '\u{013E}', '\u{015A}',
    '\u{015B}', '\u{00D6}', '\u{00DC}', '\u{0164}',
    '\u{0165}', '\u{0141}', '\u{00D7}', '\u{010D}',
    // 0xA0-0xAF
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}',
    '\u{0104}', '\u{0105}', '\u{017D}', '\u{017E}',
    '\u{0118}', '\u{0119}', '\u{00AC}', '\u{017A}',
    '\u{010C}', '\u{015F}', '\u{00AB}', '\u{00BB}',
    // 0xB0-0xBF
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}',
    '\u{2524}', '\u{00C1}', '\u{00C2}', '\u{011A}',
    '\u{0160}', '\u{2563}', '\u{2551}', '\u{2557}',
    '\u{255D}', '\u{017B}', '\u{017C}', '\u{2510}',
    // 0xC0-0xCF
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}',
    '\u{2500}', '\u{253C}', '\u{0102}', '\u{0103}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}',
    '\u{2560}', '\u{2550}', '\u{256C}', '\u{00A4}',
    // 0xD0-0xDF
    '\u{0111}', '\u{0110}', '\u{010E}', '\u{00CB}',
    '\u{010F}', '\u{0147}', '\u{00CD}', '\u{00CE}',
    '\u{011B}', '\u{2518}', '\u{250C}', '\u{2588}',
    '\u{2584}', '\u{0162}', '\u{016E}', '\u{2580}',
    // 0xE0-0xEF
    '\u{00D3}', '\u{00DF}', '\u{00D4}', '\u{0143}',
    '\u{0144}', '\u{0148}', '\u{0160}', '\u{016F}',  // 0xE6=Š 0xE7=ů (revisited)
    '\u{00DE}', '\u{00DA}', '\u{00DB}', '\u{00D9}',
    '\u{00FD}', '\u{00DD}', '\u{0163}', '\u{00B4}',
    // 0xF0-0xFF
    '\u{00AD}', '\u{02DD}', '\u{02DB}', '\u{02C7}',
    '\u{02D8}', '\u{00A7}', '\u{00F7}', '\u{00B8}',
    '\u{00B0}', '\u{00A8}', '\u{02D9}', '\u{0171}',
    '\u{0158}', '\u{0159}', '\u{25A0}', '\u{00A0}',
];

// ── Public API ────────────────────────────────────────────────────────────────

/// Map a single guest byte to its uppercase equivalent in the given codepage.
///
/// Only 1:1 Unicode case mappings are applied.  Multi-char results (e.g. ß→SS)
/// leave the byte unchanged — this matches OS/2's fixed-width NLS behaviour
/// where `DosMapCase`/`NlsMapCase` operate on a fixed-length byte buffer.
///
/// DBCS codepages (CP932/949/950) are not supported here: a single byte is not
/// a complete character so the byte is returned unchanged.  Unknown codepages
/// also return the byte unchanged.
pub fn cp_map_case_upper(byte: u8, cp: u32) -> u8 {
    // ASCII fast path — identical in all codepages.
    if byte < 0x80 {
        return byte.to_ascii_uppercase();
    }
    match cp {
        437 => map_case_sbcs(byte, &CP437_UPPER),
        850 => map_case_sbcs(byte, &CP850_UPPER),
        852 => map_case_sbcs(byte, &CP852_UPPER),
        // DBCS: a single byte may be a multi-byte lead byte — cannot case-map.
        932 | 949 | 950 => byte,
        _ => {
            // Windows SBCS (1250–1258): decode single byte → Unicode →
            // uppercase (1:1 only) → re-encode.
            if matches!(cp, 1250..=1258) && let Some(enc) = cp_to_encoding(cp) {
                let buf = [byte];
                let (decoded, _) = enc.decode_without_bom_handling(&buf);
                if let Some(ch) = decoded.chars().next() {
                    let mut it = ch.to_uppercase();
                    let upper_ch = it.next();
                    let is_single = it.next().is_none();
                    if let Some(up) = upper_ch && is_single && up != ch {
                        // Single-char uppercase — re-encode.
                        let s: String = std::iter::once(up).collect();
                        let (encoded, _, _) = enc.encode(&s);
                        if let Some(&b) = encoded.first() {
                            return b;
                        }
                    }
                }
            }
            byte
        }
    }
}

/// Uppercase a single byte using a DOS codepage upper-half table.
///
/// Looks up the Unicode character at `byte`, applies `.to_uppercase()`, and
/// reverses back to a byte.  Multi-char uppercase results (ß→SS) are skipped.
/// Uppercase codepoints that fall in ASCII are returned as their ASCII byte.
/// Codepoints with no representation in the table are returned unchanged.
fn map_case_sbcs(byte: u8, upper: &[char; 128]) -> u8 {
    let ch = upper[(byte - 0x80) as usize];
    let mut it = ch.to_uppercase();
    let Some(up) = it.next() else { return byte; };
    if it.next().is_some() { return byte; } // multi-char uppercase: leave unchanged
    if up == ch { return byte; }
    if up.is_ascii() { return up as u8; }
    // Search the upper-half table for the uppercased codepoint.
    upper.iter().position(|&t| t == up)
        .map_or(byte, |i| (i as u8) + 0x80)
}

/// Decode a byte slice from the given OS/2 codepage into a Rust `String`.
///
/// - Bytes 0x00–0x7F are treated as ASCII (identical in all supported codepages).
/// - The upper half (0x80–0xFF) is decoded via an embedded table for DOS codepages
///   (437, 850, 852) or via `encoding_rs` for Windows/DBCS codepages.
/// - If `cp` is unrecognised, falls back to Latin-1 promotion (`byte as char`).
pub fn cp_decode(bytes: &[u8], cp: u32) -> String {
    match cp {
        437 => decode_single_byte(bytes, &CP437_UPPER),
        850 => decode_single_byte(bytes, &CP850_UPPER),
        852 => decode_single_byte(bytes, &CP852_UPPER),
        _ => {
            if let Some(enc) = cp_to_encoding(cp) {
                let (decoded, _, _) = enc.decode(bytes);
                decoded.into_owned()
            } else {
                // Unknown codepage: fall back to Latin-1 (byte value = codepoint).
                bytes.iter().map(|&b| b as char).collect()
            }
        }
    }
}

/// Encode a Rust `&str` (UTF-8) to a byte vector in the given OS/2 codepage.
///
/// For DOS codepages a reverse lookup is built from the table on first call.
/// Codepoints that have no representation in the target codepage are replaced
/// with `b'?'`.
pub fn cp_encode(s: &str, cp: u32) -> Vec<u8> {
    match cp {
        437 => encode_single_byte(s, &CP437_UPPER),
        850 => encode_single_byte(s, &CP850_UPPER),
        852 => encode_single_byte(s, &CP852_UPPER),
        _ => {
            if let Some(enc) = cp_to_encoding(cp) {
                let (encoded, _, _) = enc.encode(s);
                encoded.into_owned()
            } else {
                // Unknown codepage: strip non-ASCII, replace with '?'
                s.chars()
                    .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
                    .collect()
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Map an OS/2 codepage number to an `encoding_rs` `Encoding`.
///
/// Only Windows / DBCS codepages supported by the WHATWG Encoding Standard are
/// returned; DOS codepages (437, 850, 852) are handled via embedded tables and
/// will return `None` here.
pub fn cp_to_encoding(cp: u32) -> Option<&'static encoding_rs::Encoding> {
    match cp {
        932  => Some(encoding_rs::SHIFT_JIS),
        949  => Some(encoding_rs::EUC_KR),
        950  => Some(encoding_rs::BIG5),
        1250 => Some(encoding_rs::WINDOWS_1250),
        1251 => Some(encoding_rs::WINDOWS_1251),
        1252 => Some(encoding_rs::WINDOWS_1252),
        1253 => Some(encoding_rs::WINDOWS_1253),
        1254 => Some(encoding_rs::WINDOWS_1254),
        1255 => Some(encoding_rs::WINDOWS_1255),
        1256 => Some(encoding_rs::WINDOWS_1256),
        1257 => Some(encoding_rs::WINDOWS_1257),
        1258 => Some(encoding_rs::WINDOWS_1258),
        _    => None,
    }
}

/// Decode using an upper-half table for single-byte (DOS) codepages.
fn decode_single_byte(bytes: &[u8], upper: &[char; 128]) -> String {
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x80 {
            s.push(b as char);
        } else {
            s.push(upper[(b - 0x80) as usize]);
        }
    }
    s
}

/// Encode using a reverse lookup built from an upper-half table.
fn encode_single_byte(s: &str, upper: &[char; 128]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii() {
            out.push(ch as u8);
        } else {
            // Linear search in the 128-entry table.  Not hot path; good enough.
            let found = upper.iter().position(|&t| t == ch);
            out.push(found.map_or(b'?', |i| (i as u8) + 0x80));
        }
    }
    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ASCII bytes are identical in all codepages.
    #[test]
    fn test_ascii_passthrough_cp437() {
        let bytes: Vec<u8> = (0u8..128u8).collect();
        let s = cp_decode(&bytes, 437);
        assert_eq!(s.len(), 128);
        for (i, ch) in s.chars().enumerate() {
            assert_eq!(ch as u32, i as u32, "byte {i:#04x} should be ASCII");
        }
    }

    #[test]
    fn test_ascii_passthrough_cp1252() {
        let bytes: Vec<u8> = (0u8..128u8).collect();
        let s = cp_decode(&bytes, 1252);
        for (i, ch) in s.chars().enumerate() {
            assert_eq!(ch as u32, i as u32, "byte {i:#04x} should be ASCII");
        }
    }

    // CP437-specific upper-half spot checks (Unicode.org CP437.TXT authoritative).
    #[test]
    fn test_cp437_upper_half() {
        assert_eq!(cp_decode(&[0xE1], 437), "\u{00DF}"); // ß  (German sharp-s)
        assert_eq!(cp_decode(&[0x80], 437), "\u{00C7}"); // Ç
        assert_eq!(cp_decode(&[0xDB], 437), "\u{2588}"); // █ (full block)
        assert_eq!(cp_decode(&[0xC4], 437), "\u{2500}"); // ─ (box-drawing)
        assert_eq!(cp_decode(&[0xFF], 437), "\u{00A0}"); // NBSP
    }

    // CP850 differs from CP437 in the upper half.
    #[test]
    fn test_cp850_differs_from_cp437() {
        // Byte 0x9B: CP437 → ¢ (U+00A2), CP850 → ø (U+00F8)
        assert_eq!(cp_decode(&[0x9B], 437), "\u{00A2}");
        assert_eq!(cp_decode(&[0x9B], 850), "\u{00F8}");
    }

    // CP852 Central-European spot checks.
    #[test]
    fn test_cp852_upper_half() {
        assert_eq!(cp_decode(&[0x85], 852), "\u{016F}"); // ů
        assert_eq!(cp_decode(&[0x86], 852), "\u{0107}"); // ć
        assert_eq!(cp_decode(&[0x88], 852), "\u{0142}"); // ł
    }

    // Windows-1252 via encoding_rs.
    #[test]
    fn test_cp1252_euro_sign() {
        // 0x80 in Windows-1252 is € (U+20AC), not a Latin-1 control character.
        assert_eq!(cp_decode(&[0x80], 1252), "\u{20AC}");
    }

    // encode → decode roundtrip for ASCII.
    #[test]
    fn test_encode_decode_ascii_roundtrip_cp437() {
        let original = "Hello, OS/2!";
        let encoded = cp_encode(original, 437);
        let decoded = cp_decode(&encoded, 437);
        assert_eq!(decoded, original);
    }

    // encode → decode roundtrip for upper-half chars.
    #[test]
    fn test_encode_decode_upper_roundtrip_cp437() {
        let original = "\u{00DF}\u{00C7}"; // ß Ç
        let encoded = cp_encode(original, 437);
        assert_eq!(encoded, &[0xE1, 0x80]);
        let decoded = cp_decode(&encoded, 437);
        assert_eq!(decoded, original);
    }

    // Unencodable characters become '?'.
    #[test]
    fn test_encode_unencodable_becomes_question_mark() {
        // 🦀 (U+1F980) has no CP437 representation.
        let encoded = cp_encode("A\u{1F980}B", 437);
        assert_eq!(encoded, b"A?B");
    }

    // cp_to_encoding returns None for DOS codepages (handled by tables).
    #[test]
    fn test_cp_to_encoding_dos_returns_none() {
        assert!(cp_to_encoding(437).is_none());
        assert!(cp_to_encoding(850).is_none());
        assert!(cp_to_encoding(852).is_none());
    }

    // cp_to_encoding returns Some for Windows codepages.
    #[test]
    fn test_cp_to_encoding_windows_returns_some() {
        for cp in [932u32, 949, 950, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258] {
            assert!(cp_to_encoding(cp).is_some(), "cp={cp} should be supported");
        }
    }

    // Unknown codepage falls back to Latin-1.
    #[test]
    fn test_unknown_cp_latin1_fallback() {
        let bytes = [0xE9u8]; // In Latin-1 this is é (U+00E9)
        let s = cp_decode(&bytes, 9999);
        assert_eq!(s, "\u{00E9}");
    }

    // ── cp_map_case_upper ─────────────────────────────────────────────────────

    #[test]
    fn test_map_case_upper_ascii() {
        assert_eq!(cp_map_case_upper(b'a', 437), b'A');
        assert_eq!(cp_map_case_upper(b'z', 850), b'Z');
        assert_eq!(cp_map_case_upper(b'A', 852), b'A'); // already uppercase
        assert_eq!(cp_map_case_upper(b'0', 437), b'0'); // digit: unchanged
    }

    // CP850: é (0x82=U+00E9) → É (0x90=U+00C9)
    #[test]
    fn test_map_case_upper_cp850_e_acute() {
        assert_eq!(cp_map_case_upper(0x82, 850), 0x90,
            "CP850 0x82 (é) should uppercase to 0x90 (É)");
    }

    // CP850: ü (0x81=U+00FC) → Ü (0x9A=U+00DC)
    #[test]
    fn test_map_case_upper_cp850_u_umlaut() {
        assert_eq!(cp_map_case_upper(0x81, 850), 0x9A,
            "CP850 0x81 (ü) should uppercase to 0x9A (Ü)");
    }

    // CP437: box-drawing char (0xC4=U+2500) has no case — must be unchanged.
    #[test]
    fn test_map_case_upper_cp437_box_drawing_unchanged() {
        assert_eq!(cp_map_case_upper(0xC4, 437), 0xC4,
            "CP437 box-drawing ─ (0xC4) has no case and must be unchanged");
    }

    // CP437: ß (0xE1=U+00DF) uppercases to SS in Unicode (multi-char) — must be unchanged.
    #[test]
    fn test_map_case_upper_cp437_sharp_s_unchanged() {
        assert_eq!(cp_map_case_upper(0xE1, 437), 0xE1,
            "CP437 ß (0xE1) multi-char uppercase must be left unchanged");
    }

    // CP852: ü (0x81=U+00FC) → Ü (0x9A=U+00DC) — same as CP850
    #[test]
    fn test_map_case_upper_cp852_u_umlaut() {
        assert_eq!(cp_map_case_upper(0x81, 852), 0x9A,
            "CP852 0x81 (ü) should uppercase to 0x9A (Ü)");
    }

    // Windows-1252: é (0xE9=U+00E9) → É (0xC9=U+00C9)
    #[test]
    fn test_map_case_upper_cp1252_e_acute() {
        assert_eq!(cp_map_case_upper(0xE9, 1252), 0xC9,
            "CP1252 0xE9 (é) should uppercase to 0xC9 (É)");
    }

    // Windows-1252: already-uppercase É (0xC9) must be unchanged.
    #[test]
    fn test_map_case_upper_cp1252_uppercase_unchanged() {
        assert_eq!(cp_map_case_upper(0xC9, 1252), 0xC9,
            "CP1252 0xC9 (É) is already uppercase — must be unchanged");
    }

    // DBCS codepage (CP932): single byte must be returned unchanged.
    #[test]
    fn test_map_case_upper_dbcs_unchanged() {
        assert_eq!(cp_map_case_upper(0x82, 932), 0x82,
            "CP932 (DBCS) single byte must be returned unchanged");
    }

    // Unknown codepage: byte returned unchanged.
    #[test]
    fn test_map_case_upper_unknown_cp_unchanged() {
        assert_eq!(cp_map_case_upper(0xE9, 9999), 0xE9);
    }
}
