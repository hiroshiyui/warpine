// SPDX-License-Identifier: GPL-3.0-only

//! The desktop server's model of the session's desktop (ADR-0052): which
//! client shows which pictures, in which order, which frame is active, and
//! which client an input event goes to. Pure logic, without SDL or sockets,
//! so the rules can be tested on their own.
//!
//! - **Stacking.** Frames of all clients are stacked in one order, back to
//!   front. A client's own frames keep the order it reports (its Z-order);
//!   a frame it names as raised goes to the top of the whole desktop, one it
//!   names as lowered to the bottom, and a new frame appears on top.
//! - **Activation.** One frame of the session is active. A client reports
//!   its own active frame with every update and acknowledgement; when that
//!   report *changes* to a frame, the frame becomes the session's active
//!   one and the client that held activation before is told to give it up
//!   (`Deactivate`). Reports that do not change are ignored, so a client
//!   that has not yet seen its `Deactivate` cannot take activation back.
//! - **Input targets.** An open pop-up menu's client gets all input. A
//!   client holding the pointer (a pressed button, or a capture it asked
//!   for) gets the pointer; otherwise the pointer goes to the client whose
//!   picture is topmost under it. Keys go to the client of the active frame.

use std::collections::HashMap;

use super::proto::{Area, Item, Update};

/// A client, by its slot.
pub type ClientId = u32;

/// One frame in the desktop stacking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stacked {
    client: ClientId,
    frame: u32,
}

/// What a change to the model asks the server to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Tell client `.0` that its frame `.1` is no longer active, frame `.2` is.
    pub deactivate: Option<(ClientId, u32, u32)>,
    /// Desktop areas to redraw.
    pub damage: Vec<Area>,
}

/// One picture as drawn, for damage tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Drawn {
    client: ClientId,
    item: Item,
    /// Its frame's place in the stacking order.
    depth: usize,
}

/// The session's desktop.
#[derive(Debug, Default)]
pub struct DesktopModel {
    stack: Vec<Stacked>,
    items: HashMap<ClientId, Vec<Item>>,
    overlay: Option<(ClientId, Item)>,
    active: Option<(ClientId, u32)>,
    /// Each client's last reported active frame.
    reported: HashMap<ClientId, u32>,
    /// Client holding the pointer: buttons held, or a capture.
    held_by: Option<ClientId>,
    buttons: u32,
    capture: Option<ClientId>,
}

/// Desktop extent of an item: its painted parts, moved to its place.
pub fn item_extent(it: &Item) -> Vec<Area> {
    it.parts.iter().filter(|p| !p.is_empty())
        .map(|p| Area::new(it.x.saturating_add(p.x), it.y.saturating_add(p.y), p.w, p.h))
        .collect()
}

impl DesktopModel {
    pub fn new() -> DesktopModel {
        DesktopModel::default()
    }

    /// The pictures to draw, back to front, with their clients: each
    /// frame's items in its client's order, then the pop-up menu.
    pub fn draw_list(&self) -> Vec<(ClientId, &Item)> {
        let mut out = Vec::new();
        for s in &self.stack {
            if let Some(items) = self.items.get(&s.client) {
                out.extend(items.iter().filter(|it| it.frame == s.frame).map(|it| (s.client, it)));
            }
        }
        if let Some((c, it)) = &self.overlay {
            out.push((*c, it));
        }
        out
    }

    fn drawn(&self) -> Vec<Drawn> {
        let depth: HashMap<(ClientId, u32), usize> =
            self.stack.iter().enumerate().map(|(i, s)| ((s.client, s.frame), i)).collect();
        let mut out: Vec<Drawn> = self.draw_list().into_iter().map(|(client, item)| {
            let d = depth.get(&(client, item.frame)).copied().unwrap_or(usize::MAX);
            Drawn { client, item: item.clone(), depth: d }
        }).collect();
        // The overlay's depth is "on top".
        if let (Some(last), Some(_)) = (out.last_mut(), &self.overlay) {
            last.depth = usize::MAX;
        }
        out
    }

