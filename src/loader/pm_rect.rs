// SPDX-License-Identifier: GPL-3.0-or-later
//
// PM rectangle helpers: WinSetRect, WinSetRectEmpty, WinCopyRect,
// WinOffsetRect, WinInflateRect, WinPtInRect, WinIntersectRect,
// WinUnionRect, WinSubtractRect, WinIsRectEmpty, WinEqualRect.
//
// From the PM Programming Reference: a rectangle "has area if its left edge
// coordinate is less than its right edge coordinate, and its bottom edge
// coordinate is less than its top edge coordinate" (WinIsRectEmpty); two
// empty rectangles are equal whatever their coordinates (WinEqualRect); an
// empty source of WinUnionRect yields the other; WinSubtractRect returns
// the first source when the difference is not a rectangle. Warpine's own
// choices (the reference does not say): the right and top edges are outside
// for WinPtInRect, matching the empty-rectangle rule; "an empty rectangle"
// written by WinIntersectRect / WinUnionRect / WinSubtractRect is (0,0,0,0);
// a NULL RECTL or POINTL pointer fails with FALSE and no error code (none
// is listed); coordinates are not limited to 16 bits.

// PMWIN ordinals of the rectangle calls (doc/os2_ordinals.md).
pub(super) const WIN_COPY_RECT: u32 = 710;
pub(super) const WIN_EQUAL_RECT: u32 = 741;
pub(super) const WIN_INFLATE_RECT: u32 = 762;
pub(super) const WIN_INTERSECT_RECT: u32 = 764;
pub(super) const WIN_IS_RECT_EMPTY: u32 = 770;
pub(super) const WIN_OFFSET_RECT: u32 = 792;
pub(super) const WIN_PT_IN_RECT: u32 = 797;
pub(super) const WIN_SET_RECT: u32 = 868;
pub(super) const WIN_SET_RECT_EMPTY: u32 = 869;
pub(super) const WIN_SUBTRACT_RECT: u32 = 887;
pub(super) const WIN_UNION_RECT: u32 = 891;

/// A `RECTL`: exclusive at the right and top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rectl {
    pub x_left: i32,
    pub y_bottom: i32,
    pub x_right: i32,
    pub y_top: i32,
}

/// The empty rectangle the helpers write: (0, 0, 0, 0).
pub const EMPTY_RECTL: Rectl = Rectl { x_left: 0, y_bottom: 0, x_right: 0, y_top: 0 };

/// Size of a `RECTL` in guest memory (four LONGs).
pub const RECTL_SIZE: u32 = 16;

impl Rectl {
    pub const fn new(x_left: i32, y_bottom: i32, x_right: i32, y_top: i32) -> Self {
        Rectl { x_left, y_bottom, x_right, y_top }
    }

    /// No area: left not less than right, or bottom not less than top.
    pub fn is_empty(&self) -> bool {
        self.x_left >= self.x_right || self.y_bottom >= self.y_top
    }

    /// WinEqualRect: identical, or both empty.
    pub fn equals(&self, other: &Rectl) -> bool {
        self == other || (self.is_empty() && other.is_empty())
    }

    /// WinPtInRect: left and bottom edges inside, right and top outside.
    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.x_left && x < self.x_right && y >= self.y_bottom && y < self.y_top
    }

    /// WinOffsetRect.
    pub fn offset(&self, cx: i32, cy: i32) -> Rectl {
        Rectl::new(self.x_left.wrapping_add(cx), self.y_bottom.wrapping_add(cy),
                   self.x_right.wrapping_add(cx), self.y_top.wrapping_add(cy))
    }

    /// WinInflateRect: `cx` off the left and onto the right, `cy` off the
    /// bottom and onto the top.
    pub fn inflate(&self, cx: i32, cy: i32) -> Rectl {
        Rectl::new(self.x_left.wrapping_sub(cx), self.y_bottom.wrapping_sub(cy),
                   self.x_right.wrapping_add(cx), self.y_top.wrapping_add(cy))
    }

    /// WinIntersectRect: the common area, `None` when there is none.
    pub fn intersect(&self, other: &Rectl) -> Option<Rectl> {
        let r = Rectl::new(self.x_left.max(other.x_left), self.y_bottom.max(other.y_bottom),
                           self.x_right.min(other.x_right), self.y_top.min(other.y_top));
        (!self.is_empty() && !other.is_empty() && !r.is_empty()).then_some(r)
    }

    /// WinUnionRect: the bounding rectangle; an empty source gives the
    /// other, two empty sources the empty rectangle.
    pub fn union(&self, other: &Rectl) -> Rectl {
        match (self.is_empty(), other.is_empty()) {
            (true, true) => EMPTY_RECTL,
            (true, false) => *other,
            (false, true) => *self,
            (false, false) => Rectl::new(self.x_left.min(other.x_left), self.y_bottom.min(other.y_bottom),
                                         self.x_right.max(other.x_right), self.y_top.max(other.y_top)),
        }
    }

    /// WinSubtractRect: `self` less `other` when that is a rectangle (the
    /// empty rectangle if `other` covers `self`), otherwise `self`.
    pub fn subtract(&self, other: &Rectl) -> Rectl {
        if self.is_empty() || self.intersect(other).is_none() {
            return *self;
        }
        let spans_x = other.x_left <= self.x_left && other.x_right >= self.x_right;
        let spans_y = other.y_bottom <= self.y_bottom && other.y_top >= self.y_top;
        let mut r = *self;
        match (spans_x, spans_y) {
            (true, true) => return EMPTY_RECTL,
            (true, false) => {
                // A band across the full width: only one off an edge leaves a rectangle.
                if other.y_bottom <= self.y_bottom { r.y_bottom = other.y_top; }
                else if other.y_top >= self.y_top { r.y_top = other.y_bottom; }
            }
            (false, true) => {
                if other.x_left <= self.x_left { r.x_left = other.x_right; }
                else if other.x_right >= self.x_right { r.x_right = other.x_left; }
            }
            (false, false) => {}
        }
        r
    }
}

