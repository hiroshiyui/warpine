// SPDX-License-Identifier: GPL-3.0-or-later

//! GPI areas, paths, polygons and curves (PMGPI).
//!
//! Between `GpiBeginArea` and `GpiEndArea`, or `GpiBeginPath` and
//! `GpiEndPath`, the line primitives build figures instead of drawing
//! ([`Figures`], `Loader::figure_part`): `GpiMove` starts a new figure;
//! `GpiLine`, `GpiPolyLine`, `GpiPartialArc`, `GpiPointArc`,
//! `GpiPolyFillet`, `GpiPolyFilletSharp` and `GpiPolySpline` extend the
//! current one from the current position; `GpiBox` and `GpiFullArc` add a
//! complete closed figure (whatever their `lControl`; the reference asks for
//! `DRO_OUTLINE`). Text and region painting inside a bracket draw nothing.
//!
//! - `GpiEndArea` closes every figure and draws them as one area
//!   (`DrawOp::Figures`): the inside by `BA_ALTERNATE` (even-odd) or
//!   `BA_WINDING` (nonzero) in the area pattern with the colours and mix of
//!   `GpiBeginArea` time, the boundary pels included (`BA_INCL`) or the
//!   bottom and right edges left out (`BA_EXCL`), the boundary lines drawn
//!   with the line attributes for `BA_BOUNDARY`. The current position moves
//!   to the start of the last figure when a closing line was needed.
//! - A path is defined once and used once: `GpiFillPath` (`FPATH_*`; open
//!   figures closed), `GpiStrokePath` (a line of the geometric width
//!   `GpiSetLineWidthGeom` along the figures, filled with the area
//!   attributes — square ends and joins, Warpine's choice), `GpiOutlinePath`
//!   (the cosmetic line attributes, open figures left open),
//!   `GpiSetClipPath` (`SCP_AND`: intersected with the current clip path;
//!   lPath 0 resets it), `GpiPathToRegion`; `GpiModifyPath(MPATH_STROKE)`
//!   turns it into the outline of that wide line, which `GpiFillPath`,
//!   `GpiSetClipPath` or `GpiPathToRegion` can then use. `GpiCloseFigure`
//!   closes the current figure of a path; the current position goes back to
//!   its start.
//! - `GpiPolygons` draws `POLYGON`s (the first starting at the current
//!   position) as one area; `POLYGON_NOFILL` leaves the inside alone.
//! - Curves (`pm_shapes.rs`): `GpiPolySpline` cubic Béziers,
//!   `GpiPolyFilletSharp` conics of the given sharpness, `GpiPolyFillet`
//!   parabolic fillets; the current position ends at the last point.
//!
//! Bounds: at most `MAX_SHAPE_POINTS` points in a bracket (more fails with
//! `PMERR_INV_LENGTH_OR_COUNT`), `MAX_POLYGONS` polygons per call, curve
//! counts by the points they read.

use std::sync::Arc;

use super::constants::*;
use super::mutex_ext::MutexExt;
use super::pm_gpi::PsTarget;
use super::Loader;
use crate::gui::region::{FillRule, Region, SetOp};
use crate::gui::surface::{AreaFill, MAX_BOUNDARY_WIDTH, MAX_SHAPE_POINTS};
use crate::gui::DrawOp;

/// Most polygons one `GpiPolygons` call draws.
pub const MAX_POLYGONS: u32 = 4096;
/// Most points a curve call (`GpiPolySpline`, …) reads.
const MAX_CURVE_INPUT: i32 = 8190;

/// A figure: its points and whether it is closed.
pub type Figure = (Vec<(i32, i32)>, bool);

/// The figures an area or path bracket is building, in device pels (each
/// primitive maps its points with the transforms in force when it is
/// called).
#[derive(Clone, Debug, Default)]
pub struct Figures {
    done: Vec<Figure>,
    current: Vec<(i32, i32)>,
    total: usize,
    /// The start of the current figure in world coordinates: where a
    /// closing line leaves the current position.
    start_world: (i32, i32),
}

impl Figures {
    fn room(&self, n: usize) -> bool {
        self.total + n <= MAX_SHAPE_POINTS
    }

    /// Extend the current figure from `from` (the current position, in
    /// device pels; `from_world` in world coordinates) through `pts`; false
    /// (nothing added) when the bracket would hold too many points.
    pub fn line_to(&mut self, from: (i32, i32), from_world: (i32, i32), pts: &[(i32, i32)]) -> bool {
        let start = usize::from(self.current.is_empty());
        if !self.room(start + pts.len()) { return false; }
        if self.current.is_empty() {
            self.current.push(from);
            self.start_world = from_world;
        }
        for &p in pts {
            if self.current.last() != Some(&p) { self.current.push(p); }
        }
        self.total += start + pts.len();
        true
    }

    /// End the current figure, open.
    pub fn move_to(&mut self) {
        if !self.current.is_empty() {
            self.done.push((std::mem::take(&mut self.current), false));
        }
    }

    /// Add a complete closed figure (box, full arc).
    pub fn closed_figure(&mut self, pts: Vec<(i32, i32)>) -> bool {
        if !self.room(pts.len()) { return false; }
        self.move_to();
        self.total += pts.len();
        self.done.push((pts, true));
        true
    }

    /// Close the current figure; its start point in world coordinates, if
    /// there was one.
    pub fn close(&mut self) -> Option<(i32, i32)> {
        self.current.first()?;
        self.done.push((std::mem::take(&mut self.current), true));
        Some(self.start_world)
    }

    /// The start of the current figure (world coordinates) when it does
    /// not end there.
    fn open_start(&self) -> Option<(i32, i32)> {
        let (first, last) = (self.current.first()?, self.current.last()?);
        (first != last).then_some(self.start_world)
    }

    pub fn finish(mut self) -> Vec<Figure> {
        self.move_to();
        self.done
    }
}

/// An open area bracket.
#[derive(Clone, Debug)]
pub struct AreaBracket {
    /// `BA_*` options.
    pub options: u32,
    pub figures: Figures,
    /// The area attributes at `GpiBeginArea`.
    pub fill: AreaFill,
}

