// SPDX-License-Identifier: GPL-3.0-or-later

//! A Warpine process's desktop when the session has a desktop server
//! (ADR-0052): instead of drawing on a window of its own, the process
//! renders each visible frame — client pels, title bar, border, menu bar,
//! text cursor — into a shared pixmap, each open pull-down and pop-up menu
//! into one of their own, and sends the server what it shows (`Update`):
//! the pictures back to front with their places and painted parts, the
//! damaged pixmap areas, its active frame and the frames it raised or
//! lowered.
//!
//! Pictures are rendered exactly as the layered compositor renders its frame
//! textures (`draw_placed_frame`, the same partial updates of changed client
//! pels), so the server's composite matches the in-process one pel for pel.

use std::collections::HashMap;
use std::sync::Arc;

use log::warn;

use crate::desktop::client::DesktopLink;
use crate::desktop::pixmap::{SharedPixmap, MAX_PIXMAP_DIM};
use crate::desktop::proto::{Activation, Area, ClientMsg, Item, Update, MAX_DAMAGE, MAX_ITEMS};
use crate::gui::cursor::CursorMark;
use crate::gui::damage::Rect;
use crate::loader::MutexExt;

use super::{copy_rect, cursor_clip, painted_parts, partial_upload, Scene, SceneFrame};
use super::super::{draw_placed_frame, DESKTOP_BG, MENU_DROPDOWN_W};

/// A pixmap this process shares with the server, by id.
struct Shared {
    id: u32,
    pix: SharedPixmap,
}

/// What a frame's picture holds, to tell what must be redrawn.
struct FrameLayer {
    pixmap: Shared,
    size: (i32, i32),
    fb_size: (u32, u32),
    generation: u64,
    cursor: Option<CursorMark>,
    parts: Vec<Rect>,
    /// The open pull-down's picture and its height.
    pulldown: Option<(Shared, i32)>,
}

/// The process's side of the shared desktop.
pub(in crate::gui::sdl_renderer) struct RemoteDesktop {
    link: Arc<DesktopLink>,
    size: (u32, u32),
    next_id: u32,
    frames: HashMap<u32, FrameLayer>,
    overlay: Option<(Shared, u64)>,
    scratch: Vec<u32>,
    /// The last update sent, without its damage, so an unchanged state is
    /// not sent again.
    last: Option<Update>,
    /// The host clipboard text, as the server last reported it.
    pub clipboard: String,
}

impl RemoteDesktop {
    pub fn new(link: Arc<DesktopLink>) -> RemoteDesktop {
        let size = link.size;
        RemoteDesktop { link, size, next_id: 1, frames: HashMap::new(), overlay: None, scratch: Vec::new(),
                        last: None, clipboard: String::new() }
    }

    pub fn link(&self) -> &Arc<DesktopLink> { &self.link }

    pub fn size(&self) -> (u32, u32) { self.size }

    pub fn resize(&mut self, w: u32, h: u32) { self.size = (w, h); }

    /// A new pixmap of `w × h`, announced to the server with its memfd.
    fn new_pixmap(&mut self, w: i32, h: i32) -> Option<Shared> {
        let pix = SharedPixmap::create(w as u32, h as u32)
            .map_err(|e| warn!("[GUI] no {w}x{h} shared pixmap: {e}")).ok()?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.link.send(&ClientMsg::NewPixmap { id, width: w as u32, height: h as u32 }, Some(pix.fd()));
        Some(Shared { id, pix })
    }

    fn drop_pixmap(&self, s: Shared) {
        self.link.send(&ClientMsg::DropPixmap { id: s.id }, None);
    }

    /// Bring the pictures up to date with `scene` and tell the server.
    pub fn compose(&mut self, scene: &Scene, active: Activation, raised: Vec<u32>, lowered: Vec<u32>) {
        let mut items = Vec::new();
        let mut damage = Vec::new();
        for f in &scene.frames {
            self.update_frame(f, scene.full || scene.chrome_changed.contains(&f.placement.hwnd),
                              &mut items, &mut damage);
        }
        // Frames no longer shown.
        let shown: Vec<u32> = scene.frames.iter().map(|f| f.placement.hwnd).collect();
        let gone: Vec<u32> = self.frames.keys().copied().filter(|h| !shown.contains(h)).collect();
        for h in gone {
            if let Some(l) = self.frames.remove(&h) {
                if let Some((pd, _)) = l.pulldown { self.drop_pixmap(pd); }
                self.drop_pixmap(l.pixmap);
            }
        }
        let overlay = self.update_overlay(scene, &mut damage);
        // Too much damage: whole pictures instead.
        if damage.len() > MAX_DAMAGE {
            damage = items.iter().map(|it: &Item| (it.pixmap, whole(&it.parts))).take(MAX_DAMAGE).collect();
        }
        items.truncate(MAX_ITEMS);
        let update = Update { active, raised, lowered, items, damage, overlay };
        let unchanged = update.damage.is_empty() && update.raised.is_empty() && update.lowered.is_empty()
            && self.last.as_ref().is_some_and(|l| *l == update);
        if !unchanged {
            self.link.send(&ClientMsg::Update(update.clone()), None);
            self.last = Some(Update { damage: Vec::new(), raised: Vec::new(), lowered: Vec::new(), ..update });
        }
    }