impl super::Loader {
    /// A `RECTL` from guest memory; `None` for a NULL or unreadable pointer.
    pub(super) fn read_rectl(&self, ptr: u32) -> Option<Rectl> {
        if ptr == 0 { return None; }
        let at = |i: u32| self.guest_read::<i32>(ptr.wrapping_add(4 * i));
        Some(Rectl::new(at(0)?, at(1)?, at(2)?, at(3)?))
    }

    /// Write a `RECTL` to guest memory; false for a NULL or unwritable pointer.
    pub(super) fn write_rectl(&self, ptr: u32, r: Rectl) -> bool {
        if ptr == 0 || self.guest_slice_mut(ptr, RECTL_SIZE as usize).is_none() { return false; }
        [r.x_left, r.y_bottom, r.x_right, r.y_top].iter().enumerate()
            .all(|(i, &v)| self.guest_write::<i32>(ptr.wrapping_add(4 * i as u32), v).is_some())
    }

    /// The rectangle calls of PMWIN by ordinal, `args` the stack arguments
    /// after `hab`. Returns the BOOL result.
    pub(super) fn win_rect_call(&self, ordinal: u32, args: [u32; 5]) -> u32 {
        let r = |ptr: u32| self.read_rectl(ptr);
        let ok = match ordinal {
            WIN_SET_RECT => self.write_rectl(args[0], Rectl::new(
                args[1] as i32, args[2] as i32, args[3] as i32, args[4] as i32)),
            WIN_SET_RECT_EMPTY => self.write_rectl(args[0], EMPTY_RECTL),
            WIN_COPY_RECT => r(args[1]).is_some_and(|src| self.write_rectl(args[0], src)),
            WIN_OFFSET_RECT => r(args[0])
                .is_some_and(|rc| self.write_rectl(args[0], rc.offset(args[1] as i32, args[2] as i32))),
            WIN_INFLATE_RECT => r(args[0])
                .is_some_and(|rc| self.write_rectl(args[0], rc.inflate(args[1] as i32, args[2] as i32))),
            WIN_PT_IN_RECT => match (r(args[0]), self.read_pointl_nonnull(args[1])) {
                (Some(rc), Some(pt)) => rc.contains(pt),
                _ => false,
            },
            WIN_IS_RECT_EMPTY => r(args[0]).is_none_or(|rc| rc.is_empty()),
            WIN_EQUAL_RECT => matches!((r(args[0]), r(args[1])), (Some(a), Some(b)) if a.equals(&b)),
            WIN_INTERSECT_RECT => match (r(args[1]), r(args[2])) {
                (Some(a), Some(b)) => {
                    let common = a.intersect(&b);
                    self.write_rectl(args[0], common.unwrap_or(EMPTY_RECTL)) && common.is_some()
                }
                _ => false,
            },
            WIN_UNION_RECT => match (r(args[1]), r(args[2])) {
                (Some(a), Some(b)) => {
                    let u = a.union(&b);
                    self.write_rectl(args[0], u) && !u.is_empty()
                }
                _ => false,
            },
            WIN_SUBTRACT_RECT => match (r(args[1]), r(args[2])) {
                (Some(a), Some(b)) => {
                    let d = a.subtract(&b);
                    self.write_rectl(args[0], d) && !d.is_empty()
                }
                _ => false,
            },
            _ => false,
        };
        ok as u32
    }

