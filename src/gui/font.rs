// SPDX-License-Identifier: GPL-3.0-only

//! Presentation Manager typeface: the IBM Plex families.
//!
//! All PM text (window chrome, menus, dialogs, built-in controls and text that
//! applications draw through GPI) is rasterised from the IBM Plex outline fonts
//! vendored under `vendor/ibm-plex` (SIL Open Font License 1.1) and embedded in
//! the binary:
//!
//! * IBM Plex Sans / Mono / Serif in Regular, Bold, Italic and Bold Italic;
//! * IBM Plex Sans JP / KR / TC / SC (Regular) for CJK text.
//!
//! A character is looked up in the requested Latin face first, then in the CJK
//! faces (ordered by the active DBCS codepage, because Han ideographs are
//! shared between them), and finally in the GNU Unifont bitmap tables that the
//! VIO text renderer uses, so every character Warpine could draw before is
//! still drawable.
//!
//! The VIO text-mode console does not go through this module: it needs a fixed
//! cell grid and keeps its 8×16 bitmap font.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};

use super::text_renderer::{get_glyph_dbcs, get_glyph_for_char};

/// Line height (ascent + descent) of the PM system font, in pixels.
///
/// Matches the 16-px text rows the PM layout code is built around (menu rows,
/// dialog units, control heights).
pub const SYSTEM_FONT_PX: u16 = 16;

/// Smallest and largest line height a guest may request.
const MIN_FONT_PX: u16 = 6;
const MAX_FONT_PX: u16 = 200;

/// Clamp a guest-requested line height to the supported range.
pub fn clamp_px(px: u32) -> u16 {
    px.clamp(MIN_FONT_PX as u32, MAX_FONT_PX as u32) as u16
}

/// Typeface family, mapped from OS/2 face names by [`FontSpec::from_facename`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontFamily {
    /// IBM Plex Sans — the default (System Proportional, WarpSans, Helv, …).
    Sans,
    /// IBM Plex Mono — fixed pitch (Courier, System Monospaced, System VIO, …).
    Mono,
    /// IBM Plex Serif — (Tms Rmn, Times New Roman, …).
    Serif,
}

/// A concrete font selection: family, style and line height in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontSpec {
    pub family: FontFamily,
    pub bold: bool,
    pub italic: bool,
    /// Line height (ascent + descent) in pixels.
    pub px: u16,
}

impl FontSpec {
    /// The PM system font: IBM Plex Sans Regular at [`SYSTEM_FONT_PX`].
    pub const SYSTEM: FontSpec =
        FontSpec { family: FontFamily::Sans, bold: false, italic: false, px: SYSTEM_FONT_PX };

    /// Bold system font, used for window title bars.
    pub const SYSTEM_BOLD: FontSpec =
        FontSpec { family: FontFamily::Sans, bold: true, italic: false, px: SYSTEM_FONT_PX };

    /// Build a spec from an OS/2 face name (FATTRS `szFacename`) plus the
    /// requested style and line height. `px == 0` selects the system size.
    ///
    /// Face names are matched case-insensitively by substring, so "Courier
    /// New", "System Monospaced" and "IBM Plex Mono" all map to
    /// [`FontFamily::Mono`]. Style words inside the name ("Helv Bold",
    /// "Times New Roman Italic") also switch the style on. Unknown and empty
    /// names fall back to Sans.
    pub fn from_facename(facename: &str, bold: bool, italic: bool, px: u32) -> FontSpec {
        let name = facename.to_ascii_lowercase();
        let family = if name.contains("sans") {
            FontFamily::Sans
        } else if ["courier", "mono", "system vio", "fixed", "typewriter"]
            .iter().any(|k| name.contains(k))
        {
            FontFamily::Mono
        } else if ["tms rmn", "times", "roman", "serif"].iter().any(|k| name.contains(k)) {
            FontFamily::Serif
        } else {
            FontFamily::Sans
        };
        let px = if px == 0 { SYSTEM_FONT_PX } else { clamp_px(px) };
        FontSpec {
            family,
            bold: bold || name.contains("bold"),
            italic: italic || name.contains("italic") || name.contains("oblique"),
            px,
        }
    }

