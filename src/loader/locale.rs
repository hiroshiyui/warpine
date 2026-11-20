// SPDX-License-Identifier: GPL-3.0-or-later
//
// Host locale detection for OS/2 NLS (National Language Support).
// Reads the host's locale settings and maps them to OS/2 COUNTRYINFO values.

use log::debug;

/// Environment variable that overrides the OS/2 country code.
pub const COUNTRY_VAR: &str = "WARPINE_COUNTRY";
/// Environment variable that overrides the starting process codepage.
pub const CODEPAGE_VAR: &str = "WARPINE_CODEPAGE";
/// The country and codepage of the C / POSIX locale (and of no locale).
pub const DEFAULT_COUNTRY_CP: (u32, u32) = (1, 437);

/// The OS/2 country code and starting process codepage for a host locale
/// (`lang`, `territory` from e.g. `ja_JP.UTF-8`), like an OS/2 installation
/// in that language: Japanese → 81 / CP932, German → 49 / CP850.
///
/// - The country code is OS/2's, mostly the international dialling code
///   of the territory (or of the language's main country without one).
/// - The codepage is the language's usual OS/2 codepage among those
///   Warpine supports: 932 / 936 / 949 / 950 for CJK, 866 for Cyrillic,
///   852 for Central European languages, 437 for US English and 850 for
///   the rest (languages whose OS/2 codepage Warpine lacks, e.g. Greek
///   869, also get 850).
/// - `country_var` / `codepage_var` (`WARPINE_COUNTRY` /
///   `WARPINE_CODEPAGE`) override each; a codepage Warpine does not
///   support, or a value that is not a number, is ignored with a warning.
///
/// The country codes are from common knowledge of OS/2's COUNTRY.SYS
/// numbering (dialling codes, 88 for Taiwan, 42 for the Czech Republic and
/// Slovakia) and are not checked against an IBM source.
pub fn resolve_country(lang: &str, territory: &str,
                       country_var: Option<&str>, codepage_var: Option<&str>) -> (u32, u32) {
    let (mut country, mut cp) = locale_country(lang, territory);
    if let Some(v) = country_var {
        match v.trim().parse::<u32>() {
            Ok(c) if c > 0 => country = c,
            _ => log::warn!("{COUNTRY_VAR}={v:?} is not a country code; using {country}"),
        }
    }
    if let Some(v) = codepage_var {
        match v.trim().parse::<u32>() {
            Ok(c) if super::stubs::is_supported_codepage(c) => cp = c,
            _ => log::warn!("{CODEPAGE_VAR}={v:?} is not a supported codepage; using {cp}"),
        }
    }
    (country, cp)
}

/// [`resolve_country`] without the overrides.
fn locale_country(lang: &str, territory: &str) -> (u32, u32) {
    let lang = lang.to_ascii_lowercase();
    let territory = territory.to_ascii_uppercase();
    if lang.is_empty() || lang == "c" || lang == "posix" {
        return DEFAULT_COUNTRY_CP;
    }
    let cp = match lang.as_str() {
        "ja" => 932,
        "ko" => 949,
        "zh" if matches!(territory.as_str(), "TW" | "HK" | "MO") => 950,
        "zh" => 936,
        "ru" | "uk" | "be" | "bg" | "mk" => 866,
        "pl" | "cs" | "sk" | "hu" | "hr" | "sl" | "ro" | "bs" | "sq" => 852,
        "en" if matches!(territory.as_str(), "US" | "") => 437,
        _ => 850,
    };
    let by_territory = match territory.as_str() {
        "US" => Some(1), "CA" => Some(2), "MX" | "AR" | "CL" | "CO" | "PE" | "VE" => Some(3),
        "RU" | "KZ" => Some(7), "GR" => Some(30), "NL" => Some(31), "BE" => Some(32),
        "FR" => Some(33), "ES" => Some(34), "HU" => Some(36), "HR" | "SI" | "RS" | "BA" | "MK" => Some(38),
        "IT" => Some(39), "RO" => Some(40), "CH" => Some(41), "CZ" | "SK" => Some(42),
        "AT" => Some(43), "GB" | "IE" => Some(44), "DK" => Some(45), "SE" => Some(46),
        "NO" => Some(47), "PL" => Some(48), "DE" => Some(49), "BR" => Some(55),
        "AU" => Some(61), "NZ" => Some(64), "JP" => Some(81), "KR" => Some(82),
        "CN" | "SG" => Some(86), "TW" => Some(88), "TR" => Some(90), "IN" => Some(91),
        "PT" => Some(351), "IS" => Some(354), "FI" => Some(358), "BG" => Some(359),
        "UA" => Some(380), "BY" => Some(375),
        "HK" => Some(852), "MO" => Some(853),
        _ => None,
    };
    let country = by_territory.unwrap_or(match lang.as_str() {
        "ja" => 81, "ko" => 82, "zh" => 86, "ru" => 7, "de" => 49, "fr" => 33,
        "es" => 34, "it" => 39, "nl" => 31, "pl" => 48, "cs" => 42, "hu" => 36,
        "pt" => 351, "fi" => 358, "sv" => 46, "da" => 45, "nb" | "nn" | "no" => 47,
        _ => 1,
    });
    (country, cp)
}