    /// A POINTL, `None` for a NULL or unreadable pointer.
    fn read_pointl_nonnull(&self, ptr: u32) -> Option<(i32, i32)> {
        if ptr == 0 { None } else { self.read_pointl(ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::Loader;

    const A: Rectl = Rectl::new(10, 20, 30, 40);

    #[test]
    fn empty_means_no_area() {
        assert!(!A.is_empty());
        assert!(Rectl::new(5, 5, 5, 9).is_empty(), "zero width");
        assert!(Rectl::new(5, 9, 8, 9).is_empty(), "zero height");
        assert!(Rectl::new(9, 0, 5, 9).is_empty(), "left right of right");
        assert!(EMPTY_RECTL.is_empty());
    }

    #[test]
    fn equal_rectangles_and_any_two_empty_ones() {
        assert!(A.equals(&A));
        assert!(!A.equals(&A.offset(1, 0)));
        assert!(Rectl::new(1, 1, 1, 5).equals(&Rectl::new(7, 7, 3, 3)), "both empty");
        assert!(!A.equals(&EMPTY_RECTL));
    }

    #[test]
    fn point_inside_excludes_right_and_top_edges() {
        assert!(A.contains((10, 20)), "bottom-left corner");
        assert!(A.contains((29, 39)));
        assert!(!A.contains((30, 25)), "right edge");
        assert!(!A.contains((15, 40)), "top edge");
        assert!(!A.contains((9, 25)));
    }

    #[test]
    fn offset_and_inflate() {
        assert_eq!(A.offset(5, -10), Rectl::new(15, 10, 35, 30));
        assert_eq!(A.inflate(2, 3), Rectl::new(8, 17, 32, 43));
        assert_eq!(A.inflate(-2, -3), Rectl::new(12, 23, 28, 37));
        assert_eq!(Rectl::new(i32::MAX, 0, i32::MAX, 0).offset(1, 0).x_left, i32::MIN, "wraps, no panic");
    }

    #[test]
    fn intersection() {
        assert_eq!(A.intersect(&Rectl::new(25, 0, 50, 30)), Some(Rectl::new(25, 20, 30, 30)));
        assert_eq!(A.intersect(&Rectl::new(30, 20, 40, 40)), None, "touching edges only");
        assert_eq!(A.intersect(&EMPTY_RECTL), None);
        assert_eq!(A.intersect(&A), Some(A));
    }

    #[test]
    fn union_skips_empty_sources() {
        assert_eq!(A.union(&Rectl::new(0, 0, 5, 5)), Rectl::new(0, 0, 30, 40));
        assert_eq!(A.union(&Rectl::new(100, 100, 100, 200)), A, "empty second source");
        assert_eq!(Rectl::new(3, 3, 1, 1).union(&A), A, "empty first source");
        assert_eq!(EMPTY_RECTL.union(&Rectl::new(4, 4, 4, 4)), EMPTY_RECTL);
    }

    #[test]
    fn subtraction_cuts_bands_off_edges_only() {
        // A band across the full width at the bottom, then at the top.
        assert_eq!(A.subtract(&Rectl::new(0, 0, 50, 25)), Rectl::new(10, 25, 30, 40));
        assert_eq!(A.subtract(&Rectl::new(0, 35, 50, 99)), Rectl::new(10, 20, 30, 35));
        // Full height off the left and off the right.
        assert_eq!(A.subtract(&Rectl::new(0, 0, 12, 99)), Rectl::new(12, 20, 30, 40));
        assert_eq!(A.subtract(&Rectl::new(28, 0, 99, 99)), Rectl::new(10, 20, 28, 40));
        // A hole in the middle or a corner: not a rectangle, the first source.
        assert_eq!(A.subtract(&Rectl::new(15, 25, 20, 30)), A);
        assert_eq!(A.subtract(&Rectl::new(0, 0, 15, 25)), A);
        // A band through the middle splits it in two: the first source.
        assert_eq!(A.subtract(&Rectl::new(0, 25, 99, 30)), A);
        // Covered completely, disjoint, empty subtrahend.
        assert_eq!(A.subtract(&Rectl::new(0, 0, 99, 99)), EMPTY_RECTL);
        assert_eq!(A.subtract(&Rectl::new(50, 50, 60, 60)), A);
        assert_eq!(A.subtract(&EMPTY_RECTL), A);
    }

    // ── Through guest memory ────────────────────────────────────────────

    const R1: u32 = 0x3000;
    const R2: u32 = 0x3010;
    const R3: u32 = 0x3020;
    const PT: u32 = 0x3030;

    fn put(loader: &Loader, ptr: u32, r: Rectl) { assert!(loader.write_rectl(ptr, r)); }

    #[test]
    fn calls_through_guest_memory() {
        let loader = Loader::new_mock();
        let args = |a: &[u32]| { let mut v = [0u32; 5]; v[..a.len()].copy_from_slice(a); v };
        assert_eq!(loader.win_rect_call(WIN_SET_RECT, args(&[R1, 10, 20, 30, 40])), 1);
        assert_eq!(loader.read_rectl(R1), Some(A));
        assert_eq!(loader.win_rect_call(WIN_COPY_RECT, args(&[R2, R1])), 1);
        assert_eq!(loader.read_rectl(R2), Some(A));
        assert_eq!(loader.win_rect_call(WIN_OFFSET_RECT, args(&[R2, (-5i32) as u32, 5])), 1);
        assert_eq!(loader.read_rectl(R2), Some(Rectl::new(5, 25, 25, 45)));
        assert_eq!(loader.win_rect_call(WIN_INFLATE_RECT, args(&[R2, 1, 1])), 1);
        assert_eq!(loader.read_rectl(R2), Some(Rectl::new(4, 24, 26, 46)));

        assert_eq!(loader.win_rect_call(WIN_INTERSECT_RECT, args(&[R3, R1, R2])), 1);
        assert_eq!(loader.read_rectl(R3), Some(Rectl::new(10, 24, 26, 40)));
        assert_eq!(loader.win_rect_call(WIN_UNION_RECT, args(&[R3, R1, R2])), 1);
        assert_eq!(loader.read_rectl(R3), Some(Rectl::new(4, 20, 30, 46)));
        assert_eq!(loader.win_rect_call(WIN_EQUAL_RECT, args(&[R1, R1])), 1);
        assert_eq!(loader.win_rect_call(WIN_EQUAL_RECT, args(&[R1, R2])), 0);

        // No intersection: FALSE and the empty rectangle.
        put(&loader, R2, Rectl::new(100, 100, 110, 110));
        assert_eq!(loader.win_rect_call(WIN_INTERSECT_RECT, args(&[R3, R1, R2])), 0);
        assert_eq!(loader.read_rectl(R3), Some(EMPTY_RECTL));

        // Subtracting a covering rectangle: FALSE (empty result).
        put(&loader, R2, Rectl::new(0, 0, 99, 99));
        assert_eq!(loader.win_rect_call(WIN_SUBTRACT_RECT, args(&[R3, R1, R2])), 0);
        assert_eq!(loader.read_rectl(R3), Some(EMPTY_RECTL));

        loader.guest_write::<i32>(PT, 10);
        loader.guest_write::<i32>(PT + 4, 39);
        assert_eq!(loader.win_rect_call(WIN_PT_IN_RECT, args(&[R1, PT])), 1);
        loader.guest_write::<i32>(PT, 30);
        assert_eq!(loader.win_rect_call(WIN_PT_IN_RECT, args(&[R1, PT])), 0);

        assert_eq!(loader.win_rect_call(WIN_IS_RECT_EMPTY, args(&[R1])), 0);
        assert_eq!(loader.win_rect_call(WIN_SET_RECT_EMPTY, args(&[R1])), 1);
        assert_eq!(loader.read_rectl(R1), Some(EMPTY_RECTL));
        assert_eq!(loader.win_rect_call(WIN_IS_RECT_EMPTY, args(&[R1])), 1);
    }

    #[test]
    fn null_pointers_fail() {
        let loader = Loader::new_mock();
        assert_eq!(loader.win_rect_call(WIN_SET_RECT, [0, 1, 2, 3, 4]), 0);
        assert_eq!(loader.win_rect_call(WIN_COPY_RECT, [R1, 0, 0, 0, 0]), 0);
        assert_eq!(loader.win_rect_call(WIN_PT_IN_RECT, [R1, 0, 0, 0, 0]), 0);
        assert_eq!(loader.win_rect_call(WIN_UNION_RECT, [0, R1, R2, 0, 0]), 0);
        assert_eq!(loader.win_rect_call(WIN_EQUAL_RECT, [0, 0, 0, 0, 0]), 0);
        // Out of guest memory.
        assert_eq!(loader.win_rect_call(WIN_SET_RECT_EMPTY, [u32::MAX - 3, 0, 0, 0, 0]), 0);
    }
}