    /// Index of this spec's Latin face in [`FONT_DATA`].
    fn latin_face(self) -> usize {
        let family = match self.family {
            FontFamily::Sans => 0,
            FontFamily::Mono => 1,
            FontFamily::Serif => 2,
        };
        family * 4 + usize::from(self.bold) + 2 * usize::from(self.italic)
    }
}

/// Integer pixel metrics of a font, as reported to guests by the GPI font APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontMetricsPx {
    /// Pixels above the baseline.
    pub ascent: i32,
    /// Pixels below the baseline (positive).
    pub descent: i32,
    /// `ascent + descent`.
    pub line_height: i32,
    /// Mean advance of the lowercase Latin letters.
    pub avg_char_width: i32,
    /// Largest advance among printable ASCII.
    pub max_char_width: i32,
    /// Height of a lowercase `x`.
    pub x_height: i32,
}

// ── Embedded font data ───────────────────────────────────────────────────────

macro_rules! plex {
    ($file:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/ibm-plex/", $file))
    };
}

/// Latin faces are indexed `family * 4 + bold + 2 * italic`; the CJK faces follow.
static FONT_DATA: [&[u8]; FACE_COUNT] = [
    plex!("IBMPlexSans-Regular.ttf"),
    plex!("IBMPlexSans-Bold.ttf"),
    plex!("IBMPlexSans-Italic.ttf"),
    plex!("IBMPlexSans-BoldItalic.ttf"),
    plex!("IBMPlexMono-Regular.ttf"),
    plex!("IBMPlexMono-Bold.ttf"),
    plex!("IBMPlexMono-Italic.ttf"),
    plex!("IBMPlexMono-BoldItalic.ttf"),
    plex!("IBMPlexSerif-Regular.ttf"),
    plex!("IBMPlexSerif-Bold.ttf"),
    plex!("IBMPlexSerif-Italic.ttf"),
    plex!("IBMPlexSerif-BoldItalic.ttf"),
    plex!("IBMPlexSansJP-Regular.ttf"),
    plex!("IBMPlexSansKR-Regular.ttf"),
    plex!("IBMPlexSansTC-Regular.ttf"),
    plex!("IBMPlexSansSC-Regular.ttf"),
];

const FACE_COUNT: usize = 16;
const FACE_JP: usize = 12;
const FACE_KR: usize = 13;
const FACE_TC: usize = 14;
const FACE_SC: usize = 15;

/// Parsed faces. Parsing is lazy and zero-copy, so the multi-megabyte CJK
/// fonts cost nothing until a character actually needs them.
static FACES: [OnceLock<Option<FontRef<'static>>>; FACE_COUNT] =
    [const { OnceLock::new() }; FACE_COUNT];

fn face(id: usize) -> Option<&'static FontRef<'static>> {
    FACES[id]
        .get_or_init(|| FontRef::try_from_slice(FONT_DATA[id]).ok())
        .as_ref()
}

// ── CJK face preference ──────────────────────────────────────────────────────

/// Active guest codepage, used only to order the CJK fallback faces.
static CJK_CODEPAGE: AtomicU32 = AtomicU32::new(0);

/// Tell the font layer which codepage the guest process uses, so that Han
/// ideographs are drawn with the matching regional face (Japanese forms under
/// CP932, Simplified Chinese under CP936, and so on).
pub fn set_active_codepage(cp: u32) {
    CJK_CODEPAGE.store(cp, Ordering::Relaxed);
}

/// CJK fallback faces in preference order for `cp`.
fn cjk_order(cp: u32) -> [usize; 4] {
    match cp {
        949 | 1361 => [FACE_KR, FACE_JP, FACE_SC, FACE_TC],
        950 | 938 | 948 => [FACE_TC, FACE_SC, FACE_JP, FACE_KR],
        936 | 1381 | 1386 => [FACE_SC, FACE_TC, FACE_JP, FACE_KR],
        // CP932/942/943 and every non-DBCS codepage.
        _ => [FACE_JP, FACE_SC, FACE_TC, FACE_KR],
    }
}

// ── Glyph resolution and rasterisation ───────────────────────────────────────

/// Where a character's glyph comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GlyphSource {
    /// Outline glyph `id` of embedded face `face`.
    Outline { face: usize, id: GlyphId },
    /// 8×16 GNU Unifont bitmap.
    BitmapNarrow([u8; 16]),
    /// 16×16 GNU Unifont bitmap.
    BitmapWide([u8; 32]),
    /// Not drawable; advances like a narrow blank cell.
    Blank,
}