/// OS/2 COUNTRYINFO fields derived from the host locale.
#[derive(Debug, Clone)]
pub struct Os2Locale {
    pub country: u32,
    pub codepage: u32,
    pub date_fmt: u32,          // 0=MDY, 1=DMY, 2=YMD
    pub currency: [u8; 5],      // null-terminated
    pub thousands_sep: u8,
    pub decimal_sep: u8,
    pub date_sep: u8,
    pub time_sep: u8,
    pub currency_fmt: u8,       // 0=prefix, 1=suffix, 2=prefix+space, 3=suffix+space
    pub decimal_places: u8,
    pub time_fmt: u8,           // 0=12-hour, 1=24-hour
    pub data_sep: u8,           // list separator
}

impl Os2Locale {
    /// Detect locale from the host environment.
    pub fn from_host() -> Self {
        let lang = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LC_CTYPE"))
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        debug!("Host locale: {:?}", lang);

        // Extract language and country from locale string (e.g., "ja_JP.UTF-8" → "ja", "JP")
        let locale_part = lang.split('.').next().unwrap_or("");
        let (language, territory) = if let Some(idx) = locale_part.find('_') {
            (&locale_part[..idx], &locale_part[idx + 1..])
        } else {
            (locale_part, "")
        };

        // Also read libc locale for numeric/monetary formatting
        let lconv = Self::read_lconv();

        let (country, codepage) = Self::map_country(language, territory);
        let date_fmt = Self::map_date_format(language, territory);
        let currency = Self::map_currency(language, territory, &lconv);
        let time_fmt = Self::map_time_format(language, territory);

        let locale = Os2Locale {
            country,
            codepage,
            date_fmt,
            currency,
            thousands_sep: lconv.thousands_sep,
            decimal_sep: lconv.decimal_sep,
            date_sep: Self::map_date_separator(language, territory),
            time_sep: Self::map_time_separator(language, territory),
            currency_fmt: lconv.currency_fmt,
            decimal_places: lconv.decimal_places,
            time_fmt,
            data_sep: if language == "de" || language == "nl" { b';' } else { b',' },
        };
        debug!("OS/2 locale: country={} cp={} datefmt={} timefmt={} datesep='{}' timesep='{}'",
            locale.country, locale.codepage, locale.date_fmt, locale.time_fmt,
            locale.date_sep as char, locale.time_sep as char);
        locale
    }

    /// The OS/2 country code and process codepage for a host locale, unless
    /// `WARPINE_COUNTRY` / `WARPINE_CODEPAGE` override them (see
    /// [`resolve_country`]).
    fn map_country(lang: &str, territory: &str) -> (u32, u32) {
        resolve_country(
            lang,
            territory,
            std::env::var(COUNTRY_VAR).ok().as_deref(),
            std::env::var(CODEPAGE_VAR).ok().as_deref(),
        )
    }

    /// Map to OS/2 date format: 0=MDY, 1=DMY, 2=YMD.
    fn map_date_format(lang: &str, territory: &str) -> u32 {
        match (lang, territory) {
            ("en", "US") | ("en", "") => 0, // MDY
            ("ja", _) | ("ko", _) | ("zh", _) | ("hu", _) | ("sv", _) | ("fi", _) => 2, // YMD
            _ => 1, // DMY (most of the world)
        }
    }

