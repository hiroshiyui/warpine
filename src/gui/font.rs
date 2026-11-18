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

use std::cell::RefCell;
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
    /// Which regional CJK face is tried first for characters the Latin face
    /// lacks.
    pub cjk: CjkRegion,
}

/// Regional preference among the CJK fallback faces. Han ideographs are
/// shared between the Japanese, Chinese and Korean fonts but drawn
/// differently, so the right face depends on the text's codepage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CjkRegion {
    /// Follow the process codepage at the time the text is rasterised
    /// (see [`set_active_codepage`]). Used for chrome and control text.
    Auto,
    Japanese,
    Korean,
    TraditionalChinese,
    SimplifiedChinese,
}

impl CjkRegion {
    /// Region for a guest codepage; non-DBCS codepages map to Japanese, the
    /// face with the broadest coverage.
    pub fn from_codepage(cp: u32) -> CjkRegion {
        match cp {
            949 | 1361 => CjkRegion::Korean,
            950 | 938 | 948 => CjkRegion::TraditionalChinese,
            936 | 1381 | 1386 => CjkRegion::SimplifiedChinese,
            _ => CjkRegion::Japanese,
        }
    }
}

impl FontSpec {
    /// The PM system font: IBM Plex Sans Regular at [`SYSTEM_FONT_PX`].
    pub const SYSTEM: FontSpec =
        FontSpec { family: FontFamily::Sans, bold: false, italic: false, px: SYSTEM_FONT_PX,
                   cjk: CjkRegion::Auto };

