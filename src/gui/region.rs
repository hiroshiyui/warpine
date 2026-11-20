// SPDX-License-Identifier: GPL-3.0-or-later
//
// GPI regions and the scan conversion shared by the shape rasteriser and
// regions made from paths.
//
// A `Region` is a set of pels held as y-bands: horizontal strips
// `y0..y1` (PM coordinates, y growing upwards, exclusive at the top) each
// with sorted, disjoint, non-touching x spans `x0..x1` (exclusive at the
// right). Bands are sorted bottom-up, do not overlap, are never empty, and
// two touching bands never have the same spans (they are merged), so equal
// pel sets have equal representations and `==` compares regions.
//
// This is the representation of `GpiCreateRegion` rectangles ("points on
// the right-hand and top boundaries are not included") and of clipping: a
// draw through a presentation space with a clip region keeps only the pels
// of the region (`Surface::draw_clipped`).

/// Region coordinates are bounded to ± this, the range of GPI coordinates
/// in a `GPIF_LONG` presentation space (28 bits, signed); offsets saturate
/// there, so region arithmetic cannot overflow.
pub const REGION_COORD_LIMIT: i32 = 1 << 27;

/// Most rectangles in one region. Guest calls whose result would need more
/// (an exotic combination, a path with many rows) fail instead of growing
/// host memory without bound.
pub const MAX_REGION_RECTS: usize = 16384;

/// Shapes turned into regions (clip paths, `GpiPathToRegion`) are scan
/// converted within ± this, well beyond any surface (`MAX_SURFACE_DIM`);
/// points further out are clamped.
pub const RASTER_LIMIT: i32 = 16384;

/// A rectangle of pels `x0..x1` × `y0..y1` (exclusive at the right and top).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl RRect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> RRect {
        RRect { x0, y0, x1, y1 }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// Clamped to ±[`REGION_COORD_LIMIT`].
    fn bounded(self) -> RRect {
        let l = |v: i32| v.clamp(-REGION_COORD_LIMIT, REGION_COORD_LIMIT);
        RRect::new(l(self.x0), l(self.y0), l(self.x1), l(self.y1))
    }
}

/// How `Region::combine` joins two regions (`CRGN_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOp {
    /// Union.
    Or,
    /// The first region only.
    Copy,
    /// Pels in exactly one of them.
    Xor,
    /// Intersection.
    And,
    /// Pels of the first that are not in the second.
    Diff,
}

impl SetOp {
    fn keep(self, a: bool, b: bool) -> bool {
        match self {
            SetOp::Or => a || b,
            SetOp::Copy => a,
            SetOp::Xor => a != b,
            SetOp::And => a && b,
            SetOp::Diff => a && !b,
        }
    }
}

/// How much of a rectangle lies in a region (`RRGN_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlap {
    Outside,
    Partial,
    Inside,
}

/// The shape of a region (`RGN_NULL`, `RGN_RECT`, `RGN_COMPLEX`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Complexity {
    Null,
    Rect,
    Complex,
}

/// The order `Region::rects` lists rectangles in (`RECTDIR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectOrder {
    LeftRightTopBottom,
    RightLeftTopBottom,
    LeftRightBottomTop,
    RightLeftBottomTop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Band {
    y0: i32,
    y1: i32,
    spans: Vec<(i32, i32)>,
}

/// A set of pels (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    bands: Vec<Band>,
}