    /// Map to 12-hour (0) or 24-hour (1) time format.
    fn map_time_format(lang: &str, territory: &str) -> u8 {
        match (lang, territory) {
            ("en", "US") | ("en", "AU") | ("en", "") => 0, // 12-hour
            _ => 1, // 24-hour (most of the world)
        }
    }

    /// Map to date separator character.
    fn map_date_separator(lang: &str, territory: &str) -> u8 {
        match (lang, territory) {
            ("de", _) | ("nl", _) | ("fr", _) | ("it", _) | ("es", _)
            | ("pt", _) | ("ru", _) | ("uk", _) | ("pl", _) | ("cs", _)
            | ("sk", _) | ("hu", _) | ("tr", _) | ("el", _) | ("da", _)
            | ("no", _) | ("nb", _) | ("nn", _) | ("sv", _) | ("fi", _)
            | ("en", "GB") | ("en", "AU") => b'.',
            ("ja", _) | ("ko", _) | ("zh", _) => b'/',
            _ => b'-', // US and others
        }
    }

    /// Map to time separator character.
    fn map_time_separator(lang: &str, _territory: &str) -> u8 {
        match lang {
            "fi" => b'.',
            _ => b':', // Almost universal
        }
    }

    /// Map to currency symbol (up to 4 chars + null).
    fn map_currency(lang: &str, territory: &str, lconv: &HostLconv) -> [u8; 5] {
        // Prefer libc lconv if available and ASCII-representable
        if !lconv.currency_symbol.is_empty() && lconv.currency_symbol.is_ascii() && lconv.currency_symbol.len() <= 4 {
            let mut buf = [0u8; 5];
            let bytes = lconv.currency_symbol.as_bytes();
            buf[..bytes.len()].copy_from_slice(bytes);
            return buf;
        }
        // Fallback mapping (OS/2 CP437-compatible symbols)
        let sym: &[u8] = match (lang, territory) {
            ("en", "US") | ("en", "AU") | ("en", "CA") | ("en", "") => b"$",
            ("en", "GB") | ("en", "UK") => &[0x9C], // £ in CP437/850
            ("ja", _) => &[0x9D], // ¥ in CP437
            ("de", _) | ("fr", _) | ("it", _) | ("es", _) | ("pt", _)
            | ("nl", _) | ("fi", _) | ("el", _) => b"EUR",
            ("da", _) | ("no", _) | ("nb", _) | ("nn", _) | ("sv", _) => b"kr",
            ("pl", _) => b"zl",
            ("cs", _) | ("sk", _) => b"Kc",
            ("hu", _) => b"Ft",
            ("ru", _) | ("uk", _) => b"RUB",
            ("ko", _) => b"W",
            ("zh", "TW") => b"NT$",
            ("zh", _) => b"Y",
            ("tr", _) => b"TL",
            ("th", _) => b"B",
            _ => b"$",
        };
        let mut buf = [0u8; 5];
        let len = sym.len().min(4);
        buf[..len].copy_from_slice(&sym[..len]);
        buf
    }

    /// Read libc locale conventions.
    fn read_lconv() -> HostLconv {
        // Read locale conventions without changing global locale state.
        // setlocale(LC_ALL, "") would affect the host process's libc behavior.

        let lc = unsafe { libc::localeconv() };
        if lc.is_null() {
            return HostLconv::default();
        }
        // SAFETY: localeconv() returns a valid pointer to a static struct
        let lc = unsafe { &*lc };

        let read_char = |p: *const libc::c_char| -> u8 {
            if p.is_null() { return 0; }
            // SAFETY: checked for null above
            let b = unsafe { *p } as u8;
            if b == 0 { 0 } else { b }
        };
        let read_str = |p: *const libc::c_char| -> String {
            if p.is_null() { return String::new(); }
            // SAFETY: checked for null above, localeconv strings are null-terminated
            unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
        };

        HostLconv {
            decimal_sep: {
                let d = read_char(lc.decimal_point);
                if d == 0 { b'.' } else { d }
            },
            thousands_sep: {
                let t = read_char(lc.thousands_sep);
                if t == 0 { b',' } else { t }
            },
            currency_symbol: read_str(lc.currency_symbol),
            currency_fmt: {
                // p_cs_precedes: 1 = symbol before value, 0 = after
                // p_sep_by_space: 1 = space between symbol and value
                match (lc.p_cs_precedes as u8, lc.p_sep_by_space as u8) {
                    (1, 0) => 0, // prefix, no space
                    (0, 0) => 1, // suffix, no space
                    (1, _) => 2, // prefix with space
                    (0, _) => 3, // suffix with space
                    _ => 0,
                }
            },
            decimal_places: {
                let d = lc.frac_digits as u8;
                if d == u8::MAX { 2 } else { d } // CHAR_MAX means unset
            },
        }
    }
}