/// Unifont's baseline sits this many pixels above the bottom of its 16-px cell.
const UNIFONT_DESCENT: i32 = 2;
const UNIFONT_CELL_H: i32 = 16;
const UNIFONT_NARROW_W: i32 = 8;
const UNIFONT_WIDE_W: i32 = 16;

fn resolve(ch: char, spec: FontSpec) -> GlyphSource {
    resolve_for_codepage(ch, spec, CJK_CODEPAGE.load(Ordering::Relaxed))
}

fn resolve_for_codepage(ch: char, spec: FontSpec, cp: u32) -> GlyphSource {
    let latin = spec.latin_face();
    let cjk = cjk_order(cp);
    for id in std::iter::once(latin).chain(cjk) {
        if let Some(f) = face(id) {
            let gid = f.glyph_id(ch);
            if gid.0 != 0 {
                return GlyphSource::Outline { face: id, id: gid };
            }
        }
    }
    let narrow = get_glyph_for_char(ch);
    if narrow.iter().any(|&b| b != 0) {
        return GlyphSource::BitmapNarrow(narrow);
    }
    let wide = get_glyph_dbcs(ch);
    if wide.iter().any(|&b| b != 0) {
        return GlyphSource::BitmapWide(wide);
    }
    GlyphSource::Blank
}

/// `PxScale` for `face_id` such that its em size matches the em size of the
/// spec's Latin face at the requested line height. Fallback faces have
/// different ascent/descent ratios, so scaling by line height alone would
/// make CJK glyphs visibly smaller or larger than the Latin text beside them.
fn scale_for(face_id: usize, spec: FontSpec) -> f32 {
    let px = spec.px as f32;
    let primary_id = spec.latin_face();
    if face_id == primary_id {
        return px;
    }
    let (Some(primary), Some(f)) = (face(primary_id), face(face_id)) else { return px };
    let em_px = px * primary.units_per_em().unwrap_or(1000.0) / primary.height_unscaled();
    em_px * f.height_unscaled() / f.units_per_em().unwrap_or(1000.0)
}

/// A rasterised outline glyph: coverage bitmap plus placement relative to the
/// pen position on the baseline.
struct Raster {
    left: i32,
    /// Offset of the bitmap's top row from the baseline (negative = above).
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
    advance: f32,
}

type RasterKey = (usize, u16, u32);

/// Upper bound on cached glyph bitmaps; the cache is simply cleared when full.
const RASTER_CACHE_MAX: usize = 4096;

fn raster_cache() -> &'static Mutex<HashMap<RasterKey, std::sync::Arc<Raster>>> {
    static CACHE: OnceLock<Mutex<HashMap<RasterKey, std::sync::Arc<Raster>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn rasterize(face_id: usize, gid: GlyphId, scale: f32) -> Option<std::sync::Arc<Raster>> {
    let key = (face_id, gid.0, scale.to_bits());
    let mut cache = raster_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = cache.get(&key) {
        return Some(r.clone());
    }
    let f = face(face_id)?;
    let px_scale = PxScale::from(scale);
    let advance = f.as_scaled(px_scale).h_advance(gid);
    let raster = match f.outline_glyph(gid.with_scale_and_position(px_scale, point(0.0, 0.0))) {
        Some(outline) => {
            let b = outline.px_bounds();
            let (width, height) = (b.width() as usize, b.height() as usize);
            let mut coverage = vec![0u8; width * height];
            outline.draw(|x, y, c| {
                if let Some(p) = coverage.get_mut(y as usize * width + x as usize) {
                    *p = (c.clamp(0.0, 1.0) * 255.0) as u8;
                }
            });
            Raster { left: b.min.x as i32, top: b.min.y as i32, width, height, coverage, advance }
        }
        // Glyphs without an outline (space) only advance the pen.
        None => Raster { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new(), advance },
    };
    if cache.len() >= RASTER_CACHE_MAX {
        cache.clear();
    }
    let raster = std::sync::Arc::new(raster);
    cache.insert(key, raster.clone());
    Some(raster)
}