/// Merge sorted (by start) spans that overlap or touch.
fn merge_sorted(spans: &mut Vec<(i32, i32)>) {
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(spans.len());
    for &(s, e) in spans.iter() {
        if s >= e { continue; }
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *spans = out;
}

/// Combine two sorted, disjoint span lists.
fn combine_spans(a: &[(i32, i32)], b: &[(i32, i32)], op: SetOp) -> Vec<(i32, i32)> {
    let mut xs: Vec<i32> = a.iter().chain(b).flat_map(|&(s, e)| [s, e]).collect();
    xs.sort_unstable();
    xs.dedup();
    let (mut ia, mut ib) = (0, 0);
    let mut out: Vec<(i32, i32)> = Vec::new();
    for w in xs.windows(2) {
        let (x0, x1) = (w[0], w[1]);
        while ia < a.len() && a[ia].1 <= x0 { ia += 1; }
        while ib < b.len() && b[ib].1 <= x0 { ib += 1; }
        let in_a = ia < a.len() && a[ia].0 <= x0;
        let in_b = ib < b.len() && b[ib].0 <= x0;
        if op.keep(in_a, in_b) {
            match out.last_mut() {
                Some(last) if last.1 == x0 => last.1 = x1,
                _ => out.push((x0, x1)),
            }
        }
    }
    out
}

impl Region {
    pub fn empty() -> Region {
        Region::default()
    }

    /// The pels of one rectangle (empty if it is).
    pub fn rect(r: RRect) -> Region {
        let r = r.bounded();
        if r.is_empty() { return Region::empty(); }
        Region { bands: vec![Band { y0: r.y0, y1: r.y1, spans: vec![(r.x0, r.x1)] }] }
    }

    /// Every pel GPI coordinates can address.
    pub fn infinite() -> Region {
        let l = REGION_COORD_LIMIT;
        Region::rect(RRect::new(-l, -l, l, l))
    }

    /// The union of `rects` (they may overlap); `None` when the result has
    /// more than [`MAX_REGION_RECTS`] rectangles.
    pub fn from_rects(rects: &[RRect]) -> Option<Region> {
        let rects: Vec<RRect> = rects.iter().map(|r| r.bounded()).filter(|r| !r.is_empty()).collect();
        let mut ys: Vec<i32> = rects.iter().flat_map(|r| [r.y0, r.y1]).collect();
        ys.sort_unstable();
        ys.dedup();
        let mut region = Region::empty();
        let mut spans = Vec::new();
        for w in ys.windows(2) {
            let (y0, y1) = (w[0], w[1]);
            spans.clear();
            spans.extend(rects.iter().filter(|r| r.y0 <= y0 && y1 <= r.y1).map(|r| (r.x0, r.x1)));
            spans.sort_unstable();
            merge_sorted(&mut spans);
            region.push_band(y0, y1, &spans);
            if region.rect_count() > MAX_REGION_RECTS { return None; }
        }
        Some(region)
    }

    /// Build from rows: `rows[i]` holds the spans (any order, may overlap)
    /// of PM row `y0 + i`. `None` when too complex.
    pub fn from_rows(y0: i32, rows: Vec<Vec<(i32, i32)>>) -> Option<Region> {
        let mut region = Region::empty();
        for (i, mut spans) in rows.into_iter().enumerate() {
            spans.sort_unstable();
            merge_sorted(&mut spans);
            let y = y0 + i as i32;
            region.push_band(y, y + 1, &spans);
            if region.rect_count() > MAX_REGION_RECTS { return None; }
        }
        Some(region)
    }

    /// Append a band above every existing one, merging it into the top band
    /// when it touches it with the same spans.
    fn push_band(&mut self, y0: i32, y1: i32, spans: &[(i32, i32)]) {
        if spans.is_empty() || y0 >= y1 { return; }
        if let Some(last) = self.bands.last_mut()
            && last.y1 == y0 && last.spans == spans
        {
            last.y1 = y1;
            return;
        }
        self.bands.push(Band { y0, y1, spans: spans.to_vec() });
    }

    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Number of rectangles (one per span of each band).
    pub fn rect_count(&self) -> usize {
        self.bands.iter().map(|b| b.spans.len()).sum()
    }

    pub fn complexity(&self) -> Complexity {
        match self.rect_count() {
            0 => Complexity::Null,
            1 => Complexity::Rect,
            _ => Complexity::Complex,
        }
    }

    /// The smallest rectangle holding every pel; `None` when empty.
    pub fn bounds(&self) -> Option<RRect> {
        let (first, last) = (self.bands.first()?, self.bands.last()?);
        let x0 = self.bands.iter().map(|b| b.spans[0].0).min()?;
        let x1 = self.bands.iter().map(|b| b.spans[b.spans.len() - 1].1).max()?;
        Some(RRect::new(x0, first.y0, x1, last.y1))
    }

    /// The rectangles, in `order` (the band's rectangles share their y
    /// range; within a band, x order).
    pub fn rects(&self, order: RectOrder) -> Vec<RRect> {
        let top_first = matches!(order, RectOrder::LeftRightTopBottom | RectOrder::RightLeftTopBottom);
        let right_first = matches!(order, RectOrder::RightLeftTopBottom | RectOrder::RightLeftBottomTop);
        let band_rects = |b: &Band| -> Vec<RRect> {
            let mut v: Vec<RRect> = b.spans.iter().map(|&(x0, x1)| RRect::new(x0, b.y0, x1, b.y1)).collect();
            if right_first { v.reverse(); }
            v
        };
        if top_first {
            self.bands.iter().rev().flat_map(band_rects).collect()
        } else {
            self.bands.iter().flat_map(band_rects).collect()
        }
    }

    /// The spans of PM row `y` (empty outside the region).
    pub fn spans_at(&self, y: i32) -> &[(i32, i32)] {
        let i = self.bands.partition_point(|b| b.y1 <= y);
        match self.bands.get(i) {
            Some(b) if b.y0 <= y => &b.spans,
            _ => &[],
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.spans_at(y).iter().any(|&(s, e)| s <= x && x < e)
    }

    /// How much of `r` lies in the region (an empty `r` is outside).
    pub fn overlap(&self, r: RRect) -> Overlap {
        if r.is_empty() { return Overlap::Outside; }
        let inter = Region::combine(self, &Region::rect(r), SetOp::And);
        match inter {
            Some(i) if i.is_empty() => Overlap::Outside,
            Some(i) if i == Region::rect(r) => Overlap::Inside,
            _ => Overlap::Partial,
        }
    }

    /// Move every pel by `(dx, dy)` (saturating at the coordinate limit).
    pub fn offset(&mut self, dx: i32, dy: i32) {
        let l = REGION_COORD_LIMIT;
        let mv = |v: i32, d: i32| v.saturating_add(d).clamp(-l, l);
        for b in &mut self.bands {
            b.y0 = mv(b.y0, dy);
            b.y1 = mv(b.y1, dy);
            for s in &mut b.spans {
                *s = (mv(s.0, dx), mv(s.1, dx));
            }
            b.spans.retain(|&(s, e)| s < e);
            merge_sorted(&mut b.spans);
        }
        // Clamping can collapse bands or spans at the limits: rebuild.
        let bands = std::mem::take(&mut self.bands);
        for b in bands {
            self.push_band(b.y0, b.y1, &b.spans);
        }
    }

    /// `a` joined with `b` by `op`; `None` when the result would have more
    /// than [`MAX_REGION_RECTS`] rectangles.
    pub fn combine(a: &Region, b: &Region, op: SetOp) -> Option<Region> {
        if op == SetOp::Copy { return Some(a.clone()); }
        let mut ys: Vec<i32> = a.bands.iter().chain(&b.bands).flat_map(|b| [b.y0, b.y1]).collect();
        ys.sort_unstable();
        ys.dedup();
        let (mut ia, mut ib) = (0, 0);
        let mut out = Region::empty();
        for w in ys.windows(2) {
            let (y0, y1) = (w[0], w[1]);
            while ia < a.bands.len() && a.bands[ia].y1 <= y0 { ia += 1; }
            while ib < b.bands.len() && b.bands[ib].y1 <= y0 { ib += 1; }
            let sa: &[(i32, i32)] = match a.bands.get(ia) { Some(x) if x.y0 <= y0 => &x.spans, _ => &[] };
            let sb: &[(i32, i32)] = match b.bands.get(ib) { Some(x) if x.y0 <= y0 => &x.spans, _ => &[] };
            if sa.is_empty() && sb.is_empty() { continue; }
            let spans = combine_spans(sa, sb, op);
            out.push_band(y0, y1, &spans);
            if out.rect_count() > MAX_REGION_RECTS { return None; }
        }
        Some(out)
    }

    /// The region with x and y swapped.
    fn transposed(&self) -> Option<Region> {
        let rects: Vec<RRect> = self.rects(RectOrder::LeftRightBottomTop).into_iter()
            .map(|r| RRect::new(r.y0, r.x0, r.y1, r.x1)).collect();
        Region::from_rects(&rects)
    }

    /// Pels whose horizontal neighbours up to `d` pels away are all in
    /// the region.
    fn eroded_x(&self, d: i32) -> Region {
        let mut out = Region::empty();
        for b in &self.bands {
            let spans: Vec<(i32, i32)> = b.spans.iter()
                .map(|&(s, e)| (s.saturating_add(d), e.saturating_sub(d)))
                .filter(|&(s, e)| s < e).collect();
            // Bands that become equal are merged by push_band only when
            // they touch the previous one, which the representation needs.
            out.push_band(b.y0, b.y1, &spans);
        }
        out
    }

    /// The frame of `GpiFrameRegion`: the pels of the region less than
    /// `cx` pels (horizontally) or `cy` pels (vertically) inside its
    /// boundary, so a `cx` × `cy` rectangle traced along the inside of the
    /// boundary covers it. `None` when too complex.
    pub fn frame(&self, cx: i32, cy: i32) -> Option<Region> {
        let (cx, cy) = (cx.clamp(0, REGION_COORD_LIMIT), cy.clamp(0, REGION_COORD_LIMIT));
        // Erosion by a box is erosion by its rows then by its columns.
        let inner = self.eroded_x(cx).transposed()?.eroded_x(cy).transposed()?;
        Region::combine(self, &inner, SetOp::Diff)
    }

    /// The pels of `figures` (each a polygon through its points): the
    /// inside by `rule` (pel centres, as the shape rasteriser fills), when
    /// given, plus the pels a `boundary`-wide square brush covers along the
    /// figures' edges (each closed back to its start when `closed`); with
    /// no boundary the inside leaves out the bottom and right edges
    /// ([`Edges::TopLeft`]).
    /// Coordinates are clamped to ±[`RASTER_LIMIT`]. `None` when too
    /// complex.
    pub fn from_figures(figures: &[(&[(i32, i32)], bool)], rule: Option<FillRule>, boundary: u32) -> Option<Region> {
        let lim = |(x, y): (i32, i32)| (x.clamp(-RASTER_LIMIT, RASTER_LIMIT), y.clamp(-RASTER_LIMIT, RASTER_LIMIT));
        let figs: Vec<(Vec<(i32, i32)>, bool)> = figures.iter()
            .map(|(p, c)| (p.iter().copied().map(lim).collect(), *c)).collect();
        let all = figs.iter().flat_map(|(p, _)| p.iter().copied());
        let reach = boundary as i32 / 2 + 1;
        let (mut y0, mut y1) = (i32::MAX, i32::MIN);
        for (_, y) in all {
            (y0, y1) = (y0.min(y), y1.max(y));
        }
        if y0 > y1 { return Some(Region::empty()); }
        let (y0, y1) = (y0 - reach, y1 + reach);
        let mut rows: Vec<Vec<(i32, i32)>> = vec![Vec::new(); (y1 - y0 + 1) as usize];
        let edges = if boundary == 0 { Edges::TopLeft } else { Edges::Centres };
        if let Some(rule) = rule {
            let segs = fill_segments(figs.iter().map(|(p, _)| p.as_slice()));
            let mut spans = Vec::new();
            for (i, row) in rows.iter_mut().enumerate() {
                scan_row(&segs, rule, edges, y0 + i as i32, &mut spans);
                row.extend(spans.iter().map(|&(a, b)| (a, b + 1)));
            }
        }
        if boundary > 0 {
            let w = boundary as i32;
            let (lo, hi) = (-(w - 1) / 2, w / 2);
            let bounds = (-RASTER_LIMIT - w, -RASTER_LIMIT - w, RASTER_LIMIT + w, RASTER_LIMIT + w);
            for (pts, closed) in &figs {
                for seg in outline_segments(pts, *closed) {
                    walk_segment(seg, bounds, |x, y| {
                        for oy in lo..=hi {
                            let r = y + oy - y0;
                            if (0..rows.len() as i32).contains(&r) {
                                rows[r as usize].push((x + lo, x + hi + 1));
                            }
                        }
                    });
                }
            }
        }
        Region::from_rows(y0, rows)
    }
}

/// The interior rule of an area, path or polygon fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    /// Inside where a ray to infinity crosses the boundary an odd number
    /// of times (`BA_ALTERNATE`, `FPATH_ALTERNATE`, …).
    Alternate,
    /// Inside where the crossings, counted +1 or −1 by direction, do not
    /// sum to zero (`BA_WINDING`, `FPATH_WINDING`, …).
    Winding,
}

/// A polygon edge from the first point to the second.
pub type Segment = ((i32, i32), (i32, i32));

/// The edges the interior of `figures` is found from: each figure of at
/// least three points closed back to its start.
pub fn fill_segments<'a>(figures: impl Iterator<Item = &'a [(i32, i32)]>) -> Vec<Segment> {
    let mut segs = Vec::new();
    for pts in figures {
        if pts.len() < 3 { continue; }
        segs.extend(pts.windows(2).map(|w| (w[0], w[1])));
        segs.push((pts[pts.len() - 1], pts[0]));
    }
    segs
}