/// Return the DBCS lead-byte ranges for a given OS/2 codepage.
///
/// A DBCS lead byte introduces a two-byte character sequence.  Any byte whose
/// value falls within one of the returned `(first, last)` inclusive ranges is
/// a lead byte for that codepage; all other bytes are single-byte characters.
///
/// Returns an empty slice for SBCS codepages (e.g. CP437, CP850, CP1252).
///
/// Source: IBM National Language Support Reference Manual, Vol. 2, lead-byte
/// tables for each supported DBCS codepage.
///
/// ```
/// # use warpine::loader::locale::dbcs_lead_ranges;
/// assert_eq!(dbcs_lead_ranges(437), &[]);                        // SBCS
/// assert_eq!(dbcs_lead_ranges(932)[0], (0x81, 0x9F));            // Shift-JIS low
/// assert_eq!(dbcs_lead_ranges(936), &[(0x81_u8, 0xFE_u8)]);      // GBK
/// ```
pub fn dbcs_lead_ranges(cp: u32) -> &'static [(u8, u8)] {
    match cp {
        // CP932 — Shift-JIS (Japanese): two disjoint lead-byte ranges.
        932 => &[(0x81, 0x9F), (0xE0, 0xFC)],
        // CP936 — GBK / GB2312 (Simplified Chinese)
        936 => &[(0x81, 0xFE)],
        // CP949 — EUC-KR / UHC (Korean)
        949 => &[(0x81, 0xFE)],
        // CP950 — Big5 (Traditional Chinese)
        950 => &[(0x81, 0xFE)],
        // All SBCS codepages (CP437, CP850, CP852, CP1250–CP1258, etc.)
        _ => &[],
    }
}

/// Returns `true` if `byte` is a DBCS lead byte for the given codepage.
///
/// This is a convenience wrapper around [`dbcs_lead_ranges`] used by VIO
/// routines that need to classify individual bytes.
#[inline]
pub fn is_dbcs_lead_byte(byte: u8, cp: u32) -> bool {
    dbcs_lead_ranges(cp).iter().any(|&(lo, hi)| byte >= lo && byte <= hi)
}

#[derive(Debug, Default)]
struct HostLconv {
    decimal_sep: u8,
    thousands_sep: u8,
    currency_symbol: String,
    currency_fmt: u8,
    decimal_places: u8,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{resolve_country, DEFAULT_COUNTRY_CP};

    #[test]
    fn country_and_codepage_follow_the_host_locale() {
        let r = |l, t| resolve_country(l, t, None, None);
        assert_eq!(r("ja", "JP"), (81, 932));
        assert_eq!(r("ko", "KR"), (82, 949));
        assert_eq!(r("zh", "CN"), (86, 936));
        assert_eq!(r("zh", "TW"), (88, 950));
        assert_eq!(r("zh", "HK"), (852, 950));
        assert_eq!(r("ru", "RU"), (7, 866));
        assert_eq!(r("de", "DE"), (49, 850));
        assert_eq!(r("de", "AT"), (43, 850));
        assert_eq!(r("pl", "PL"), (48, 852));
        assert_eq!(r("en", "US"), (1, 437));
        assert_eq!(r("en", "GB"), (44, 850));
        assert_eq!(r("en", ""), (1, 437));
        assert_eq!(r("fr", ""), (33, 850), "no territory: the language's country");
        assert_eq!(r("eo", ""), (1, 850), "unknown language: country 1");
        assert_eq!(r("", ""), DEFAULT_COUNTRY_CP);
        assert_eq!(r("C", ""), DEFAULT_COUNTRY_CP);
        assert_eq!(r("POSIX", ""), DEFAULT_COUNTRY_CP);
    }