    /// Bold system font, used for window title bars.
    pub const SYSTEM_BOLD: FontSpec =
        FontSpec { family: FontFamily::Sans, bold: true, italic: false, px: SYSTEM_FONT_PX,
                   cjk: CjkRegion::Auto };

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
            cjk: CjkRegion::Auto,
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

/// Tell the font layer which codepage the guest process uses, so that text
/// drawn with [`CjkRegion::Auto`] uses the matching regional face (Japanese
/// forms under CP932, Simplified Chinese under CP936, and so on).
pub fn set_active_codepage(cp: u32) {
    CJK_CODEPAGE.store(cp, Ordering::Relaxed);
}

/// CJK fallback faces in preference order for `region`.
fn cjk_order(region: CjkRegion) -> [usize; 4] {
    let region = match region {
        CjkRegion::Auto => CjkRegion::from_codepage(CJK_CODEPAGE.load(Ordering::Relaxed)),
        r => r,
    };
    match region {
        CjkRegion::Korean => [FACE_KR, FACE_JP, FACE_SC, FACE_TC],
        CjkRegion::TraditionalChinese => [FACE_TC, FACE_SC, FACE_JP, FACE_KR],
        CjkRegion::SimplifiedChinese => [FACE_SC, FACE_TC, FACE_JP, FACE_KR],
        CjkRegion::Japanese | CjkRegion::Auto => [FACE_JP, FACE_SC, FACE_TC, FACE_KR],
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
pub(crate) const UNIFONT_DESCENT: i32 = 2;
pub(crate) const UNIFONT_CELL_H: i32 = 16;
pub(crate) const UNIFONT_NARROW_W: i32 = 8;
pub(crate) const UNIFONT_WIDE_W: i32 = 16;

fn resolve(ch: char, spec: FontSpec) -> GlyphSource {
    let latin = spec.latin_face();
    let cjk = cjk_order(spec.cjk);
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

/// Horizontal glyph positions per pixel: outline glyphs are rasterised at
/// the pen's fraction of a pixel, rounded to a quarter, so the spaces
/// between letters follow the font's fractional advances instead of
/// each glyph's advance being rounded on its own.
pub const SUBPIXEL_STEPS: u8 = 4;

/// The whole pixel and the subpixel phase (`0..SUBPIXEL_STEPS`) of pen
/// position `pen`.
pub fn pen_phase(pen: f32) -> (i32, u8) {
    let base = pen.floor();
    let phase = ((pen - base) * f32::from(SUBPIXEL_STEPS)).round() as u8;
    if phase >= SUBPIXEL_STEPS { (base as i32 + 1, 0) } else { (base as i32, phase) }
}

/// The contrast curve applied to outline coverage: `c^(1 / TEXT_CONTRAST)`
/// darkens the anti-aliased edges, so thin stems read as ink instead of
/// grey (ADR-0047).
pub const TEXT_CONTRAST: f32 = 1.45;

/// Coverage (0.0–1.0) through the contrast curve, as a byte.
pub fn contrast_coverage(c: f32) -> u8 {
    (c.clamp(0.0, 1.0).powf(1.0 / TEXT_CONTRAST) * 255.0 + 0.5) as u8
}

/// The faces drawn with light hinting: the Latin ones (the CJK faces stay
/// unhinted; the autohinter's per-font analysis of tens of thousands of
/// glyphs would cost too much).
fn hinted_face(face_id: usize) -> bool {
    face_id < FACE_JP
}

/// Bitmaps larger than this (per side) are not rasterised.
const MAX_GLYPH_BITMAP: f32 = 1024.0;

/// `skrifa`'s view of face `id` and its autohinter glyph styles, computed
/// once per face.
fn hint_face(id: usize) -> Option<&'static (skrifa::FontRef<'static>, skrifa::outline::GlyphStyles)> {
    use skrifa::MetadataProvider;
    static HINT_FACES: [OnceLock<Option<(skrifa::FontRef<'static>, skrifa::outline::GlyphStyles)>>; FACE_COUNT] =
        [const { OnceLock::new() }; FACE_COUNT];
    HINT_FACES[id].get_or_init(|| {
        let f = skrifa::FontRef::new(FONT_DATA[id]).ok()?;
        let styles = skrifa::outline::GlyphStyles::new(&f.outline_glyphs());
        Some((f, styles))
    }).as_ref()
}

thread_local! {
    /// Light-hinting instances per face and size (ppem bits), per thread.
    static HINTERS: RefCell<HashMap<(usize, u32), Option<skrifa::outline::HintingInstance>>> =
        RefCell::new(HashMap::new());
}

/// Upper bound on cached hinting instances per thread.
const HINTERS_MAX: usize = 64;

/// A glyph outline in pixels (y up), from `skrifa`'s pen.
#[derive(Default)]
struct PathPen {
    segs: Vec<Seg>,
    start: (f32, f32),
    at: (f32, f32),
}

#[derive(Clone, Copy)]
enum Seg {
    Line([(f32, f32); 2]),
    Quad([(f32, f32); 3]),
    Cubic([(f32, f32); 4]),
}

impl skrifa::outline::OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) { self.close(); self.start = (x, y); self.at = (x, y); }
    fn line_to(&mut self, x: f32, y: f32) { self.segs.push(Seg::Line([self.at, (x, y)])); self.at = (x, y); }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segs.push(Seg::Quad([self.at, (cx, cy), (x, y)]));
        self.at = (x, y);
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.segs.push(Seg::Cubic([self.at, (c0x, c0y), (c1x, c1y), (x, y)]));
        self.at = (x, y);
    }
    fn close(&mut self) {
        if self.at != self.start { self.segs.push(Seg::Line([self.at, self.start])); }
        self.at = self.start;
    }
}

/// Rasterise glyph `gid` of Latin face `face_id` at ab_glyph scale `scale`
/// with light (vertical-only) autohinting (ADR-0047): the outline from
/// `skrifa`, hinted for `FT_LOAD_TARGET_LIGHT`-like output keeping the
/// linear advances, then filled by `ab_glyph_rasterizer` at subpixel
/// `phase`. `None` when hinting is not available for it.
fn rasterize_hinted(face_id: usize, gid: GlyphId, scale: f32, phase: u8, advance: f32) -> Option<Raster> {
    use skrifa::MetadataProvider;
    use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, SmoothMode, Target};
    if !hinted_face(face_id) { return None; }
    let f = face(face_id)?;
    let (sk, styles) = hint_face(face_id)?;
    let ppem = scale * f.units_per_em()? / f.height_unscaled();
    if !(ppem.is_finite() && ppem > 0.0 && ppem < MAX_GLYPH_BITMAP) { return None; }
    let outlines = sk.outline_glyphs();
    let glyph = outlines.get(skrifa::GlyphId::new(u32::from(gid.0)))?;
    let mut pen = PathPen::default();
    HINTERS.with(|h| {
        let mut h = h.borrow_mut();
        if h.len() >= HINTERS_MAX && !h.contains_key(&(face_id, ppem.to_bits())) { h.clear(); }
        let inst = h.entry((face_id, ppem.to_bits())).or_insert_with(|| {
            let options = HintingOptions {
                engine: Engine::Auto(Some(styles.clone())),
                target: Target::Smooth { mode: SmoothMode::Light, symmetric_rendering: false, preserve_linear_metrics: true },
            };
            HintingInstance::new(&outlines, skrifa::instance::Size::new(ppem), skrifa::instance::LocationRef::default(), options).ok()
        });
        let inst = inst.as_ref()?;
        glyph.draw(DrawSettings::hinted(inst, false), &mut pen).ok()
    })?;
    skrifa::outline::OutlinePen::close(&mut pen);
    if pen.segs.is_empty() {
        return Some(Raster { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new(), advance });
    }
    let off = f32::from(phase) / f32::from(SUBPIXEL_STEPS);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let pts = |s: &Seg| -> Vec<(f32, f32)> {
        match *s { Seg::Line(p) => p.to_vec(), Seg::Quad(p) => p.to_vec(), Seg::Cubic(p) => p.to_vec() }
    };
    for s in &pen.segs {
        for (x, y) in pts(s) {
            (x0, y0, x1, y1) = (x0.min(x + off), y0.min(y), x1.max(x + off), y1.max(y));
        }
    }
    if !(x0.is_finite() && y0.is_finite() && x1 - x0 < MAX_GLYPH_BITMAP && y1 - y0 < MAX_GLYPH_BITMAP) { return None; }
    let (left, top) = (x0.floor(), (-y1).floor());
    let width = ((x1.ceil() - left) as usize).max(1);
    let height = ((-y0).ceil() - top).max(1.0) as usize;
    // Bitmap coordinates: x right of `left`, y down from `top`.
    let p = |(x, y): (f32, f32)| ab_glyph_rasterizer::point(x + off - left, -y - top);
    let mut r = ab_glyph_rasterizer::Rasterizer::new(width, height);
    for s in &pen.segs {
        match *s {
            Seg::Line([a, b]) => r.draw_line(p(a), p(b)),
            Seg::Quad([a, b, c]) => r.draw_quad(p(a), p(b), p(c)),
            Seg::Cubic([a, b, c, d]) => r.draw_cubic(p(a), p(b), p(c), p(d)),
        }
    }
    let mut coverage = vec![0u8; width * height];
    r.for_each_pixel(|i, c| if let Some(px) = coverage.get_mut(i) { *px = contrast_coverage(c); });
    Some(Raster { left: left as i32, top: top as i32, width, height, coverage, advance })
}

fn rasterize(face_id: usize, gid: GlyphId, scale: f32, phase: u8) -> Option<Raster> {
    let f = face(face_id)?;
    if let Some(r) = rasterize_hinted(face_id, gid, scale, phase, f.as_scaled(PxScale::from(scale)).h_advance(gid)) {
        return Some(r);
    }
    let px_scale = PxScale::from(scale);
    let advance = f.as_scaled(px_scale).h_advance(gid);
    let offset = f32::from(phase) / f32::from(SUBPIXEL_STEPS);
    Some(match f.outline_glyph(gid.with_scale_and_position(px_scale, point(offset, 0.0))) {
        Some(outline) => {
            let b = outline.px_bounds();
            let (width, height) = (b.width() as usize, b.height() as usize);
            let mut coverage = vec![0u8; width * height];
            outline.draw(|x, y, c| {
                if let Some(p) = coverage.get_mut(y as usize * width + x as usize) {
                    *p = contrast_coverage(c);
                }
            });
            Raster { left: b.min.x as i32, top: b.min.y as i32, width, height, coverage, advance }
        }
        // Glyphs without an outline (space) only advance the pen.
        None => Raster { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new(), advance },
    })
}

/// One character in one font, ready to measure and draw.
struct Glyph {
    src: GlyphSource,
    /// Pen advance in pixels.
    advance: f32,
    /// Coverage bitmap for outline glyphs.
    raster: Option<Raster>,
}

impl Glyph {
    /// The glyph rasterised at subpixel `phase` (see [`SUBPIXEL_STEPS`]).
    fn load_at(ch: char, spec: FontSpec, phase: u8) -> Glyph {
        let src = resolve(ch, spec);
        let raster = match src {
            GlyphSource::Outline { face: id, id: gid } => rasterize(id, gid, scale_for(id, spec), phase),
            _ => None,
        };
        let advance = raster.as_ref().map_or_else(|| advance_of(src, spec), |r| r.advance);
        Glyph { src, advance, raster }
    }
}

/// Glyph cache key: the character, the font, the CJK face order in
/// effect (`CjkRegion::Auto` follows the process codepage, which can
/// change) and the subpixel phase it is rasterised at.
type GlyphKey = (char, FontSpec, [usize; 4], u8);

/// Upper bound on cached glyphs per thread; the cache is cleared when full.
const GLYPH_CACHE_MAX: usize = 4096;

thread_local! {
    /// Per-thread glyph cache. Text is drawn almost only on the GUI thread,
    /// so a thread-local cache costs no lock and no reference counting per
    /// glyph; the API threads only measure text and fill their own.
    static GLYPH_CACHE: RefCell<HashMap<GlyphKey, Glyph>> = RefCell::new(HashMap::new());
}

/// Run `f` on the cached glyph for `ch` in `spec`, loading it on first use.
/// `f` must not use the glyph cache itself.
fn with_glyph<R>(ch: char, spec: FontSpec, f: impl FnOnce(&Glyph) -> R) -> R {
    with_glyph_at(ch, spec, 0, f)
}

/// [`with_glyph`] for the glyph rasterised at subpixel `phase`.
fn with_glyph_at<R>(ch: char, spec: FontSpec, phase: u8, f: impl FnOnce(&Glyph) -> R) -> R {
    let key = (ch, spec, cjk_order(spec.cjk), phase);
    GLYPH_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= GLYPH_CACHE_MAX && !cache.contains_key(&key) {
            cache.clear();
        }
        f(cache.entry(key).or_insert_with(|| Glyph::load_at(ch, spec, phase)))
    })
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
    text.chars().map(|ch| with_glyph(ch, spec, |g| g.advance)).sum::<f32>().round() as i32
}