/// The edges an outline follows: consecutive points, plus the closing edge
/// when `closed` and there are more than two points; a single point is a
/// zero-length edge.
pub fn outline_segments(pts: &[(i32, i32)], closed: bool) -> Vec<Segment> {
    let mut segs: Vec<Segment> = pts.windows(2).map(|w| (w[0], w[1])).collect();
    if closed && pts.len() > 2 { segs.push((pts[pts.len() - 1], pts[0])); }
    if pts.len() == 1 { segs.push((pts[0], pts[0])); }
    segs
}

/// Which pels on the edges of a polygon its inside holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    /// Pel centres on the bottom, left and right edges (an edge spans
    /// [low y, high y)), as the shape rasteriser has always filled; with a
    /// one-pel boundary added, every edge pel.
    Centres,
    /// Without the bottom and right edges (`BA_EXCL`, `FPATH_EXCL`,
    /// `POLYGON_EXCL`): an edge spans (low y, high y], spans end before
    /// the right crossing.
    TopLeft,
}

/// The inside spans of PM row `y` (pel `x` inclusive ranges, sorted) of the
/// polygon edges `segs`: pel centres between edge crossings under `rule`,
/// with the edge pels `edges` says. A shared vertex counts once.
pub fn scan_row(segs: &[Segment], rule: FillRule, edges: Edges, y: i32, out: &mut Vec<(i32, i32)>) {
    out.clear();
    let yf = y as f64;
    let top_left = edges == Edges::TopLeft;
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for &((ax, ay), (bx, by)) in segs {
        let (ayf, byf) = (ay as f64, by as f64);
        let crosses = if top_left {
            (ayf < yf && yf <= byf) || (byf < yf && yf <= ayf)
        } else {
            (ayf <= yf && yf < byf) || (byf <= yf && yf < ayf)
        };
        if crosses {
            let x = ax as f64 + (yf - ayf) * (bx - ax) as f64 / (byf - ayf);
            xs.push((x, if by > ay { 1 } else { -1 }));
        }
    }
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut push = |a: f64, b: f64| {
        let from = a.ceil().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        let to = if top_left { b.ceil() - 1.0 } else { b.floor() };
        let to = to.clamp(i32::MIN as f64, i32::MAX as f64) as i32;
        if from <= to { out.push((from, to)); }
    };
    match rule {
        FillRule::Alternate => {
            for pair in xs.as_chunks::<2>().0 {
                push(pair[0].0, pair[1].0);
            }
        }
        FillRule::Winding => {
            let mut wind = 0;
            let mut start = 0.0;
            for &(x, d) in &xs {
                let before = wind;
                wind += d;
                if before == 0 && wind != 0 {
                    start = x;
                } else if before != 0 && wind == 0 {
                    push(start, x);
                }
            }
        }
    }
}