    /// Bring frame `f`'s picture (and its pull-down's) up to date; add its
    /// items and damage.
    fn update_frame(&mut self, f: &SceneFrame, redraw: bool, items: &mut Vec<Item>, damage: &mut Vec<(u32, Area)>) {
        let p = f.placement;
        let (w, h) = (p.frame.width().min(MAX_PIXMAP_DIM as i32), p.frame.height().min(MAX_PIXMAP_DIM as i32));
        if w <= 0 || h <= 0 {
            if let Some(l) = self.frames.remove(&p.hwnd) { self.drop_pixmap(l.pixmap); }
            return;
        }
        let mut fb = f.surface.lock_or_recover();
        let reusable = !redraw && self.frames.get(&p.hwnd)
            .is_some_and(|l| l.size == (w, h) && l.fb_size == (fb.width, fb.height));
        if reusable {
            let layer = self.frames.get_mut(&p.hwnd).expect("checked above");
            let mut changed = Vec::new();
            if layer.generation != fb.generation() {
                changed.push(fb.take_changes());
                layer.generation = fb.generation();
            }
            if layer.cursor != f.cursor {
                changed.extend([layer.cursor, f.cursor].iter().flatten().map(|c| c.surface_rect(fb.height)));
                layer.cursor = f.cursor;
            }
            for rect in changed {
                let (dst, src) = partial_upload(p, rect, (w, h));
                if dst.is_empty() { continue; }
                copy_rect(&fb.buffer, fb.width as usize, src, &mut self.scratch);
                if let Some(c) = &f.cursor
                    && !c.surface_rect(fb.height).intersect(&src).is_empty() {
                    c.apply(&mut self.scratch, src.width() as usize, (-src.x0, -src.y0), fb.height,
                            Rect::at(0, 0, src.width(), src.height()));
                }
                write_rect(&layer.pixmap.pix, dst, &self.scratch);
                damage.push((layer.pixmap.id, area(dst)));
            }
        } else {
            // The whole picture at the origin, without the pull-down.
            let local = p.moved_without_pulldown(-p.frame.x0, -p.frame.y0);
            self.scratch.clear();
            self.scratch.resize(w as usize * h as usize, DESKTOP_BG);
            draw_placed_frame(&mut self.scratch, w as usize, &local, &fb, f.chrome, Rect::at(0, 0, w, h));
            if let Some(c) = &f.cursor {
                c.apply(&mut self.scratch, w as usize, (local.client.x0, local.client.y0), fb.height,
                        cursor_clip(&local));
            }
            let parts: Vec<Rect> = painted_parts(&local, fb.width, fb.height).iter()
                .map(|r| r.intersect(&Rect::at(0, 0, w, h)))
                .filter(|r| !r.is_empty())
                .collect();
            fb.take_changes();
            let (generation, fb_size) = (fb.generation(), (fb.width, fb.height));
            drop(fb);
            let old = self.frames.remove(&p.hwnd);
            let (pixmap, pulldown) = match old {
                Some(l) if l.size == (w, h) => (l.pixmap, l.pulldown),
                other => {
                    let Some(s) = self.new_pixmap(w, h) else {
                        if let Some(l) = other { self.drop_pixmap(l.pixmap); }
                        return;
                    };
                    if let Some(l) = other {
                        self.drop_pixmap(l.pixmap);
                        (s, l.pulldown)
                    } else {
                        (s, None)
                    }
                }
            };
            write_rect(&pixmap.pix, Rect::at(0, 0, w, h), &self.scratch);
            damage.push((pixmap.id, Area::new(0, 0, w, h)));
            self.frames.insert(p.hwnd, FrameLayer { pixmap, size: (w, h), fb_size, generation,
                                                    cursor: f.cursor, parts, pulldown });
        }
        let layer = self.frames.get(&p.hwnd).expect("present");
        items.push(Item { frame: p.hwnd, pixmap: layer.pixmap.id, x: p.frame.x0, y: p.frame.y0,
                          parts: layer.parts.iter().map(|r| area(*r)).collect(), rounded: p.rounded });

        // The open pull-down follows the chrome.
        let pd = f.chrome.menu.as_ref().and_then(|m| m.pulldown.as_ref());
        let mut layer = self.frames.remove(&p.hwnd).expect("present");
        match pd {
            None => {
                if let Some((s, _)) = layer.pulldown.take() { self.drop_pixmap(s); }
            }
            Some(pd) => {
                let stale = !reusable || layer.pulldown.as_ref().is_none_or(|(_, h)| *h != pd.h);
                if stale {
                    if let Some((s, _)) = layer.pulldown.take() { self.drop_pixmap(s); }
                    if pd.h > 0 && let Some(s) = self.new_pixmap(MENU_DROPDOWN_W, pd.h) {
                        write_rect(&s.pix, Rect::at(0, 0, MENU_DROPDOWN_W, pd.h), &pd.pixels);
                        damage.push((s.id, Area::new(0, 0, MENU_DROPDOWN_W, pd.h)));
                        layer.pulldown = Some((s, pd.h));
                    }
                }
                if let Some((s, h)) = &layer.pulldown && !p.pulldown.is_empty() {
                    items.push(Item { frame: p.hwnd, pixmap: s.id, x: p.pulldown.x0, y: p.pulldown.y0,
                                      parts: vec![Area::new(0, 0, MENU_DROPDOWN_W, *h)], rounded: p.rounded });
                }
            }
        }
        self.frames.insert(p.hwnd, layer);
    }