    /// Apply client `c`'s update. `owns(frame)` tells whether a frame
    /// handle belongs to `c` (its slot prefix); items for other frames are
    /// dropped.
    pub fn update(&mut self, c: ClientId, u: &Update, owns: impl Fn(u32) -> bool) -> Effects {
        let before = self.drawn();
        let items: Vec<Item> = u.items.iter().filter(|it| owns(it.frame)).cloned().collect();
        // The client's frames, in its order (first appearance).
        let mut order: Vec<u32> = Vec::new();
        for it in &items {
            if !order.contains(&it.frame) { order.push(it.frame); }
        }
        // Frames gone; new frames on top.
        self.stack.retain(|s| s.client != c || order.contains(&s.frame));
        for &f in &order {
            if !self.stack.iter().any(|s| s.client == c && s.frame == f) {
                self.stack.push(Stacked { client: c, frame: f });
            }
        }
        // Raised frames go to the top of the desktop, lowered ones to the
        // bottom; first, so that the client's order then fills the slots
        // its frames hold around them.
        for &f in u.raised.iter().filter(|&&f| owns(f)) {
            if let Some(i) = self.stack.iter().position(|s| s.client == c && s.frame == f) {
                let s = self.stack.remove(i);
                self.stack.push(s);
            }
        }
        for &f in u.lowered.iter().filter(|&&f| owns(f)) {
            if let Some(i) = self.stack.iter().position(|s| s.client == c && s.frame == f) {
                let s = self.stack.remove(i);
                self.stack.insert(0, s);
            }
        }
        // The client's frames keep its order, in the slots they hold.
        let slots: Vec<usize> = self.stack.iter().enumerate().filter(|(_, s)| s.client == c).map(|(i, _)| i).collect();
        for (slot, &f) in slots.iter().zip(order.iter()) {
            self.stack[*slot] = Stacked { client: c, frame: f };
        }
        self.items.insert(c, items);
        self.overlay = match (self.overlay.take(), &u.overlay) {
            (_, Some(o)) if owns(o.frame) || o.frame == 0 => Some((c, o.clone())),
            (Some((oc, _)), _) if oc == c => None,
            (other, _) => other,
        };
        let mut fx = self.report_active(c, u.active, &owns);
        fx.damage = changed_extents(&before, &self.drawn());
        // Damaged pels of the client's pixmaps, wherever they are shown.
        for (pixmap, area) in &u.damage {
            for (_, it) in self.draw_list().into_iter().filter(|(cl, it)| *cl == c && it.pixmap == *pixmap) {
                fx.damage.push(Area::new(it.x.saturating_add(area.x), it.y.saturating_add(area.y), area.w, area.h));
            }
        }
        fx
    }

    /// Client `c` acknowledged input with `active` as its active frame.
    pub fn acknowledged(&mut self, c: ClientId, active: u32, owns: impl Fn(u32) -> bool) -> Effects {
        self.report_active(c, active, &owns)
    }

    fn report_active(&mut self, c: ClientId, active: u32, owns: &impl Fn(u32) -> bool) -> Effects {
        let mut fx = Effects::default();
        let active = if owns(active) { active } else { 0 };
        let previous = self.reported.insert(c, active);
        if previous == Some(active) {
            return fx;
        }
        if active != 0 {
            if let Some((pc, pf)) = self.active
                && pc != c {
                fx.deactivate = Some((pc, pf, active));
            }
            self.active = Some((c, active));
        } else if self.active.is_some_and(|(ac, _)| ac == c) {
            self.active = None;
        }
        fx
    }

    /// Client `c` is gone: its pictures go, and what it held.
    pub fn remove_client(&mut self, c: ClientId) -> Effects {
        let before = self.drawn();
        self.stack.retain(|s| s.client != c);
        self.items.remove(&c);
        self.reported.remove(&c);
        if self.overlay.as_ref().is_some_and(|(oc, _)| *oc == c) { self.overlay = None; }
        if self.active.is_some_and(|(ac, _)| ac == c) { self.active = None; }
        if self.held_by == Some(c) { self.held_by = None; self.buttons = 0; }
        if self.capture == Some(c) { self.capture = None; }
        Effects { deactivate: None, damage: changed_extents(&before, &self.drawn()) }
    }

    /// The client whose picture is topmost at desktop point `(x, y)`.
    pub fn client_at(&self, x: i32, y: i32) -> Option<ClientId> {
        self.draw_list().into_iter().rev()
            .find(|(_, it)| item_extent(it).iter().any(|a| a.contains(x, y)))
            .map(|(c, _)| c)
    }