/// A presentation space's path.
#[derive(Clone, Debug)]
pub enum PathState {
    /// Between `GpiBeginPath` and `GpiEndPath`.
    Open(Figures),
    /// Defined, not used yet.
    Defined(Vec<Figure>),
    /// `GpiModifyPath(MPATH_STROKE)`: the outline of a line this wide
    /// along the figures.
    Stroked(Vec<Figure>, u32),
}

/// What a primitive adds to a bracket's figures, in device pels.
pub(super) enum Part {
    /// Lines from the current position through these points.
    To(Vec<(i32, i32)>),
    /// A complete closed figure.
    Closed(Vec<(i32, i32)>),
    /// A new figure starts (`GpiMove`).
    Move,
}

fn all_closed(figs: &[Figure]) -> Vec<Figure> {
    figs.iter().map(|(p, _)| (p.clone(), true)).collect()
}

fn rule(winding: bool) -> FillRule {
    if winding { FillRule::Winding } else { FillRule::Alternate }
}

/// Width of a geometric line (`GpiSetLineWidthGeom`) in pels.
fn geom_width(w: i32) -> u32 {
    (w.max(1) as u32).min(MAX_BOUNDARY_WIDTH)
}

impl Loader {
    /// Whether `hps` is inside an area or path bracket (text and region
    /// painting then draw nothing).
    pub(super) fn in_bracket(&self, hps: u32) -> bool {
        self.shared.window_mgr.lock_or_recover().ps_map.get(&hps)
            .is_some_and(|ps| ps.area.is_some() || matches!(ps.path, Some(PathState::Open(_))))
    }