/// The longest prefix of `text` that fits in `max_width` pixels when drawn
/// with `spec`. Used to clip labels to the control they are drawn in.
pub fn fit_text(text: &str, max_width: i32, spec: FontSpec) -> &str {
    let mut pen = 0.0f32;
    for (idx, ch) in text.char_indices() {
        pen += with_glyph(ch, spec, |g| g.advance);
        if pen.round() as i32 > max_width {
            return &text[..idx];
        }
    }
    text
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

/// Advance of one character in `spec`, in (fractional) pixels: the amounts
/// [`text_width`] adds up. Uses the glyph cache when the glyph is there and
/// otherwise reads the advance from the font without rasterising the glyph,
/// so measuring many characters (GPI width tables) stays cheap.
pub fn char_advance(ch: char, spec: FontSpec) -> f32 {
    let key = (ch, spec, cjk_order(spec.cjk), 0);
    GLYPH_CACHE.with(|cache| cache.borrow().get(&key).map(|g| g.advance))
        .unwrap_or_else(|| advance_of(resolve(ch, spec), spec))
}

/// The shape of a character's glyph, for drawing it under a transform
/// (`glyph_transform.rs`).
pub(crate) enum GlyphShape {
    /// Outline in font units (y up), with the factor from font units to
    /// pixels.
    Outline { outline: ab_glyph::Outline, scale: ab_glyph::PxScaleFactor },
    /// 8×16 GNU Unifont bitmap (rows top first, MSB = left pel).
    Narrow([u8; 16]),
    /// 16×16 GNU Unifont bitmap (two bytes per row, big-endian).
    Wide([u8; 32]),
    /// Nothing to draw.
    Blank,
}

/// The glyph shape of `ch` in `spec`, from the same face the upright path
/// would draw it from.
pub(crate) fn glyph_shape(ch: char, spec: FontSpec) -> GlyphShape {
    match resolve(ch, spec) {
        GlyphSource::Outline { face: id, id: gid } => face(id)
            .and_then(|f| {
                let scale = f.as_scaled(PxScale::from(scale_for(id, spec))).scale_factor();
                Some(GlyphShape::Outline { outline: f.outline(gid)?, scale })
            })
            .unwrap_or(GlyphShape::Blank),
        GlyphSource::BitmapNarrow(rows) => GlyphShape::Narrow(rows),
        GlyphSource::BitmapWide(rows) => GlyphShape::Wide(rows),
        GlyphSource::Blank => GlyphShape::Blank,
    }
}

/// Blend `color` (0x00RRGGBB) over `dst` with 8-bit coverage `cov`.
pub(crate) fn blend(dst: u32, color: u32, cov: u8) -> u32 {
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
    draw_text_with(buf, buf_w, buf_h, x, baseline, text, spec, |p, cov| *p = blend(*p, color, cov))
}

/// Like [`draw_text`], but instead of blending a colour, call `paint` with
/// each covered pixel and its 8-bit coverage (never 0). Lets callers apply
/// a raster operation (a GPI mix mode) instead of alpha blending.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_with(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    x: i32, baseline: i32, text: &str, spec: FontSpec,
    mut paint: impl FnMut(&mut u32, u8),
) -> i32 {
    let buf_h = buf_h.min(buf.len() / buf_w.max(1));
    let mut put = |px: i32, py: i32, cov: u8| {
        if cov != 0 && px >= 0 && py >= 0 && (px as usize) < buf_w && (py as usize) < buf_h {
            paint(&mut buf[py as usize * buf_w + px as usize], cov);
        }
    };
    let mut pen = 0.0f32;
    for ch in text.chars() {
        // Outline glyphs at the pen's quarter pixel; bitmap glyphs (Unifont)
        // at the rounded pen, as they have whole-pixel advances.
        let gx = x + pen.round() as i32;
        let (whole, phase) = pen_phase(pen);
        let ox = x + whole;
        pen += with_glyph_at(ch, spec, phase, |g| {
            match (&g.raster, g.src) {
                (Some(r), _) => {
                    for row in 0..r.height {
                        for col in 0..r.width {
                            put(ox + r.left + col as i32, baseline + r.top + row as i32,
                                r.coverage[row * r.width + col]);
                        }
                    }
                }
                (None, GlyphSource::BitmapNarrow(rows)) => {
                    let top = baseline + UNIFONT_DESCENT - UNIFONT_CELL_H;
                    for (row, bits) in rows.iter().enumerate() {
                        for col in 0..UNIFONT_NARROW_W {
                            if bits & (0x80 >> col) != 0 { put(gx + col, top + row as i32, 255); }
                        }
                    }
                }
                (None, GlyphSource::BitmapWide(bytes)) => {
                    let top = baseline + UNIFONT_DESCENT - UNIFONT_CELL_H;
                    for (row, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
                        let bits = u16::from_be_bytes(*pair);
                        for col in 0..UNIFONT_WIDE_W {
                            if bits & (0x8000 >> col) != 0 { put(gx + col, top + row as i32, 255); }
                        }
                    }
                }
                // Outline faces that failed to load, and blanks, only advance.
                (None, GlyphSource::Outline { .. } | GlyphSource::Blank) => {}
            }
            g.advance
        });
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
    fn fit_text_returns_the_longest_prefix_that_fits() {
        let spec = FontSpec::SYSTEM;
        let text = "initial text — 日本語";
        assert_eq!(fit_text(text, 10_000, spec), text);
        assert_eq!(fit_text(text, 0, spec), "");
        assert_eq!(fit_text(text, -5, spec), "");
        let w = text_width("initial", spec);
        assert_eq!(fit_text(text, w, spec), "initial");
        // Every prefix it returns fits, and adding one more character would not.
        for max in 1..text_width(text, spec) {
            let fit = fit_text(text, max, spec);
            assert!(text_width(fit, spec) <= max, "max={max}");
            let next: String = text.chars().take(fit.chars().count() + 1).collect();
            assert!(text_width(&next, spec) > max, "max={max}");
        }
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
    fn coverage_goes_through_the_contrast_curve() {
        assert_eq!(contrast_coverage(0.0), 0);
        assert_eq!(contrast_coverage(1.0), 255);
        assert!(contrast_coverage(0.5) > 150, "half coverage is darkened");
        assert_eq!(contrast_coverage(-1.0), 0);
        assert_eq!(contrast_coverage(2.0), 255);
    }

    /// Latin faces are hinted vertically: the flat top of an "x" sits on a
    /// pixel row (the row is almost fully covered), the advance is the
    /// font's own; CJK faces are not hinted.
    #[test]
    fn latin_glyphs_are_light_hinted() {
        let spec = FontSpec::SYSTEM;
        let f = face(spec.latin_face()).unwrap();
        let gid = f.glyph_id('z');
        let scale = scale_for(spec.latin_face(), spec);
        let r = rasterize_hinted(spec.latin_face(), gid, scale, 0, 1.0).expect("a hinted raster");
        // The bar's rows: hinted, its edge is on a pixel boundary, so the
        // first row is (almost) solid instead of two half-covered rows.
        let row = |k: usize| r.coverage[k * r.width..][..r.width].iter().map(|&c| u32::from(c)).max().unwrap_or(0);
        assert!(row(0) >= 240, "the x-height bar is snapped: top row {}", row(0));
        assert_eq!(r.advance, 1.0, "the advance passed in is kept");
        // Each quarter-pixel phase gives a raster.
        for phase in 1..SUBPIXEL_STEPS {
            assert!(rasterize_hinted(spec.latin_face(), gid, scale, phase, 1.0).is_some());
        }
        let cjk = face(FACE_JP).unwrap().glyph_id('日');
        assert!(rasterize_hinted(FACE_JP, cjk, scale, 0, 1.0).is_none(), "CJK stays unhinted");
        // A space has no outline: an empty raster.
        let space = rasterize_hinted(spec.latin_face(), f.glyph_id(' '), scale, 0, 4.0).unwrap();
        assert_eq!((space.width, space.coverage.len()), (0, 0));
    }

    #[test]
    fn pen_phases_are_quarter_pixels() {
        assert_eq!(pen_phase(0.0), (0, 0));
        assert_eq!(pen_phase(3.3), (3, 1));
        assert_eq!(pen_phase(3.5), (3, 2));
        assert_eq!(pen_phase(3.9), (4, 0), "rounds up to the next whole pixel");
        assert_eq!(pen_phase(-0.25), (-1, 3));
    }

    /// Glyphs land at the font's fractional pen positions: an "i" after k
    /// spaces has its ink centred k space advances (fractional) to the
    /// right of a lone "i", within a quarter pixel's rounding — not half a
    /// pixel off as when each glyph sat at the rounded pen.
    #[test]
    fn glyphs_follow_fractional_advances() {
        let spec = FontSpec::SYSTEM_BOLD;
        let space = char_advance(' ', spec);
        assert!(space.fract() != 0.0, "the test needs a fractional advance ({space})");
        let (w, h) = (200usize, 24usize);
        let centroid = |text: &str| {
            let mut buf = vec![0u32; w * h];
            draw_text(&mut buf, w, h, 2, 18, text, 0x00FF_FFFF, spec);
            let (mut sum, mut weight) = (0.0f64, 0.0f64);
            for (i, &p) in buf.iter().enumerate() {
                let c = f64::from(p & 0xFF);
                sum += c * (i % w) as f64;
                weight += c;
            }
            sum / weight
        };
        let lone = centroid("i");
        for k in 1..=8 {
            let got = centroid(&format!("{}i", " ".repeat(k))) - lone;
            let want = f64::from(space) * k as f64;
            assert!((got - want).abs() <= 0.2, "{k} spaces: ink moved {got:.2}, the pen {want:.2}");
        }
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
                                 (950, FACE_TC, FACE_TC), (949, FACE_KR, FACE_JP),
                                 (850, FACE_JP, FACE_JP)] {
            let region = CjkRegion::from_codepage(cp);
            assert_eq!(cjk_order(region)[0], first);
            assert!(matches!(resolve('中', FontSpec { cjk: region, ..spec }),
                             GlyphSource::Outline { face, .. } if face == han), "cp {cp}");
        }
        // An explicit region is fixed when the spec is built: it does not
        // follow later changes of the process codepage, unlike `Auto`.
        assert_ne!(FontSpec { cjk: CjkRegion::Korean, ..spec }, FontSpec { cjk: CjkRegion::Auto, ..spec });
        // CJK glyphs are scaled to the Latin face's em size: full-width, not tiny.
        let w = text_width("中", spec);
        assert!((10..=16).contains(&w), "unexpected CJK advance {w}");
    }

    /// Explicit regions only: `Auto` follows a process-wide codepage that
    /// other tests change concurrently.
    const MIXED: &str = "Ag 中あ한 ᚠ";
    fn mixed_spec() -> FontSpec {
        FontSpec { cjk: CjkRegion::Korean, ..FontSpec::SYSTEM }
    }

    #[test]
    fn cached_glyphs_match_freshly_resolved_ones() {
        let spec = mixed_spec();
        for ch in MIXED.chars() {
            let fresh = Glyph::load_at(ch, spec, 0);
            // Twice: the first call fills the cache, the second reads it.
            for _ in 0..2 {
                with_glyph(ch, spec, |g| {
                    assert_eq!(g.src, fresh.src, "{ch:?}");
                    assert_eq!(g.advance, fresh.advance, "{ch:?}");
                    assert_eq!(g.raster.as_ref().map(|r| &r.coverage),
                               fresh.raster.as_ref().map(|r| &r.coverage), "{ch:?}");
                });
            }
            assert_eq!(fresh.advance, advance_of(fresh.src, spec), "{ch:?}: raster and font advance agree");
        }
        let sum: f32 = MIXED.chars().map(|ch| Glyph::load_at(ch, spec, 0).advance).sum();
        assert_eq!(text_width(MIXED, spec), sum.round() as i32);
    }

    #[test]
    fn text_draws_the_same_on_another_thread() {
        let draw = || {
            let mut buf = vec![BG; W * H];
            let adv = draw_text(&mut buf, W, H, 2, 30, MIXED, 0, mixed_spec());
            (buf, adv)
        };
        let here = draw();
        let there = std::thread::spawn(draw).join().unwrap();
        assert!(here == there, "each thread's glyph cache renders identically");
        assert!(!painted(&here.0).is_empty());
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