    /// Where a pointer event at `(x, y)` goes. A press (`press` = the SDL
    /// button number) makes that client hold the pointer until every
    /// button is up again (`release`).
    pub fn route_pointer(&mut self, x: i32, y: i32, press: Option<u8>, release: Option<u8>) -> Option<ClientId> {
        let target = self.overlay.as_ref().map(|(c, _)| *c)
            .or(self.capture)
            .or(self.held_by)
            .or_else(|| self.client_at(x, y));
        if let Some(b) = press {
            self.buttons |= 1 << (b & 31);
            if self.held_by.is_none() { self.held_by = target; }
        }
        if let Some(b) = release {
            self.buttons &= !(1 << (b & 31));
            if self.buttons == 0 { self.held_by = None; }
        }
        target
    }

    /// Where a key or typed text goes.
    pub fn key_target(&self) -> Option<ClientId> {
        self.overlay.as_ref().map(|(c, _)| *c).or(self.active.map(|(c, _)| c))
    }

    /// Client `c` asks to keep (or let go of) the pointer.
    pub fn set_capture(&mut self, c: ClientId, on: bool) {
        if on { self.capture = Some(c); } else if self.capture == Some(c) { self.capture = None; }
    }

    /// The session's active frame.
    pub fn active(&self) -> Option<(ClientId, u32)> {
        self.active
    }

    /// Clients with pictures on the desktop.
    pub fn clients(&self) -> impl Iterator<Item = &ClientId> {
        self.items.keys()
    }
}