    /// If `hps` is building an area or a path, add `part` to its figures and
    /// return the call's result (`GPI_OK`, or `GPI_ERROR` with
    /// `PMERR_INV_LENGTH_OR_COUNT` when the bracket would hold too many
    /// points); `None` when the PS draws.
    pub(super) fn figure_part(&self, tid: u32, hps: u32, part: Part) -> Option<u32> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let ps = wm.ps_map.get_mut(&hps)?;
        let from_world = ps.current_pos;
        let from = ps.coords.world_to_device().pt(from_world);
        let figures = match (&mut ps.area, &mut ps.path) {
            (Some(a), _) => &mut a.figures,
            (None, Some(PathState::Open(f))) => f,
            _ => return None,
        };
        let ok = match part {
            Part::To(pts) => figures.line_to(from, from_world, &pts),
            Part::Closed(pts) => figures.closed_figure(pts),
            Part::Move => { figures.move_to(); true }
        };
        drop(wm);
        if ok { return Some(GPI_OK); }
        self.pm_error(tid, PMERR_INV_LENGTH_OR_COUNT);
        Some(GPI_ERROR)
    }

    /// The PMGPI area, path, polygon and curve calls by ordinal; `None` for
    /// other ordinals.
    pub(super) fn handle_area_call(&self, tid: u32, ordinal: u32, arg: &dyn Fn(u64) -> u32) -> Option<u32> {
        Some(match ordinal {
            352 => {
                // GpiBeginArea(HPS hps, ULONG flOptions) → BOOL
                self.gpi_begin_area(tid, arg(4), arg(8))
            }
            385 => {
                // GpiEndArea(HPS hps) → LONG
                self.gpi_end_area(tid, arg(4))
            }
            354 => {
                // GpiBeginPath(HPS hps, LONG lPath) → BOOL
                self.gpi_begin_path(tid, arg(4), arg(8))
            }
            387 => {
                // GpiEndPath(HPS hps) → BOOL
                self.gpi_end_path(tid, arg(4))
            }
            360 => {
                // GpiCloseFigure(HPS hps) → BOOL
                self.gpi_close_figure(tid, arg(4))
            }
            392 => {
                // GpiFillPath(HPS hps, LONG lPath, LONG lOptions) → LONG
                self.gpi_use_path(tid, arg(4), arg(8), arg(12), PathUse::Fill)
            }
            554 => {
                // GpiStrokePath(HPS hps, LONG lPath, ULONG flOptions) → LONG
                self.gpi_use_path(tid, arg(4), arg(8), arg(12), PathUse::Stroke)
            }
            563 => {
                // GpiOutlinePath(HPS hps, LONG lPath, LONG lOptions) → LONG
                self.gpi_use_path(tid, arg(4), arg(8), arg(12), PathUse::Outline)
            }
            403 => {
                // GpiModifyPath(HPS hps, LONG lPath, LONG lMode) → BOOL
                self.gpi_modify_path(tid, arg(4), arg(8), arg(12))
            }
            515 => {
                // GpiSetClipPath(HPS hps, LONG lPath, LONG lOptions) → BOOL
                self.gpi_set_clip_path(tid, arg(4), arg(8), arg(12))
            }
            559 => {
                // GpiPathToRegion(HPS hps, LONG lPath, LONG lOptions) → HRGN
                self.gpi_path_to_region(tid, arg(4), arg(8), arg(12))
            }
            650 => {
                // GpiPolygons(HPS hps, ULONG ulCount, PPOLYGON paplgn, ULONG flOptions, ULONG flModel) → LONG
                self.gpi_polygons(tid, arg(4), arg(8), arg(12), arg(16), arg(20))
            }
            413 => {
                // GpiPolyFillet(HPS hps, LONG lCount, PPOINTL aptlPoints) → LONG
                self.gpi_curve(tid, arg(4), arg(8) as i32, arg(12), Curve::Fillet)
            }
            414 => {
                // GpiPolyFilletSharp(HPS hps, LONG lCount, PPOINTL aptlPoints, PFIXED afxPoints) → LONG
                self.gpi_curve(tid, arg(4), arg(8) as i32, arg(12), Curve::Sharp(arg(16)))
            }
            417 => {
                // GpiPolySpline(HPS hps, LONG lCount, PPOINTL aptlPoints) → LONG
                self.gpi_curve(tid, arg(4), arg(8) as i32, arg(12), Curve::Spline)
            }
            _ => return None,
        })
    }

    fn gpi_begin_area(&self, tid: u32, hps: u32, options: u32) -> u32 {
        let Some(t) = self.ps_target(hps) else { return self.gpi_fail(tid, PMERR_INV_HPS) };
        if options & !(BA_BOUNDARY | BA_WINDING | BA_EXCL) != 0 { return self.gpi_fail(tid, PMERR_INV_AREA_CONTROL); }
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let ps = wm.ps_map.get_mut(&hps).expect("ps_target found it");
        let err = if ps.area.is_some() {
            PMERR_ALREADY_IN_AREA
        } else if matches!(ps.path, Some(PathState::Open(_))) {
            PMERR_INV_IN_PATH
        } else {
            ps.area = Some(AreaBracket { options, figures: Figures::default(), fill: t.fill() });
            return 1;
        };
        drop(wm);
        self.gpi_fail(tid, err)
    }

    fn gpi_end_area(&self, tid: u32, hps: u32) -> u32 {
        let Some(t) = self.ps_target(hps) else { return self.gpi_fail(tid, PMERR_INV_HPS) };
        let area = {
            let mut wm = self.shared.window_mgr.lock_or_recover();
            let ps = wm.ps_map.get_mut(&hps).expect("ps_target found it");
            let Some(area) = ps.area.take() else {
                drop(wm);
                return self.gpi_fail(tid, PMERR_NOT_IN_AREA);
            };
            // A closing line ends on the start of the last figure.
            if let Some(start) = area.figures.open_start() { ps.current_pos = start; }
            area
        };
        let o = area.options;
        let figures = all_closed(&area.figures.finish());
        if !figures.is_empty() {
            self.draw_ps(&t, DrawOp::Figures {
                figures,
                rule: Some(rule(o & BA_WINDING != 0)),
                boundary: u32::from(o & BA_EXCL == 0),
                fill: Some(area.fill),
                stroke: if o & BA_BOUNDARY != 0 { t.stroke() } else { None },
            });
        }
        GPI_OK
    }

    fn gpi_begin_path(&self, tid: u32, hps: u32, id: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let err = match wm.ps_map.get_mut(&hps) {
            None => PMERR_INV_HPS,
            Some(_) if id != 1 => PMERR_INV_PATH_ID,
            Some(ps) if ps.area.is_some() => PMERR_INV_IN_AREA,
            Some(ps) if matches!(ps.path, Some(PathState::Open(_))) => PMERR_ALREADY_IN_PATH,
            Some(ps) => {
                ps.path = Some(PathState::Open(Figures::default()));
                return 1;
            }
        };
        drop(wm);
        self.gpi_fail(tid, err)
    }

    fn gpi_end_path(&self, tid: u32, hps: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let err = match wm.ps_map.get_mut(&hps) {
            None => PMERR_INV_HPS,
            Some(ps) => match ps.path.take() {
                Some(PathState::Open(f)) => {
                    ps.path = Some(PathState::Defined(f.finish()));
                    return 1;
                }
                other => { ps.path = other; PMERR_NOT_IN_PATH }
            },
        };
        drop(wm);
        self.gpi_fail(tid, err)
    }

    /// GpiCloseFigure: only in a path (outside one `PMERR_NOT_IN_PATH`, in
    /// an area `PMERR_INV_IN_AREA` — the reference forbids both without
    /// naming the error, these are Warpine's choice).
    fn gpi_close_figure(&self, tid: u32, hps: u32) -> u32 {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let err = match wm.ps_map.get_mut(&hps) {
            None => PMERR_INV_HPS,
            Some(ps) if ps.area.is_some() => PMERR_INV_IN_AREA,
            Some(ps) => match &mut ps.path {
                Some(PathState::Open(f)) => {
                    if let Some(start) = f.close() { ps.current_pos = start; }
                    return 1;
                }
                _ => PMERR_NOT_IN_PATH,
            },
        };
        drop(wm);
        self.gpi_fail(tid, err)
    }

    /// Record `code` and return `GPI_ERROR` (also FALSE, `RGN_ERROR`).
    fn gpi_fail(&self, tid: u32, code: u32) -> u32 {
        self.pm_error(tid, code);
        GPI_ERROR
    }

    /// Take the defined path of `hps` (lPath must be 1): `Err` with the
    /// error code when it cannot be used.
    fn take_path(&self, hps: u32, id: u32) -> Result<PathState, u32> {
        let mut wm = self.shared.window_mgr.lock_or_recover();
        let Some(ps) = wm.ps_map.get_mut(&hps) else { return Err(PMERR_INV_HPS) };
        if id != 1 { return Err(PMERR_INV_PATH_ID); }
        match ps.path.take() {
            Some(p @ (PathState::Defined(_) | PathState::Stroked(..))) => Ok(p),
            other => { ps.path = other; Err(PMERR_PATH_UNKNOWN) }
        }
    }

    fn gpi_use_path(&self, tid: u32, hps: u32, id: u32, options: u32, how: PathUse) -> u32 {
        let Some(t) = self.ps_target(hps) else { return self.gpi_fail(tid, PMERR_INV_HPS) };
        let valid = match how {
            PathUse::Fill => options & !(FPATH_WINDING | FPATH_EXCL) == 0,
            PathUse::Stroke | PathUse::Outline => options == 0,
        };
        if !valid {
            let code = if how == PathUse::Fill { PMERR_INV_FILL_PATH_OPTIONS } else { PMERR_INV_RESERVED_FIELD };
            return self.gpi_fail(tid, code);
        }
        let path = match self.take_path(hps, id) {
            Ok(p) => p,
            Err(e) => return self.gpi_fail(tid, e),
        };
        let op = match (how, path) {
            (PathUse::Fill, PathState::Defined(f)) => DrawOp::Figures {
                figures: all_closed(&f),
                rule: Some(rule(options & FPATH_WINDING != 0)),
                boundary: u32::from(options & FPATH_EXCL == 0),
                fill: Some(t.fill()), stroke: None,
            },
            (PathUse::Fill, PathState::Stroked(f, w)) => DrawOp::Figures {
                figures: f, rule: None, boundary: w, fill: Some(t.fill()), stroke: None,
            },
            (PathUse::Stroke, PathState::Defined(f)) => DrawOp::Figures {
                figures: f, rule: None, boundary: geom_width(t.line_width_geom), fill: Some(t.fill()), stroke: None,
            },
            (PathUse::Outline, PathState::Defined(f)) => DrawOp::Figures {
                figures: f, rule: None, boundary: 0, fill: None, stroke: t.stroke(),
            },
            // A modified path can only be filled or made a clip path or region.
            (_, PathState::Stroked(..)) => return self.gpi_fail(tid, PMERR_PATH_UNKNOWN),
            (_, PathState::Open(_)) => unreachable!("take_path returns defined paths"),
        };
        self.draw_ps(&t, op);
        GPI_OK
    }

    fn gpi_modify_path(&self, tid: u32, hps: u32, id: u32, mode: u32) -> u32 {
        if !self.check_hps(tid, hps) { return 0; }
        if mode != MPATH_STROKE { return self.gpi_fail(tid, PMERR_INV_MODIFY_PATH_MODE); }
        let width = {
            let wm = self.shared.window_mgr.lock_or_recover();
            geom_width(wm.ps_map[&hps].line_width_geom)
        };
        match self.take_path(hps, id) {
            Ok(PathState::Defined(f)) => {
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.path = Some(PathState::Stroked(f, width)); }
                1
            }
            Ok(other) => {
                // Already modified: put it back, it cannot be modified again.
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.path = Some(other); }
                drop(wm);
                self.gpi_fail(tid, PMERR_PATH_UNKNOWN)
            }
            Err(e) => self.gpi_fail(tid, e),
        }
    }

    /// The pels of a path as a region: open figures closed, the inside by
    /// `winding`, the boundary in unless `excl`; a modified path is the
    /// outline of its wide line.
    fn path_region(path: PathState, winding: bool, excl: bool) -> Option<Region> {
        match path {
            PathState::Defined(f) => {
                let figs: Vec<(&[(i32, i32)], bool)> = f.iter().map(|(p, _)| (p.as_slice(), true)).collect();
                Region::from_figures(&figs, Some(rule(winding)), u32::from(!excl))
            }
            PathState::Stroked(f, w) => {
                let figs: Vec<(&[(i32, i32)], bool)> = f.iter().map(|(p, c)| (p.as_slice(), *c)).collect();
                Region::from_figures(&figs, None, w)
            }
            PathState::Open(_) => None,
        }
    }

    /// GpiSetClipPath: lPath 0 resets the clip path (no `SCP_AND`); lPath 1
    /// (with `SCP_AND`) intersects the current clip path with the path.
    fn gpi_set_clip_path(&self, tid: u32, hps: u32, id: u32, options: u32) -> u32 {
        if !self.check_hps(tid, hps) { return 0; }
        if options & !(SCP_WINDING | SCP_AND | SCP_EXCL) != 0 {
            return self.gpi_fail(tid, PMERR_INV_CLIP_PATH_OPTIONS);
        }
        let clip = match id {
            0 => {
                if options & SCP_AND != 0 { return self.gpi_fail(tid, PMERR_INV_CLIP_PATH_OPTIONS); }
                None
            }
            1 => {
                if options & SCP_AND == 0 { return self.gpi_fail(tid, PMERR_INV_CLIP_PATH_OPTIONS); }
                let path = match self.take_path(hps, 1) {
                    Ok(p) => p,
                    Err(e) => return self.gpi_fail(tid, e),
                };
                let Some(region) = Self::path_region(path, options & SCP_WINDING != 0, options & SCP_EXCL != 0)
                    else { return self.gpi_fail(tid, PMERR_INSUFFICIENT_MEMORY) };
                let current = self.shared.window_mgr.lock_or_recover().ps_map[&hps].clip_path.clone();
                let region = match current {
                    Some(c) => match Region::combine(&c, &region, SetOp::And) {
                        Some(r) => r,
                        None => return self.gpi_fail(tid, PMERR_INSUFFICIENT_MEMORY),
                    },
                    None => region,
                };
                Some(Arc::new(region))
            }
            _ => return self.gpi_fail(tid, PMERR_INV_PATH_ID),
        };
        let mut wm = self.shared.window_mgr.lock_or_recover();
        if let Some(ps) = wm.ps_map.get_mut(&hps) { ps.clip_path = clip; }
        wm.refresh_clip(hps);
        1
    }

    fn gpi_path_to_region(&self, tid: u32, hps: u32, id: u32, options: u32) -> u32 {
        if !self.check_hps(tid, hps) { return RGN_ERROR; }
        if options & !(FPATH_WINDING | FPATH_EXCL) != 0 { return self.gpi_fail(tid, PMERR_INV_FILL_PATH_OPTIONS); }
        let path = match self.take_path(hps, id) {
            Ok(p) => p,
            Err(e) => return self.gpi_fail(tid, e),
        };
        let Some(region) = Self::path_region(path, options & FPATH_WINDING != 0, options & FPATH_EXCL != 0)
            else { return self.gpi_fail(tid, PMERR_INSUFFICIENT_MEMORY) };
        match self.shared.window_mgr.lock_or_recover().regions.create(region) {
            Some(h) => h,
            None => { self.pm_error(tid, PMERR_INSUFFICIENT_MEMORY); RGN_ERROR }
        }
    }

    /// GpiPolygons: `count` POLYGONs at `ptr` as one area; the first starts
    /// at the current position. The current position is left alone (the
    /// reference does not say; unverified).
    fn gpi_polygons(&self, tid: u32, hps: u32, count: u32, ptr: u32, options: u32, model: u32) -> u32 {
        let Some(t) = self.ps_target(hps) else { return self.gpi_fail(tid, PMERR_INV_HPS) };
        if options & !(POLYGON_BOUNDARY | POLYGON_WINDING) != 0 || model & !(POLYGON_EXCL | POLYGON_NOFILL) != 0 {
            return self.gpi_fail(tid, PMERR_INV_AREA_CONTROL);
        }
        {
            let wm = self.shared.window_mgr.lock_or_recover();
            let ps = &wm.ps_map[&hps];
            let err = if ps.area.is_some() { Some(PMERR_ALREADY_IN_AREA) }
                else if matches!(ps.path, Some(PathState::Open(_))) { Some(PMERR_INV_IN_PATH) }
                else { None };
            drop(wm);
            if let Some(e) = err { return self.gpi_fail(tid, e); }
        }
        if count > MAX_POLYGONS { return self.gpi_fail(tid, PMERR_INV_LENGTH_OR_COUNT); }
        let mut figures: Vec<(Vec<(i32, i32)>, bool)> = Vec::new();
        let mut total = 0usize;
        for i in 0..count {
            let at = ptr.wrapping_add(i * POLYGON_SIZE);
            let (Some(n), Some(points)) = (self.guest_read::<u32>(at), self.guest_read::<u32>(at.wrapping_add(4)))
                else { return self.gpi_fail(tid, PMERR_INV_LENGTH_OR_COUNT) };
            total += n as usize + usize::from(i == 0);
            if total > MAX_SHAPE_POINTS { return self.gpi_fail(tid, PMERR_INV_LENGTH_OR_COUNT); }
            let Some(mut pts) = self.read_pointls(n, points) else { return self.gpi_fail(tid, PMERR_INV_LENGTH_OR_COUNT) };
            if i == 0 { pts.insert(0, t.pos); }
            figures.push((t.xform.pts(&pts), true));
        }
        let fill = (model & POLYGON_NOFILL == 0).then(|| t.fill());
        let stroke = if options & POLYGON_BOUNDARY != 0 { t.stroke() } else { None };
        if !figures.is_empty() && (fill.is_some() || stroke.is_some()) {
            self.draw_ps(&t, DrawOp::Figures {
                figures,
                rule: Some(rule(options & POLYGON_WINDING != 0)),
                boundary: u32::from(model & POLYGON_EXCL == 0),
                fill, stroke,
            });
        }
        GPI_OK
    }

    /// `n` POINTLs at `ptr`.
    fn read_pointls(&self, n: u32, ptr: u32) -> Option<Vec<(i32, i32)>> {
        (0..n).map(|i| self.read_pointl(ptr.wrapping_add(i * 8))).collect()
    }

    /// GpiPolyFillet / GpiPolyFilletSharp / GpiPolySpline: the curve from
    /// the current position, drawn with the line attributes (or added to
    /// the bracket's figure); the current position ends at the last point.
    fn gpi_curve(&self, tid: u32, hps: u32, count: i32, ptr: u32, curve: Curve) -> u32 {
        if !self.check_hps(tid, hps) { return GPI_ERROR; }
        let multiple = match curve { Curve::Fillet => 1, Curve::Sharp(_) => 2, Curve::Spline => 3 };
        if !(0..=MAX_CURVE_INPUT).contains(&count) || count % multiple != 0 {
            return self.gpi_fail(tid, PMERR_INV_LENGTH_OR_COUNT);
        }
        if count == 0 { return GPI_OK; }
        let Some(world_pts) = self.read_pointls(count as u32, ptr) else { return self.gpi_fail(tid, PMERR_INV_COORDINATE) };
        // Curves are affine invariant: flatten them in device pels from the
        // mapped control points.
        let (xform, from) = {
            let wm = self.shared.window_mgr.lock_or_recover();
            let ps = &wm.ps_map[&hps];
            (ps.coords.world_to_device(), ps.current_pos)
        };
        let (pts, from) = (xform.pts(&world_pts), xform.pt(from));
        let path: Vec<(i32, i32)> = match curve {
            Curve::Fillet => super::pm_shapes::poly_fillet(from, &pts),
            Curve::Spline => {
                let mut out = Vec::new();
                let mut start = from;
                for c in pts.as_chunks::<3>().0 {
                    out.extend(super::pm_shapes::cubic(start, c[0], c[1], c[2]));
                    start = c[2];
                }
                out
            }
            Curve::Sharp(pfx) => {
                let mut out = Vec::new();
                let mut start = from;
                for (k, c) in pts.as_chunks::<2>().0.iter().enumerate() {
                    let Some(fx) = self.guest_read::<u32>(pfx.wrapping_add(4 * k as u32))
                        else { return self.gpi_fail(tid, PMERR_INV_SHARPNESS_PARM) };
                    let w = super::pm_shapes::fixed(fx);
                    if w < 0.0 { return self.gpi_fail(tid, PMERR_INV_SHARPNESS_PARM); }
                    out.extend(super::pm_shapes::conic(start, c[0], c[1], w));
                    start = c[1];
                }
                out
            }
        };
        let last = world_pts[world_pts.len() - 1];
        let result = match self.figure_part(tid, hps, Part::To(path.clone())) {
            Some(r) => r,
            None => {
                let Some(t) = self.ps_target(hps) else { return GPI_ERROR };
                let mut line = Vec::with_capacity(path.len() + 1);
                line.push(t.dev(t.pos));
                line.extend(path);
                self.draw_path(&t, line);
                GPI_OK
            }
        };
        if result == GPI_OK { self.set_current_pos(hps, last); }
        result
    }

    /// Draw `op` through presentation space target `t`, clipped to its clip
    /// region and clip path.
    pub(super) fn draw_ps(&self, t: &PsTarget, op: DrawOp) {
        self.draw_clipped(t.target, op, t.clip.as_deref());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathUse {
    Fill,
    Stroke,
    Outline,
}

#[derive(Clone, Copy)]
enum Curve {
    Fillet,
    /// With the address of the FIXED sharpness values.
    Sharp(u32),
    Spline,
}

#[cfg(test)]
mod tests {
    use super::super::constants::*;
    use super::super::pm_region::testing::*;

    // PMGPI ordinals (doc/os2_ordinals.md).
    const BEGIN_AREA: u32 = 352;
    const END_AREA: u32 = 385;
    const BEGIN_PATH: u32 = 354;
    const END_PATH: u32 = 387;
    const CLOSE_FIGURE: u32 = 360;
    const FILL_PATH: u32 = 392;
    const STROKE_PATH: u32 = 554;
    const OUTLINE_PATH: u32 = 563;
    const MODIFY_PATH: u32 = 403;
    const SET_CLIP_PATH: u32 = 515;
    const PATH_TO_REGION: u32 = 559;
    const POLYGONS: u32 = 650;
    const POLY_FILLET: u32 = 413;
    const POLY_FILLET_SHARP: u32 = 414;
    const POLY_SPLINE: u32 = 417;
    const POLY_LINE: u32 = 415;
    const LINE: u32 = 398;
    const SET_COLOR: u32 = 517;
    const SET_LINE_WIDTH_GEOM: u32 = 532;
    const QUERY_POS: u32 = 441;
    const GPI_BOX: u32 = 356;
    const REGION_BOX: u32 = 481;
    const PTS: u32 = 0x7000;
    const POLY: u32 = 0x7800;
    const OUT: u32 = 0x7C00;

    fn poly_line(l: &super::super::Loader, hps: u32, pts: &[(i32, i32)]) -> u32 {
        put_points(l, PTS, pts);
        gpi(l, POLY_LINE, &[hps, pts.len() as u32, PTS])
    }

    fn pos(l: &super::super::Loader, hps: u32) -> (i32, i32) {
        gpi(l, QUERY_POS, &[hps, OUT]);
        (l.guest_read::<i32>(OUT).unwrap(), l.guest_read::<i32>(OUT + 4).unwrap())
    }

    /// Two overlapping squares, both counter-clockwise: (10,10)-(40,40) and
    /// (25,25)-(55,55).
    fn two_squares(l: &super::super::Loader, hps: u32) {
        move_to(l, hps, (10, 10));
        poly_line(l, hps, &[(40, 10), (40, 40), (10, 40)]);
        move_to(l, hps, (25, 25));
        poly_line(l, hps, &[(55, 25), (55, 55), (25, 55)]);
    }

    #[test]
    fn areas_fill_by_the_alternate_or_winding_rule() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, BA_NOBOUNDARY | BA_ALTERNATE]), 1);
        two_squares(&l, hps);
        assert_eq!(pel(&l, hps, 20, 20), CLR_BACKGROUND, "nothing drawn inside the bracket");
        assert_eq!(gpi(&l, END_AREA, &[hps]), GPI_OK);
        assert_eq!(pel(&l, hps, 20, 20), CLR_RED);
        assert_eq!(pel(&l, hps, 30, 30), CLR_BACKGROUND, "the overlap is outside (even-odd)");
        assert_eq!(pel(&l, hps, 50, 50), CLR_RED);
        assert_eq!(pos(&l, hps), (25, 25), "the closing line ends at the last figure's start");

        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        gpi(&l, BEGIN_AREA, &[hps, BA_WINDING]);
        two_squares(&l, hps);
        gpi(&l, END_AREA, &[hps]);
        assert_eq!(pel(&l, hps, 30, 30), CLR_RED, "the overlap is inside (winding)");
    }

    #[test]
    fn area_boundary_and_exclusive_edges() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        // BA_EXCL: no bottom and right edges, left and top in.
        gpi(&l, BEGIN_AREA, &[hps, BA_EXCL]);
        move_to(&l, hps, (10, 10));
        poly_line(&l, hps, &[(20, 10), (20, 20), (10, 20)]);
        gpi(&l, END_AREA, &[hps]);
        assert_eq!(pel(&l, hps, 10, 15), CLR_RED);
        assert_eq!(pel(&l, hps, 15, 20), CLR_RED);
        assert_eq!(pel(&l, hps, 15, 10), CLR_BACKGROUND, "bottom edge out");
        assert_eq!(pel(&l, hps, 20, 15), CLR_BACKGROUND, "right edge out");
        // BA_INCL with a boundary in another colour: the edges are the
        // line's, the inside the area's.
        gpi(&l, BEGIN_AREA, &[hps, BA_BOUNDARY]);
        move_to(&l, hps, (50, 10));
        poly_line(&l, hps, &[(60, 10), (60, 20), (50, 20)]);
        gpi(&l, SET_COLOR, &[hps, CLR_BLUE]); // the line colour at GpiEndArea
        gpi(&l, END_AREA, &[hps]);
        assert_eq!(pel(&l, hps, 55, 15), CLR_RED, "pattern colour of GpiBeginArea time");
        assert_eq!(pel(&l, hps, 60, 15), CLR_BLUE);
        assert_eq!(pel(&l, hps, 55, 10), CLR_BLUE);
    }

    #[test]
    fn area_bracket_errors_and_box_figures() {
        let (l, _, hps) = setup();
        assert_eq!(gpi(&l, END_AREA, &[hps]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_NOT_IN_AREA);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0x100]), 0);
        assert_eq!(last_error(&l), PMERR_INV_AREA_CONTROL);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 1);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 0);
        assert_eq!(last_error(&l), PMERR_ALREADY_IN_AREA);
        assert_eq!(gpi(&l, BEGIN_PATH, &[hps, 1]), 0);
        assert_eq!(last_error(&l), PMERR_INV_IN_AREA);
        assert_eq!(gpi(&l, CLOSE_FIGURE, &[hps]), 0);
        assert_eq!(last_error(&l), PMERR_INV_IN_AREA);
        // A box inside the area is a closed figure of it.
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        move_to(&l, hps, (100, 50));
        put_points(&l, PTS, &[(120, 70)]);
        assert_eq!(gpi(&l, GPI_BOX, &[hps, DRO_OUTLINE, PTS, 0, 0]), GPI_OK);
        assert_eq!(pel(&l, hps, 110, 60), CLR_BACKGROUND);
        gpi(&l, END_AREA, &[hps]);
        assert_eq!(pel(&l, hps, 110, 60), CLR_BLACK as u32, "the default colour of GpiBeginArea time");
        // Too many points in a bracket fail.
        gpi(&l, BEGIN_AREA, &[hps, 0]);
        let many: Vec<(i32, i32)> = (0..4000).map(|i| (i % 50, i / 50)).collect();
        assert_eq!(poly_line(&l, hps, &many), GPI_OK);
        assert_eq!(poly_line(&l, hps, &many), GPI_OK);
        assert_eq!(poly_line(&l, hps, &many), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        gpi(&l, END_AREA, &[hps]);
    }

    /// A path of one triangle (20,10)-(60,10)-(40,50), closed.
    fn triangle_path(l: &super::super::Loader, hps: u32) {
        assert_eq!(gpi(l, BEGIN_PATH, &[hps, 1]), 1);
        move_to(l, hps, (20, 10));
        poly_line(l, hps, &[(60, 10), (40, 50)]);
        assert_eq!(gpi(l, CLOSE_FIGURE, &[hps]), 1);
        assert_eq!(gpi(l, END_PATH, &[hps]), 1);
    }

    #[test]
    fn paths_fill_stroke_and_outline_once() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        triangle_path(&l, hps);
        assert_eq!(pos(&l, hps), (20, 10), "GpiCloseFigure returns to the start");
        assert_eq!(pel(&l, hps, 40, 20), CLR_BACKGROUND, "defining draws nothing");
        assert_eq!(gpi(&l, FILL_PATH, &[hps, 1, FPATH_ALTERNATE]), GPI_OK);
        assert_eq!(pel(&l, hps, 40, 20), CLR_RED);
        assert_eq!(pel(&l, hps, 20, 30), CLR_BACKGROUND);
        // The path is used up.
        assert_eq!(gpi(&l, FILL_PATH, &[hps, 1, 0]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_PATH_UNKNOWN);

        // Outline: the cosmetic line only.
        gpi(&l, SET_COLOR, &[hps, CLR_BLUE]);
        triangle_path(&l, hps);
        assert_eq!(gpi(&l, OUTLINE_PATH, &[hps, 1, 0]), GPI_OK);
        assert_eq!(pel(&l, hps, 40, 10), CLR_BLUE);
        assert_eq!(pel(&l, hps, 40, 20), CLR_RED, "inside untouched");

        // Stroke: a geometric-width line filled with the area attributes.
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        gpi(&l, SET_LINE_WIDTH_GEOM, &[hps, 6]);
        triangle_path(&l, hps);
        assert_eq!(gpi(&l, STROKE_PATH, &[hps, 1, 0]), GPI_OK);
        assert_eq!(pel(&l, hps, 40, 8), CLR_GREEN, "the wide line reaches below the edge");
        assert_eq!(pel(&l, hps, 40, 12), CLR_GREEN);
        assert_eq!(pel(&l, hps, 40, 25), CLR_BACKGROUND, "the inside is not filled");
        // Errors.
        assert_eq!(gpi(&l, STROKE_PATH, &[hps, 1, 1]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_RESERVED_FIELD);
        assert_eq!(gpi(&l, FILL_PATH, &[hps, 2, 0]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_PATH_ID);
        assert_eq!(gpi(&l, FILL_PATH, &[hps, 1, 0x40]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_FILL_PATH_OPTIONS);
        assert_eq!(gpi(&l, BEGIN_PATH, &[hps, 2]), 0);
        assert_eq!(last_error(&l), PMERR_INV_PATH_ID);
        assert_eq!(gpi(&l, END_PATH, &[hps]), 0);
        assert_eq!(last_error(&l), PMERR_NOT_IN_PATH);
        assert_eq!(gpi(&l, CLOSE_FIGURE, &[hps]), 0);
        assert_eq!(last_error(&l), PMERR_NOT_IN_PATH);
        gpi(&l, BEGIN_PATH, &[hps, 1]);
        assert_eq!(gpi(&l, BEGIN_PATH, &[hps, 1]), 0);
        assert_eq!(last_error(&l), PMERR_ALREADY_IN_PATH);
        assert_eq!(gpi(&l, BEGIN_AREA, &[hps, 0]), 0);
        assert_eq!(last_error(&l), PMERR_INV_IN_PATH);
        assert_eq!(gpi(&l, FILL_PATH, &[hps, 1, 0]), GPI_ERROR, "still open");
        assert_eq!(last_error(&l), PMERR_PATH_UNKNOWN);
    }

    #[test]
    fn modified_paths_and_clip_paths() {
        let (l, _, hps) = setup();
        gpi(&l, SET_LINE_WIDTH_GEOM, &[hps, 4]);
        triangle_path(&l, hps);
        assert_eq!(gpi(&l, MODIFY_PATH, &[hps, 1, 5]), 0);
        assert_eq!(last_error(&l), PMERR_INV_MODIFY_PATH_MODE);
        assert_eq!(gpi(&l, MODIFY_PATH, &[hps, 1, MPATH_STROKE]), 1);
        assert_eq!(gpi(&l, MODIFY_PATH, &[hps, 1, MPATH_STROKE]), 0, "only once");
        // A modified path as a clip path: only the wide line's pels draw.
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 1, SCP_WINDING | SCP_AND]), 1);
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (0, 0));
        put_points(&l, PTS, &[(199, 99)]);
        gpi(&l, GPI_BOX, &[hps, DRO_FILL, PTS, 0, 0]);
        assert_eq!(pel(&l, hps, 40, 10), CLR_RED);
        assert_eq!(pel(&l, hps, 40, 25), CLR_BACKGROUND);
        assert_eq!(pel(&l, hps, 100, 50), CLR_BACKGROUND);
        // Reset: everything draws again.
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 0, SCP_AND]), 0, "lPath 0 with SCP_AND");
        assert_eq!(last_error(&l), PMERR_INV_CLIP_PATH_OPTIONS);
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 0, SCP_RESET]), 1);
        gpi(&l, SET_COLOR, &[hps, CLR_BLUE]);
        gpi(&l, GPI_BOX, &[hps, DRO_FILL, PTS, 0, 0]);
        assert_eq!(pel(&l, hps, 100, 50), CLR_BLUE);
        // A filled clip path (the triangle), intersected with a second one.
        triangle_path(&l, hps);
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 1, 0]), 0, "lPath 1 needs SCP_AND");
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 1, SCP_AND]), 1);
        gpi(&l, BEGIN_PATH, &[hps, 1]);
        move_to(&l, hps, (0, 0));
        poly_line(&l, hps, &[(39, 0), (39, 99), (0, 99)]);
        gpi(&l, END_PATH, &[hps]);
        assert_eq!(gpi(&l, SET_CLIP_PATH, &[hps, 1, SCP_AND]), 1);
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        gpi(&l, GPI_BOX, &[hps, DRO_FILL, PTS, 0, 0]);
        assert_eq!(pel(&l, hps, 35, 15), CLR_GREEN, "in both");
        assert_eq!(pel(&l, hps, 45, 15), CLR_BLUE, "in the triangle only");
        assert_eq!(pel(&l, hps, 10, 50), CLR_BLUE, "in the half only");
    }

    #[test]
    fn path_to_region() {
        let (l, _, hps) = setup();
        triangle_path(&l, hps);
        let r = gpi(&l, PATH_TO_REGION, &[hps, 1, FPATH_ALTERNATE]);
        assert_ne!(r, RGN_ERROR);
        assert_eq!(gpi(&l, REGION_BOX, &[hps, r, OUT]), RGN_COMPLEX);
        assert_eq!(get_rect(&l, OUT), (20, 10, 61, 51), "the boundary is in");
        assert_eq!(gpi(&l, PATH_TO_REGION, &[hps, 1, 0]), RGN_ERROR, "used up");
        assert_eq!(last_error(&l), PMERR_PATH_UNKNOWN);
    }

    #[test]
    fn polygons() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (10, 10));
        // First polygon from the current position; the second lists all.
        put_points(&l, PTS, &[(40, 10), (40, 40), (10, 40)]);
        put_points(&l, PTS + 0x100, &[(25, 25), (55, 25), (55, 55), (25, 55)]);
        for (i, (n, at)) in [(3u32, PTS), (4, PTS + 0x100)].into_iter().enumerate() {
            l.guest_write::<u32>(POLY + 8 * i as u32, n).unwrap();
            l.guest_write::<u32>(POLY + 8 * i as u32 + 4, at).unwrap();
        }
        assert_eq!(gpi(&l, POLYGONS, &[hps, 2, POLY, POLYGON_NOBOUNDARY, POLYGON_INCL]), GPI_OK);
        assert_eq!(pel(&l, hps, 20, 20), CLR_RED);
        assert_eq!(pel(&l, hps, 30, 30), CLR_BACKGROUND, "alternate");
        assert_eq!(pel(&l, hps, 50, 50), CLR_RED);
        assert_eq!(pos(&l, hps), (10, 10));
        gpi(&l, SET_COLOR, &[hps, CLR_BLUE]);
        assert_eq!(gpi(&l, POLYGONS, &[hps, 2, POLY, POLYGON_WINDING | POLYGON_BOUNDARY, POLYGON_NOFILL]), GPI_OK);
        assert_eq!(pel(&l, hps, 30, 30), CLR_BACKGROUND, "no fill");
        assert_eq!(pel(&l, hps, 40, 20), CLR_BLUE, "boundary");
        gpi(&l, SET_COLOR, &[hps, CLR_GREEN]);
        assert_eq!(gpi(&l, POLYGONS, &[hps, 2, POLY, POLYGON_WINDING, 0]), GPI_OK);
        assert_eq!(pel(&l, hps, 30, 30), CLR_GREEN, "winding");
        // Errors and bounds.
        assert_eq!(gpi(&l, POLYGONS, &[hps, 0, POLY, 0, 0]), GPI_OK, "zero draws nothing");
        assert_eq!(gpi(&l, POLYGONS, &[hps, 1, POLY, 0x80, 0]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_AREA_CONTROL);
        assert_eq!(gpi(&l, POLYGONS, &[hps, 1_000_000, POLY, 0, 0]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        l.guest_write::<u32>(POLY, 1_000_000).unwrap();
        assert_eq!(gpi(&l, POLYGONS, &[hps, 1, POLY, 0, 0]), GPI_ERROR, "too many points");
        gpi(&l, BEGIN_AREA, &[hps, 0]);
        assert_eq!(gpi(&l, POLYGONS, &[hps, 0, POLY, 0, 0]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_ALREADY_IN_AREA);
    }

    #[test]
    fn curves_draw_and_move_the_position() {
        let (l, _, hps) = setup();
        gpi(&l, SET_COLOR, &[hps, CLR_RED]);
        move_to(&l, hps, (10, 10));
        put_points(&l, PTS, &[(10, 60), (60, 60), (60, 10)]);
        assert_eq!(gpi(&l, POLY_SPLINE, &[hps, 3, PTS]), GPI_OK);
        assert_eq!(pos(&l, hps), (60, 10));
        assert_eq!(pel(&l, hps, 60, 10), CLR_RED);
        // The cubic's midpoint is (35, 47.5).
        assert!([47, 48].iter().any(|&y| pel(&l, hps, 35, y) == CLR_RED));
        assert_eq!(gpi(&l, POLY_SPLINE, &[hps, 2, PTS]), GPI_ERROR, "not a multiple of 3");
        assert_eq!(last_error(&l), PMERR_INV_LENGTH_OR_COUNT);
        // A fillet between two lines.
        move_to(&l, hps, (100, 10));
        put_points(&l, PTS, &[(100, 60), (150, 60)]);
        assert_eq!(gpi(&l, POLY_FILLET, &[hps, 2, PTS]), GPI_OK);
        assert_eq!(pos(&l, hps), (150, 60));
        assert_eq!(pel(&l, hps, 100, 60), CLR_BACKGROUND, "the corner is cut");
        // Sharp fillets: sharpness read per fillet, negative refused.
        move_to(&l, hps, (10, 80));
        put_points(&l, PTS, &[(30, 99), (50, 80)]);
        l.guest_write::<u32>(OUT, 0x0002_0000).unwrap();
        assert_eq!(gpi(&l, POLY_FILLET_SHARP, &[hps, 2, PTS, OUT]), GPI_OK);
        assert_eq!(pos(&l, hps), (50, 80));
        l.guest_write::<i32>(OUT, -0x10000).unwrap();
        assert_eq!(gpi(&l, POLY_FILLET_SHARP, &[hps, 2, PTS, OUT]), GPI_ERROR);
        assert_eq!(last_error(&l), PMERR_INV_SHARPNESS_PARM);
        assert_eq!(gpi(&l, POLY_FILLET, &[hps, (-1i32) as u32, PTS]), GPI_ERROR);
        assert_eq!(gpi(&l, POLY_FILLET, &[hps, 0, PTS]), GPI_OK, "zero points: nothing");
        // Inside a path the curve becomes part of the figure.
        gpi(&l, BEGIN_PATH, &[hps, 1]);
        move_to(&l, hps, (120, 80));
        put_points(&l, PTS, &[(120, 99), (180, 99), (180, 80)]);
        gpi(&l, POLY_SPLINE, &[hps, 3, PTS]);
        assert_eq!(pel(&l, hps, 150, 94), CLR_BACKGROUND);
        gpi(&l, END_PATH, &[hps]);
        gpi(&l, FILL_PATH, &[hps, 1, 0]);
        assert_eq!(pel(&l, hps, 150, 88), CLR_RED);
        // A line and text inside a path draw nothing.
        gpi(&l, BEGIN_PATH, &[hps, 1]);
        put_points(&l, PTS, &[(5, 95)]);
        move_to(&l, hps, (5, 5));
        assert_eq!(gpi(&l, LINE, &[hps, PTS]), GPI_OK);
        assert_eq!(pel(&l, hps, 5, 50), CLR_BACKGROUND);
        assert_eq!(pos(&l, hps), (5, 95));
        gpi(&l, END_PATH, &[hps]);
    }
}