/// Clip the segment to `(xmin, ymin, xmax, ymax)` (inclusive, Liang–Barsky);
/// a segment with both ends inside is returned unchanged. `None` when it
/// misses the box.
pub fn clip_segment(((x1, y1), (x2, y2)): Segment, (xmin, ymin, xmax, ymax): (i32, i32, i32, i32))
                    -> Option<Segment> {
    let (xmin, ymin, xmax, ymax) = (xmin as f64, ymin as f64, xmax as f64, ymax as f64);
    let inside = |x: i32, y: i32| {
        (xmin..=xmax).contains(&(x as f64)) && (ymin..=ymax).contains(&(y as f64))
    };
    if inside(x1, y1) && inside(x2, y2) {
        return Some(((x1, y1), (x2, y2)));
    }
    let (fx, fy) = (x1 as f64, y1 as f64);
    let (dx, dy) = (x2 as f64 - fx, y2 as f64 - fy);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, fx - xmin), (dx, xmax - fx), (-dy, fy - ymin), (dy, ymax - fy)] {
        if p == 0.0 {
            if q < 0.0 { return None; }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 { return None; }
            t0 = t0.max(r);
        } else {
            if r < t0 { return None; }
            t1 = t1.min(r);
        }
    }
    let at = |t: f64| ((fx + t * dx).round() as i32, (fy + t * dy).round() as i32);
    Some((at(t0), at(t1)))
}