/// Desktop areas covered by pictures that appeared, went, moved, changed or
/// changed depth between `before` and `after`.
fn changed_extents(before: &[Drawn], after: &[Drawn]) -> Vec<Area> {
    let mut out = Vec::new();
    for d in before.iter().filter(|d| !after.contains(d)) {
        out.extend(item_extent(&d.item));
    }
    for d in after.iter().filter(|d| !before.contains(d)) {
        out.extend(item_extent(&d.item));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(frame: u32, x: i32) -> Item {
        Item { frame, pixmap: frame, x, y: 0, parts: vec![Area::new(0, 0, 100, 100)] }
    }

    fn upd(active: u32, raised: &[u32], items: Vec<Item>) -> Update {
        Update { active, raised: raised.to_vec(), lowered: vec![], items, damage: vec![], overlay: None }
    }

    /// Client c owns frames with c in their high bits, as slots do.
    fn owner(c: ClientId) -> impl Fn(u32) -> bool {
        move |f| f >> 20 == c
    }

    fn order(m: &DesktopModel) -> Vec<u32> {
        m.draw_list().iter().map(|(_, it)| it.frame).collect()
    }

    const A1: u32 = 1 << 20 | 1;
    const A2: u32 = 1 << 20 | 2;
    const B1: u32 = 2 << 20 | 1;

    #[test]
    fn frames_of_clients_interleave_and_raised_frames_go_on_top() {
        let mut m = DesktopModel::new();
        m.update(1, &upd(A1, &[], vec![item(A1, 0), item(A2, 50)]), owner(1));
        m.update(2, &upd(B1, &[], vec![item(B1, 20)]), owner(2));
        assert_eq!(order(&m), vec![A1, A2, B1], "a new frame appears on top");
        m.update(1, &upd(A1, &[A1], vec![item(A2, 50), item(A1, 0)]), owner(1));
        assert_eq!(order(&m), vec![A2, B1, A1]);
        // A client reordering its frames keeps the slots they hold.
        m.update(1, &upd(A1, &[], vec![item(A1, 0), item(A2, 50)]), owner(1));
        assert_eq!(order(&m), vec![A1, B1, A2]);
        // A lowered frame goes below every other client's.
        let mut u = upd(A1, &[], vec![item(A2, 50), item(A1, 0)]);
        u.lowered = vec![A2, B1];
        m.update(1, &u, owner(1));
        assert_eq!(order(&m), vec![A2, A1, B1], "B1 is not client 1's to lower");
        m.update(1, &upd(A1, &[], vec![item(A1, 0), item(A2, 50)]), owner(1));
        // A frame that is gone leaves the stack.
        m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1));
        assert_eq!(order(&m), vec![A1, B1]);
    }

    #[test]
    fn a_client_cannot_show_or_raise_another_clients_frames() {
        let mut m = DesktopModel::new();
        m.update(2, &upd(B1, &[], vec![item(B1, 0)]), owner(2));
        m.update(1, &upd(B1, &[B1], vec![item(A1, 0), item(B1, 0)]), owner(1));
        assert_eq!(m.draw_list().iter().map(|(c, it)| (*c, it.frame)).collect::<Vec<_>>(),
                   vec![(2, B1), (1, A1)]);
        assert_eq!(m.active(), Some((2, B1)), "B1 named by client 1 is no report of client 1");
    }

    #[test]
    fn activation_moves_on_a_changed_report_only() {
        let mut m = DesktopModel::new();
        assert_eq!(m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1)).deactivate, None);
        assert_eq!(m.active(), Some((1, A1)));
        let fx = m.update(2, &upd(B1, &[], vec![item(B1, 200)]), owner(2));
        assert_eq!(fx.deactivate, Some((1, A1, B1)));
        assert_eq!(m.key_target(), Some(2));
        // Client 1 still reports A1 before it has seen Deactivate: no change.
        assert_eq!(m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1)).deactivate, None);
        assert_eq!(m.active(), Some((2, B1)));
        // Then 0; later a click activates A1 again (a changed report).
        m.acknowledged(1, 0, owner(1));
        assert_eq!(m.acknowledged(1, A1, owner(1)).deactivate, Some((2, B1, A1)));
        assert_eq!(m.key_target(), Some(1));
        // The active client reporting no active frame leaves none.
        m.acknowledged(1, 0, owner(1));
        assert_eq!(m.active(), None);
    }

    #[test]
    fn the_pointer_goes_to_the_topmost_picture_or_the_client_holding_it() {
        let mut m = DesktopModel::new();
        m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1));
        m.update(2, &upd(B1, &[], vec![item(B1, 50)]), owner(2));
        assert_eq!(m.route_pointer(10, 10, None, None), Some(1));
        assert_eq!(m.route_pointer(60, 10, None, None), Some(2), "B1 lies over A1 there");
        assert_eq!(m.route_pointer(500, 10, None, None), None);
        // A press on A holds the pointer for A until the button is up.
        assert_eq!(m.route_pointer(10, 10, Some(1), None), Some(1));
        assert_eq!(m.route_pointer(60, 10, None, None), Some(1));
        assert_eq!(m.route_pointer(60, 10, None, Some(1)), Some(1));
        assert_eq!(m.route_pointer(60, 10, None, None), Some(2));
        // A capture keeps it without a button.
        m.set_capture(1, true);
        assert_eq!(m.route_pointer(60, 10, None, None), Some(1));
        m.set_capture(1, false);
        assert_eq!(m.route_pointer(60, 10, None, None), Some(2));
    }

    #[test]
    fn a_popup_menu_takes_all_input() {
        let mut m = DesktopModel::new();
        m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1));
        let mut u = upd(B1, &[], vec![item(B1, 50)]);
        u.overlay = Some(Item { frame: 0, pixmap: 9, x: 300, y: 300, parts: vec![Area::new(0, 0, 10, 10)] });
        m.update(2, &u, owner(2));
        assert_eq!(m.route_pointer(10, 10, None, None), Some(2));
        m.acknowledged(1, 0, owner(1));
        assert_eq!(m.key_target(), Some(2));
        assert_eq!(order(&m).last(), Some(&0), "drawn last");
        m.update(2, &upd(B1, &[], vec![item(B1, 50)]), owner(2));
        assert_eq!(m.route_pointer(10, 10, None, None), Some(1));
    }

    #[test]
    fn damage_covers_what_moved_appeared_or_went() {
        let mut m = DesktopModel::new();
        let fx = m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1));
        assert_eq!(fx.damage, vec![Area::new(0, 0, 100, 100)]);
        // Unchanged: nothing.
        assert!(m.update(1, &upd(A1, &[], vec![item(A1, 0)]), owner(1)).damage.is_empty());
        // Moved: old and new place.
        let fx = m.update(1, &upd(A1, &[], vec![item(A1, 10)]), owner(1));
        assert_eq!(fx.damage, vec![Area::new(0, 0, 100, 100), Area::new(10, 0, 100, 100)]);
        // Pixmap damage shows where the pixmap is shown.
        let mut u = upd(A1, &[], vec![item(A1, 10)]);
        u.damage = vec![(A1, Area::new(1, 2, 3, 4))];
        assert_eq!(m.update(1, &u, owner(1)).damage, vec![Area::new(11, 2, 3, 4)]);
        // A client that goes takes its pictures along.
        assert_eq!(m.remove_client(1).damage, vec![Area::new(10, 0, 100, 100)]);
        assert!(m.draw_list().is_empty());
    }
}