/// Horizontal advance of `src` in pixels.
fn advance_of(src: GlyphSource, spec: FontSpec) -> f32 {
    match src {
        GlyphSource::Outline { face: id, id: gid } => face(id)
            .map(|f| f.as_scaled(PxScale::from(scale_for(id, spec))).h_advance(gid))
            .unwrap_or(0.0),
        GlyphSource::BitmapNarrow(_) | GlyphSource::Blank => UNIFONT_NARROW_W as f32,
        GlyphSource::BitmapWide(_) => UNIFONT_WIDE_W as f32,
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Width of `text` in pixels when drawn with `spec`.
pub fn text_width(text: &str, spec: FontSpec) -> i32 {
    text.chars().map(|ch| advance_of(resolve(ch, spec), spec)).sum::<f32>().round() as i32
}

/// Pixel metrics of `spec`'s Latin face.
pub fn metrics(spec: FontSpec) -> FontMetricsPx {
    static CACHE: OnceLock<Mutex<HashMap<FontSpec, FontMetricsPx>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = cache.get(&spec) {
        return *m;
    }
    let px = spec.px as i32;
    let m = match face(spec.latin_face()) {
        Some(f) => {
            let scaled = f.as_scaled(PxScale::from(spec.px as f32));
            let ascent = scaled.ascent().round() as i32;
            // Keep ascent + descent equal to the requested line height.
            let descent = (px - ascent).max(0);
            let adv = |ch: char| scaled.h_advance(f.glyph_id(ch));
            let avg = ('a'..='z').map(adv).sum::<f32>() / 26.0;
            let max = (' '..='~').map(adv).fold(0.0f32, f32::max);
            let x_height = f
                .outline_glyph(f.glyph_id('x').with_scale(PxScale::from(spec.px as f32)))
                .map(|o| o.px_bounds().height().round() as i32)
                .unwrap_or(ascent / 2);
            FontMetricsPx {
                ascent,
                descent,
                line_height: ascent + descent,
                avg_char_width: avg.round().max(1.0) as i32,
                max_char_width: max.ceil().max(1.0) as i32,
                x_height,
            }
        }
        None => FontMetricsPx {
            ascent: px * 3 / 4,
            descent: px - px * 3 / 4,
            line_height: px,
            avg_char_width: (px / 2).max(1),
            max_char_width: (px / 2).max(1),
            x_height: px / 2,
        },
    };
    cache.insert(spec, m);
    m
}

/// Blend `color` (0x00RRGGBB) over `dst` with 8-bit coverage `cov`.
fn blend(dst: u32, color: u32, cov: u8) -> u32 {
    if cov == 255 {
        return color & 0x00FF_FFFF;
    }
    let a = cov as u32;
    let ch = |shift: u32| {
        let d = (dst >> shift) & 0xFF;
        let s = (color >> shift) & 0xFF;
        ((s * a + d * (255 - a) + 127) / 255) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// Draw `text` into a top-left-origin pixel buffer of `buf_w` × `buf_h`
/// 0x00RRGGBB pixels. `x` is the left edge of the first glyph's pen position
/// and `baseline` the screen row of the text baseline. Pixels outside the
/// buffer are clipped. Returns the pen advance in pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    x: i32, baseline: i32, text: &str, color: u32, spec: FontSpec,
) -> i32 {
    let buf_h = buf_h.min(buf.len() / buf_w.max(1));
    let mut put = |px: i32, py: i32, cov: u8| {
        if cov != 0 && px >= 0 && py >= 0 && (px as usize) < buf_w && (py as usize) < buf_h {
            let p = &mut buf[py as usize * buf_w + px as usize];
            *p = blend(*p, color, cov);
        }
    };
    let mut pen = 0.0f32;
    for ch in text.chars() {
        let src = resolve(ch, spec);
        let gx = x + pen.round() as i32;
        match src {
            GlyphSource::Outline { face: id, id: gid } => {
                if let Some(r) = rasterize(id, gid, scale_for(id, spec)) {
                    for row in 0..r.height {
                        for col in 0..r.width {
                            put(gx + r.left + col as i32, baseline + r.top + row as i32,
                                r.coverage[row * r.width + col]);
                        }
                    }
                    pen += r.advance;
                    continue;
                }
            }
            GlyphSource::BitmapNarrow(rows) => {
                let top = baseline + UNIFONT_DESCENT - UNIFONT_CELL_H;
                for (row, bits) in rows.iter().enumerate() {
                    for col in 0..UNIFONT_NARROW_W {
                        if bits & (0x80 >> col) != 0 { put(gx + col, top + row as i32, 255); }
                    }
                }
            }
            GlyphSource::BitmapWide(bytes) => {
                let top = baseline + UNIFONT_DESCENT - UNIFONT_CELL_H;
                for (row, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
                    let bits = u16::from_be_bytes(*pair);
                    for col in 0..UNIFONT_WIDE_W {
                        if bits & (0x8000 >> col) != 0 { put(gx + col, top + row as i32, 255); }
                    }
                }
            }
            GlyphSource::Blank => {}
        }
        pen += advance_of(src, spec);
    }
    pen.round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 240;
    const H: usize = 60;
    const BG: u32 = 0x00FF_FFFF;

    fn painted(buf: &[u32]) -> Vec<(usize, usize)> {
        buf.iter().enumerate().filter(|&(_, &p)| p != BG).map(|(i, _)| (i % W, i / W)).collect()
    }

    #[test]
    fn every_embedded_face_parses() {
        for id in 0..FACE_COUNT {
            assert!(face(id).is_some(), "embedded face {id} failed to parse");
        }
    }

    #[test]
    fn sans_is_proportional_and_mono_is_fixed_pitch() {
        let sans = FontSpec::SYSTEM;
        let mono = FontSpec { family: FontFamily::Mono, ..FontSpec::SYSTEM };
        assert!(text_width("iiii", sans) < text_width("WWWW", sans));
        assert_eq!(text_width("iiii", mono), text_width("WWWW", mono));
        assert_eq!(text_width("", sans), 0);
    }

    #[test]
    fn bold_and_larger_sizes_are_wider() {
        let text = "Presentation Manager";
        let regular = text_width(text, FontSpec::SYSTEM);
        assert!(text_width(text, FontSpec::SYSTEM_BOLD) > regular);
        assert!(text_width(text, FontSpec { px: 32, ..FontSpec::SYSTEM }) > regular * 3 / 2);
    }

    #[test]
    fn metrics_fill_the_requested_line_height() {
        for px in [12u16, 16, 24, 48] {
            let m = metrics(FontSpec { px, ..FontSpec::SYSTEM });
            assert_eq!(m.line_height, px as i32);
            assert_eq!(m.ascent + m.descent, m.line_height);
            assert!(m.ascent > m.descent && m.descent > 0);
            assert!(m.avg_char_width > 0 && m.avg_char_width <= m.max_char_width);
            assert!(m.x_height > 0 && m.x_height < m.ascent);
        }
    }

    #[test]
    fn draw_text_stays_inside_the_line_box_and_reports_its_advance() {
        let mut buf = vec![BG; W * H];
        let spec = FontSpec::SYSTEM;
        let m = metrics(spec);
        let (x, baseline) = (10, 30);
        let adv = draw_text(&mut buf, W, H, x, baseline, "Warpine jq", 0, spec);
        assert_eq!(adv, text_width("Warpine jq", spec));
        let px = painted(&buf);
        assert!(!px.is_empty(), "text must paint pixels");
        let (min_x, max_x) = (px.iter().map(|p| p.0).min().unwrap(), px.iter().map(|p| p.0).max().unwrap());
        let (min_y, max_y) = (px.iter().map(|p| p.1).min().unwrap(), px.iter().map(|p| p.1).max().unwrap());
        assert!(min_x as i32 >= x - 1 && (max_x as i32) <= x + adv + 1);
        assert!(min_y as i32 >= baseline - m.ascent - 1, "nothing above the ascent");
        assert!((max_y as i32) < baseline + m.descent + 1, "nothing below the descent");
        assert!(max_y as i32 > baseline, "descenders of j and q go below the baseline");
    }

    #[test]
    fn draw_text_is_antialiased_and_clips_at_buffer_edges() {
        let mut buf = vec![BG; W * H];
        draw_text(&mut buf, W, H, 4, 30, "S", 0, FontSpec { px: 40, ..FontSpec::SYSTEM });
        assert!(buf.iter().any(|&p| p != BG && p != 0), "expected partially covered edge pixels");
        // Entirely and partially off-buffer text must not panic.
        draw_text(&mut buf, W, H, -500, -500, "clip", 0, FontSpec::SYSTEM);
        draw_text(&mut buf, W, H, W as i32 - 3, H as i32 + 2, "clip", 0, FontSpec::SYSTEM);
    }

    #[test]
    fn cjk_text_uses_the_plex_cjk_faces_ordered_by_codepage() {
        let spec = FontSpec::SYSTEM;
        // Kana exists only in the Japanese face, Hangul only in the Korean one.
        assert_eq!(resolve('あ', spec), GlyphSource::Outline {
            face: FACE_JP, id: face(FACE_JP).unwrap().glyph_id('あ') });
        assert!(matches!(resolve('한', spec), GlyphSource::Outline { face: FACE_KR, .. }));
        // A shared Han ideograph follows the codepage's regional face. The
        // Korean face carries no Hanja, so CP949 falls through to the next one.
        for (cp, first, han) in [(932, FACE_JP, FACE_JP), (936, FACE_SC, FACE_SC),
                                 (950, FACE_TC, FACE_TC), (949, FACE_KR, FACE_JP)] {
            assert_eq!(cjk_order(cp)[0], first);
            assert!(matches!(resolve_for_codepage('中', spec, cp),
                             GlyphSource::Outline { face, .. } if face == han), "cp {cp}");
        }
        // CJK glyphs are scaled to the Latin face's em size: full-width, not tiny.
        let w = text_width("中", spec);
        assert!((10..=16).contains(&w), "unexpected CJK advance {w}");
    }

    #[test]
    fn characters_missing_from_plex_fall_back_to_unifont() {
        // Runic letters are in GNU Unifont but in no IBM Plex face.
        let src = resolve('ᚠ', FontSpec::SYSTEM);
        assert!(matches!(src, GlyphSource::BitmapNarrow(_) | GlyphSource::BitmapWide(_)), "{src:?}");
        let mut buf = vec![BG; W * H];
        draw_text(&mut buf, W, H, 10, 30, "ᚠ", 0, FontSpec::SYSTEM);
        assert!(!painted(&buf).is_empty());
        // A control character paints nothing.
        let mut buf = vec![BG; W * H];
        draw_text(&mut buf, W, H, 10, 30, "\0", 0, FontSpec::SYSTEM);
        assert!(painted(&buf).is_empty());
        // A codepoint in neither Plex nor Unifont advances like a blank narrow cell.
        assert_eq!(resolve('\u{10FFFF}', FontSpec::SYSTEM), GlyphSource::Blank);
        assert_eq!(text_width("\u{10FFFF}", FontSpec::SYSTEM), UNIFONT_NARROW_W);
    }

    #[test]
    fn facenames_map_to_plex_families_and_styles() {
        let f = |name| FontSpec::from_facename(name, false, false, 0);
        assert_eq!(f(""), FontSpec::SYSTEM);
        assert_eq!(f("Helv").family, FontFamily::Sans);
        assert_eq!(f("WarpSans").family, FontFamily::Sans);
        assert_eq!(f("MS Sans Serif").family, FontFamily::Sans);
        assert_eq!(f("Courier New").family, FontFamily::Mono);
        assert_eq!(f("System Monospaced").family, FontFamily::Mono);
        assert_eq!(f("System VIO").family, FontFamily::Mono);
        assert_eq!(f("Tms Rmn").family, FontFamily::Serif);
        assert_eq!(f("Times New Roman").family, FontFamily::Serif);
        let styled = f("Times New Roman Bold Italic");
        assert!(styled.bold && styled.italic);
        let sel = FontSpec::from_facename("Helv", true, true, 24);
        assert!(sel.bold && sel.italic && sel.px == 24);
        assert_eq!(FontSpec::from_facename("Helv", false, false, 100_000).px, MAX_FONT_PX);
        assert_eq!(FontSpec::from_facename("Helv", false, false, 1).px, MIN_FONT_PX);
    }
}