/// Visit the pels of a segment (Bresenham, both ends included), clipped to
/// `bounds` first so the work is bounded by the box.
pub fn walk_segment(seg: Segment, bounds: (i32, i32, i32, i32), mut f: impl FnMut(i32, i32)) {
    let Some(((x1, y1), (x2, y2))) = clip_segment(seg, bounds) else { return };
    let (dx, dy) = ((x2 - x1).abs(), (y2 - y1).abs());
    let (sx, sy) = (if x1 < x2 { 1 } else { -1 }, if y1 < y2 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x1, y1, dx - dy);
    loop {
        f(x, y);
        if x == x2 && y == y2 { break; }
        let e2 = 2 * err;
        if e2 > -dy { err -= dy; x += sx; }
        if e2 < dx { err += dx; y += sy; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> RRect {
        RRect::new(x0, y0, x1, y1)
    }

    fn pels(reg: &Region) -> usize {
        reg.rects(RectOrder::LeftRightBottomTop).iter()
            .map(|r| ((r.x1 - r.x0) * (r.y1 - r.y0)) as usize).sum()
    }

    #[test]
    fn rectangles_exclude_their_right_and_top_edges() {
        let reg = Region::rect(r(10, 20, 30, 25));
        assert!(reg.contains(10, 20));
        assert!(reg.contains(29, 24));
        assert!(!reg.contains(30, 24) && !reg.contains(29, 25));
        assert_eq!(reg.bounds(), Some(r(10, 20, 30, 25)));
        assert_eq!(reg.complexity(), Complexity::Rect);
        assert_eq!(Region::rect(r(5, 5, 5, 9)).complexity(), Complexity::Null);
    }

    #[test]
    fn overlapping_rectangles_are_joined_into_bands() {
        let reg = Region::from_rects(&[r(0, 0, 10, 10), r(5, 5, 15, 15)]).unwrap();
        assert_eq!(pels(&reg), 100 + 100 - 25);
        assert_eq!(reg.rect_count(), 3, "bottom, middle, top bands");
        assert_eq!(reg.complexity(), Complexity::Complex);
        // Side by side rectangles of the same height are one.
        let one = Region::from_rects(&[r(0, 0, 5, 5), r(5, 0, 9, 5)]).unwrap();
        assert_eq!(one, Region::rect(r(0, 0, 9, 5)));
        // Stacked rectangles of the same width too.
        let one = Region::from_rects(&[r(0, 0, 5, 5), r(0, 5, 5, 9)]).unwrap();
        assert_eq!(one, Region::rect(r(0, 0, 5, 9)));
    }

    #[test]
    fn set_operations() {
        let a = Region::rect(r(0, 0, 10, 10));
        let b = Region::rect(r(5, 0, 15, 10));
        assert_eq!(Region::combine(&a, &b, SetOp::Or).unwrap(), Region::rect(r(0, 0, 15, 10)));
        assert_eq!(Region::combine(&a, &b, SetOp::And).unwrap(), Region::rect(r(5, 0, 10, 10)));
        assert_eq!(Region::combine(&a, &b, SetOp::Diff).unwrap(), Region::rect(r(0, 0, 5, 10)));
        let x = Region::combine(&a, &b, SetOp::Xor).unwrap();
        assert_eq!(x, Region::from_rects(&[r(0, 0, 5, 10), r(10, 0, 15, 10)]).unwrap());
        assert_eq!(Region::combine(&a, &b, SetOp::Copy).unwrap(), a);
        // A hole.
        let hole = Region::combine(&a, &Region::rect(r(3, 3, 7, 7)), SetOp::Diff).unwrap();
        assert_eq!(pels(&hole), 84);
        assert!(!hole.contains(5, 5) && hole.contains(2, 5) && hole.contains(5, 8));
        assert!(Region::combine(&hole, &hole, SetOp::Xor).unwrap().is_empty());
    }

    #[test]
    fn rects_are_listed_in_the_requested_order() {
        let reg = Region::from_rects(&[r(0, 0, 2, 2), r(4, 0, 6, 2), r(0, 4, 2, 6)]).unwrap();
        let tl = reg.rects(RectOrder::LeftRightTopBottom);
        assert_eq!(tl, vec![r(0, 4, 2, 6), r(0, 0, 2, 2), r(4, 0, 6, 2)]);
        let rb = reg.rects(RectOrder::RightLeftBottomTop);
        assert_eq!(rb, vec![r(4, 0, 6, 2), r(0, 0, 2, 2), r(0, 4, 2, 6)]);
    }

    #[test]
    fn overlap_and_offset() {
        let reg = Region::rect(r(0, 0, 10, 10));
        assert_eq!(reg.overlap(r(2, 2, 5, 5)), Overlap::Inside);
        assert_eq!(reg.overlap(r(8, 8, 12, 12)), Overlap::Partial);
        assert_eq!(reg.overlap(r(10, 0, 12, 5)), Overlap::Outside);
        let mut m = reg.clone();
        m.offset(5, -3);
        assert_eq!(m, Region::rect(r(5, -3, 15, 7)));
        // Saturates at the coordinate limit instead of overflowing.
        m.offset(i32::MAX, i32::MAX);
        assert!(m.is_empty(), "squeezed to nothing at the limit: {m:?}");
        let mut inf = Region::infinite();
        inf.offset(-5, 0);
        assert!(inf.contains(0, 0));
    }

    #[test]
    fn frame_traces_the_inside_of_the_boundary() {
        let reg = Region::rect(r(0, 0, 10, 8));
        let f = reg.frame(2, 1).unwrap();
        // 10×8 less the 6×6 inside.
        assert_eq!(pels(&f), 80 - 36);
        assert!(f.contains(0, 4) && f.contains(1, 4) && !f.contains(2, 4));
        assert!(f.contains(5, 0) && !f.contains(5, 1) && f.contains(5, 7));
        // Too thick: everything is frame.
        assert_eq!(reg.frame(20, 20).unwrap(), reg);
        assert_eq!(reg.frame(0, 0).unwrap(), Region::empty());
    }

    #[test]
    fn too_many_rectangles_fail() {
        let rects: Vec<RRect> = (0..(MAX_REGION_RECTS as i32 + 1)).map(|i| r(2 * i, 0, 2 * i + 1, 1)).collect();
        assert!(Region::from_rects(&rects).is_none());
    }

    #[test]
    fn winding_and_alternate_rules_differ_on_overlapping_loops() {
        // Two overlapping squares drawn in the same direction.
        let a = [(0, 0), (10, 0), (10, 10), (0, 10)];
        let b = [(5, 5), (15, 5), (15, 15), (5, 15)];
        let figs = [(&a[..], true), (&b[..], true)];
        let alt = Region::from_figures(&figs, Some(FillRule::Alternate), 0).unwrap();
        let wind = Region::from_figures(&figs, Some(FillRule::Winding), 0).unwrap();
        assert!(!alt.contains(7, 7), "the overlap is outside with alternate");
        assert!(wind.contains(7, 7), "and inside with winding");
        assert!(alt.contains(2, 2) && wind.contains(12, 12));
        // With the boundary, the top and right edges are in.
        let incl = Region::from_figures(&figs[..1], Some(FillRule::Alternate), 1).unwrap();
        assert_eq!(incl, Region::rect(r(0, 0, 11, 11)));
        let excl = Region::from_figures(&figs[..1], Some(FillRule::Alternate), 0).unwrap();
        assert_eq!(excl, Region::rect(r(0, 1, 10, 11)), "the top and left edges, not the bottom and right");
    }

    #[test]
    fn scan_row_follows_the_rule() {
        let segs = fill_segments([&[(0, 0), (10, 0), (10, 10), (0, 10)][..]].into_iter());
        let mut out = Vec::new();
        scan_row(&segs, FillRule::Alternate, Edges::Centres, 5, &mut out);
        assert_eq!(out, vec![(0, 10)]);
        scan_row(&segs, FillRule::Alternate, Edges::Centres, 10, &mut out);
        assert!(out.is_empty(), "the top edge is not crossed");
        scan_row(&segs, FillRule::Alternate, Edges::TopLeft, 10, &mut out);
        assert_eq!(out, vec![(0, 9)], "the top edge, not the right one");
        scan_row(&segs, FillRule::Alternate, Edges::TopLeft, 0, &mut out);
        assert!(out.is_empty(), "nor the bottom one");
    }
}