    /// Bring the pop-up menu's picture up to date; its item, if one is open.
    fn update_overlay(&mut self, scene: &Scene, damage: &mut Vec<(u32, Area)>) -> Option<Item> {
        let Some(o) = &scene.overlay else {
            if let Some((s, _)) = self.overlay.take() { self.drop_pixmap(s); }
            return None;
        };
        let (w, h) = (o.rect.width(), o.rect.height());
        if w <= 0 || h <= 0 || o.pixels.len() < w as usize * h as usize {
            return None;
        }
        let fresh = self.overlay.as_ref().is_some_and(|(s, g)| {
            *g == o.generation && s.pix.width() == w as u32 && s.pix.height() == h as u32
        });
        if !fresh {
            if let Some((s, _)) = self.overlay.take() { self.drop_pixmap(s); }
            let s = self.new_pixmap(w, h)?;
            write_rect(&s.pix, Rect::at(0, 0, w, h), o.pixels);
            damage.push((s.id, Area::new(0, 0, w, h)));
            self.overlay = Some((s, o.generation));
        }
        let (s, _) = self.overlay.as_ref()?;
        Some(Item { frame: 0, pixmap: s.id, x: o.rect.x0, y: o.rect.y0, parts: vec![Area::new(0, 0, w, h)],
                   rounded: crate::gui::theme::current().rounded_frames() })
    }
}

/// Write `pels` (rows of `dst`'s width) into `pix` at `dst`.
fn write_rect(pix: &SharedPixmap, dst: Rect, pels: &[u32]) {
    let w = dst.width() as usize;
    if w == 0 { return; }
    for (i, row) in pels.chunks(w).take(dst.height() as usize).enumerate() {
        pix.write_row(dst.x0 as i64, dst.y0 as i64 + i as i64, row);
    }
}

fn area(r: Rect) -> Area {
    Area::new(r.x0, r.y0, r.width(), r.height())
}

/// The bounding box of `parts`.
fn whole(parts: &[Area]) -> Area {
    parts.iter().fold(None::<Rect>, |acc, a| {
        let r = Rect::at(a.x, a.y, a.w, a.h);
        Some(acc.map_or(r, |b| b.union(&r)))
    }).map_or(Area::default(), area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_are_written_row_by_row() {
        let pix = SharedPixmap::create(4, 3).unwrap();
        write_rect(&pix, Rect::at(1, 1, 2, 2), &[1, 2, 3, 4]);
        let mut row = [0; 4];
        pix.read_row(0, 1, &mut row);
        assert_eq!(row, [0, 1, 2, 0]);
        pix.read_row(0, 2, &mut row);
        assert_eq!(row, [0, 3, 4, 0]);
    }

    #[test]
    fn the_bounding_box_covers_every_part() {
        assert_eq!(whole(&[Area::new(0, 0, 2, 2), Area::new(5, 1, 1, 4)]), Area::new(0, 0, 6, 5));
        assert_eq!(whole(&[]), Area::default());
    }
}