    #[test]
    fn overrides_win_and_bad_values_are_ignored() {
        assert_eq!(resolve_country("ja", "JP", Some("1"), Some("437")), (1, 437));
        assert_eq!(resolve_country("en", "US", None, Some(" 932 ")), (1, 932));
        assert_eq!(resolve_country("en", "US", Some("49"), None), (49, 437));
        // 1004 is a PM queue codepage only; 1200 is not supported at all.
        assert_eq!(resolve_country("ja", "JP", None, Some("1004")), (81, 932));
        assert_eq!(resolve_country("ja", "JP", Some("x"), Some("1200")), (81, 932));
        assert_eq!(resolve_country("ja", "JP", Some("0"), None), (81, 932));
    }

    use super::*;

    // ── dbcs_lead_ranges ─────────────────────────────────────────────────────

    #[test]
    fn test_dbcs_lead_ranges_sbcs_empty() {
        // All SBCS codepages must return an empty slice.
        for cp in [437, 850, 852, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258, 0] {
            assert_eq!(dbcs_lead_ranges(cp), &[],
                "cp {} should have no lead-byte ranges", cp);
        }
    }

    #[test]
    fn test_dbcs_lead_ranges_cp932_shift_jis() {
        let ranges = dbcs_lead_ranges(932);
        assert_eq!(ranges.len(), 2, "CP932 must have exactly two ranges");
        assert_eq!(ranges[0], (0x81, 0x9F));
        assert_eq!(ranges[1], (0xE0, 0xFC));
    }

    #[test]
    fn test_dbcs_lead_ranges_cp936_gbk() {
        assert_eq!(dbcs_lead_ranges(936), &[(0x81, 0xFE)]);
    }

    #[test]
    fn test_dbcs_lead_ranges_cp949_euc_kr() {
        assert_eq!(dbcs_lead_ranges(949), &[(0x81, 0xFE)]);
    }

    #[test]
    fn test_dbcs_lead_ranges_cp950_big5() {
        assert_eq!(dbcs_lead_ranges(950), &[(0x81, 0xFE)]);
    }

    // ── is_dbcs_lead_byte ────────────────────────────────────────────────────

    #[test]
    fn test_is_dbcs_lead_byte_cp932_in_range() {
        // Low range: 0x81–0x9F
        assert!(is_dbcs_lead_byte(0x81, 932));
        assert!(is_dbcs_lead_byte(0x9F, 932));
        assert!(is_dbcs_lead_byte(0x90, 932)); // mid
        // High range: 0xE0–0xFC
        assert!(is_dbcs_lead_byte(0xE0, 932));
        assert!(is_dbcs_lead_byte(0xFC, 932));
        assert!(is_dbcs_lead_byte(0xF0, 932)); // mid
    }

    #[test]
    fn test_is_dbcs_lead_byte_cp932_outside_range() {
        assert!(!is_dbcs_lead_byte(0x7F, 932)); // below first range
        assert!(!is_dbcs_lead_byte(0x80, 932)); // just below 0x81
        assert!(!is_dbcs_lead_byte(0xA0, 932)); // gap between ranges (0xA0–0xDF)
        assert!(!is_dbcs_lead_byte(0xDF, 932)); // just below 0xE0
        assert!(!is_dbcs_lead_byte(0xFD, 932)); // above 0xFC
        assert!(!is_dbcs_lead_byte(0xFF, 932));
        assert!(!is_dbcs_lead_byte(b'A', 932)); // ASCII
    }

    #[test]
    fn test_is_dbcs_lead_byte_cp936_boundaries() {
        assert!(!is_dbcs_lead_byte(0x80, 936)); // just below 0x81
        assert!(is_dbcs_lead_byte(0x81, 936));
        assert!(is_dbcs_lead_byte(0xFE, 936));
        assert!(!is_dbcs_lead_byte(0xFF, 936)); // just above 0xFE
    }

    #[test]
    fn test_is_dbcs_lead_byte_sbcs_always_false() {
        for cp in [437, 850, 1252] {
            for byte in [0x00u8, 0x41, 0x81, 0xA0, 0xFE, 0xFF] {
                assert!(!is_dbcs_lead_byte(byte, cp),
                    "cp {} byte 0x{:02X} must not be a DBCS lead", cp, byte);
            }
        }
    }
}
