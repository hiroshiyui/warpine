// SPDX-License-Identifier: GPL-3.0-only

//! The desktop server (ADR-0052): `warpine --desktop-server <session dir>`.
//!
//! One process per session owns the SDL desktop window. Every Warpine
//! process of the session that runs a PM program connects to it
//! (`desktop::client`), draws its frames — chrome included, as PM frame
//! controls belong to the program — into shared pixmaps and sends what it
//! shows; the server stacks the pictures of all clients (`desktop::model`)
//! and composites them (`PixmapCompositor`).
//!
//! Input is handed out one event at a time, in order: a pointer event to the
//! client whose picture is under the pointer (or that holds it), a key to
//! the client of the active frame. The next event waits until the client
//! has *read* the last one (`Ack`), so a key typed after a click goes to the
//! program that click activated, as PM's strictly ordered input requires.
//! A client that does not read its input within `INPUT_STALL` is marked
//! unresponsive and input goes past it until it answers again.
//!
//! The server runs no guest code and treats every client message as
//! hostile: packets are decoded strictly (`desktop::proto`), pixmaps must be
//! sealed memfds of the announced size, a client names only its own frames,
//! and anything malformed ends that client's connection only.
//!
//! Programs make calls on each other's windows through the server (phase
//! B): `Call` names a window, the server forwards it to the program the
//! window's slot names and passes the answer back (`desktop::calls`). The
//! server tells every program which frame of the session is active
//! (`Active`).
//!
//! It ends when the desktop window is closed or a GUI script says `quit`
//! (every client is told to end first), when no client connects within
//! `FIRST_CLIENT_TIMEOUT`, or `DESKTOP_LINGER` after its last client left.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{info, warn};
use sdl3::event::{Event, WindowEvent};

use crate::desktop::calls::{server_answer, CallRouter, Refusal, Route};
use crate::desktop::client::DesktopLock;
use crate::desktop::model::{DesktopModel, Effects};
use crate::desktop::services::Services;
use crate::desktop::pixmap::SharedPixmap;
use crate::desktop::proto::{Area, ClientMsg, Input, Pointer, ServerMsg, WinCall, WinReply, PROTOCOL_VERSION, SERVER_ADDRESS};
use crate::desktop::transport::{Conn, Listener, SOCKET_NAME};
use crate::loader::pm_types::{handle_in_slot, MAX_HANDLE_SLOTS};
use crate::loader::MutexExt;

use super::damage::{Damage, Rect};
use super::desktop_events::{to_input, SYSTEM_POINTERS};
use super::gui_script::{self, GuiScript};
use super::message::GuiWake;
use super::pointer::{HostPointer, PointerImage, PointerShape};
use super::sdl_renderer::compositor::{PixmapCompositor, Placed};
use super::sdl_renderer::{driver_can_wait, is_host_input, is_wake_event, script_input_to_event, WakeEvent};

/// How long the server stays after its last client left (a parent that ends
/// right after starting a child must not take the desktop down).
pub const DESKTOP_LINGER: Duration = Duration::from_secs(2);
/// How long a new server waits for its first client.
pub const FIRST_CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a client may take to read an input event before input goes
/// past it (Warpine's choice, ADR-0052).
pub const INPUT_STALL: Duration = Duration::from_secs(5);
/// How long clients have to end after `Quit`.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a scripted `dump` waits for input to be read and pictures sent.
const DUMP_TIMEOUT: Duration = Duration::from_secs(3);
/// Most pixmaps one client may hold.
const MAX_CLIENT_PIXMAPS: usize = 1024;
/// Composites at most this often.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Longest wait without anything to do.
const IDLE_WAIT: Duration = Duration::from_millis(100);

/// What the connection threads tell the main thread.
enum Incoming {
    Connected { slot: u32, conn: Arc<Conn> },
    Message { slot: u32, msg: ClientMsg, fd: Option<OwnedFd> },
    Gone { slot: u32 },
}

struct Client {
    conn: Arc<Conn>,
    pixmaps: HashMap<u32, SharedPixmap>,
    /// Did not read an input event within `INPUT_STALL`.
    unresponsive: bool,
}

impl Client {
    fn send(&self, m: &ServerMsg) {
        if self.conn.send(&m.encode(), None).is_err() {
            self.conn.shutdown();
        }
    }
}

/// A scripted `dump` in progress.
struct Dump {
    path: PathBuf,
    deadline: Instant,
    /// The flush sent once input is drained: its token and the clients
    /// that have not answered.
    flush: Option<(u64, HashSet<u32>)>,
}

/// Run the desktop server for session directory `dir`; returns the exit code.
pub fn run(dir: &Path) -> i32 {
    match Server::start(dir) {
        Ok(mut s) => s.run(),
        Err(e) => {
            eprintln!("warpine: desktop server cannot start: {e}");
            1
        }
    }
}

struct Server {
    dir: PathBuf,
    listener: Arc<Listener>,
    rx: Receiver<Incoming>,
    slots: Arc<Mutex<BTreeSet<u32>>>,
    wake: Arc<GuiWake>,
    native_wait: bool,
    video: sdl3::VideoSubsystem,
    pump: sdl3::EventPump,
    mouse: sdl3::mouse::MouseUtil,
    pointer: HostPointer,
    comp: PixmapCompositor,
    clients: HashMap<u32, Client>,
    model: DesktopModel,
    /// Calls on other programs' windows in flight.
    calls: CallRouter,
    /// The session's switch list and system atom table (ADR-0056).
    services: Services,
    /// The active frame last announced to the clients.
    announced_active: u32,
    queue: VecDeque<Input>,
    /// The event a client has not read yet: token, client, when sent.
    outstanding: Option<(u64, u32, Instant)>,
    next_token: u64,
    damage: Vec<Area>,
    full: bool,
    last_compose: Instant,
    clipboard: String,
    script: Option<GuiScript>,
    /// Scripted input due while a dump waits.
    script_backlog: VecDeque<gui_script::Input>,
    dump: Option<Dump>,
    quitting: Option<Instant>,
    started: Instant,
    ever_connected: bool,
    last_gone: Option<Instant>,
}

impl Server {
    fn start(dir: &Path) -> std::io::Result<Server> {
        let listener = Arc::new(Listener::bind(dir)?);
        let sdl = sdl3::init().map_err(std::io::Error::other)?;
        let video = sdl.video().map_err(std::io::Error::other)?;
        let event = sdl.event().map_err(std::io::Error::other)?;
        let pump = sdl.event_pump().map_err(std::io::Error::other)?;
        let (w, h) = crate::loader::constants::desktop_size();
        let comp = PixmapCompositor::new(&video, w, h);
        video.text_input().start(comp.window());
        let native_wait = driver_can_wait(video.current_video_driver())
            && event.register_custom_event::<WakeEvent>().is_ok();
        let wake = Arc::new(GuiWake::default());
        if native_wait {
            let sender = event.event_sender();
            wake.set_hook(Box::new(move || { let _ = sender.push_custom_event(WakeEvent); }));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let slots = Arc::new(Mutex::new(BTreeSet::new()));
        spawn_acceptor(listener.clone(), tx, slots.clone(), wake.clone())?;
        let script = GuiScript::from_env();
        if script.is_some() {
            info!("[desktop] GUI script active: host mouse and keyboard input is ignored");
        }
        let now = Instant::now();
        Ok(Server {
            dir: dir.to_path_buf(), listener, rx, slots, wake, native_wait,
            mouse: sdl.mouse(), pointer: HostPointer::new(sdl.mouse()),
            video, pump, comp,
            clients: HashMap::new(), model: DesktopModel::new(), calls: CallRouter::new(), services: Services::new(), announced_active: 0,
            queue: VecDeque::new(), outstanding: None,
            next_token: 1, damage: Vec::new(), full: true,
            last_compose: now.checked_sub(FRAME_INTERVAL).unwrap_or(now),
            clipboard: String::new(), script, script_backlog: VecDeque::new(), dump: None, quitting: None,
            started: now, ever_connected: false, last_gone: None,
        })
    }

    fn run(&mut self) -> i32 {
        self.sync_clipboard();
        loop {
            self.wait();
            self.wake.rearm();
            let events: Vec<Event> = self.pump.poll_iter().collect();
            let scripted = self.script.is_some();
            for e in events {
                if is_wake_event(&e) || (scripted && is_host_input(&e)) { continue; }
                self.on_sdl(e);
            }
            while let Ok(m) = self.rx.try_recv() {
                self.on_incoming(m);
            }
            self.run_script();
            self.dispatch_input();
            self.check_stall();
            self.advance_dump();
            if self.last_compose.elapsed() >= FRAME_INTERVAL {
                self.compose();
            }
            if self.should_exit() && self.try_exit() {
                return 0;
            }
        }
    }

    /// Sleep until input, a client message, a scripted step, a composite or
    /// a deadline is due.
    fn wait(&mut self) {
        let now = Instant::now();
        let mut timeout = IDLE_WAIT;
        if self.full || !self.damage.is_empty() {
            timeout = timeout.min(FRAME_INTERVAL.saturating_sub(self.last_compose.elapsed()));
        }
        if let Some(next) = self.script.as_ref().and_then(GuiScript::next_due) {
            timeout = timeout.min(next.saturating_duration_since(now));
        }
        if !self.script_backlog.is_empty() || self.dump.is_some() {
            timeout = timeout.min(Duration::from_millis(5));
        }
        if timeout.is_zero() { return; }
        if self.native_wait {
            if let Some(e) = self.pump.wait_event_timeout(timeout.max(Duration::from_millis(1))) {
                self.pump_back(e);
            }
        } else {
            self.wake.wait(timeout);
        }
    }

    /// An event taken by the wait is handled with the rest of the pass.
    fn pump_back(&mut self, e: Event) {
        if is_wake_event(&e) || (self.script.is_some() && is_host_input(&e)) { return; }
        self.on_sdl(e);
    }

    fn on_sdl(&mut self, e: Event) {
        match e {
            Event::Quit { .. } => self.begin_quit(),
            Event::Window { window_id, .. } if window_id != self.comp.window().id() => {}
            Event::Window { win_event, .. } => match win_event {
                WindowEvent::CloseRequested => self.begin_quit(),
                WindowEvent::Resized(w, h) => {
                    let (w, h) = (w.max(1) as u32, h.max(1) as u32);
                    self.comp.resize(w, h);
                    self.full = true;
                    self.broadcast(&ServerMsg::Resized { width: w, height: h });
                }
                WindowEvent::Exposed => self.full = true,
                WindowEvent::FocusGained => self.sync_clipboard(),
                _ => {}
            },
            Event::RenderTargetsReset { .. } | Event::RenderDeviceReset { .. } => self.full = true,
            Event::ClipboardUpdate { .. } => self.sync_clipboard(),
            e => {
                if let Some(input) = to_input(&e) {
                    self.queue_input(input);
                }
            }
        }
    }

    /// Queue `input`; pointer moves waiting next to each other become one.
    fn queue_input(&mut self, input: Input) {
        if let (Some(Input::Motion { x, y, dx, dy, buttons }), Input::Motion { x: nx, y: ny, dx: ndx, dy: ndy, buttons: nb })
            = (self.queue.back_mut(), &input) {
            (*x, *y, *buttons) = (*nx, *ny, *nb);
            *dx = dx.saturating_add(*ndx);
            *dy = dy.saturating_add(*ndy);
            return;
        }
        self.queue.push_back(input);
    }

    fn run_script(&mut self) {
        if let Some(s) = self.script.as_mut() {
            self.script_backlog.extend(s.due());
        }
        while self.dump.is_none() {
            let Some(step) = self.script_backlog.pop_front() else { break };
            match step {
                gui_script::Input::Dump(path) => {
                    self.dump = Some(Dump { path, deadline: Instant::now() + DUMP_TIMEOUT, flush: None });
                }
                gui_script::Input::Quit => self.begin_quit(),
                step => {
                    if let Some(e) = script_input_to_event(&step) {
                        self.on_sdl(e);
                    }
                }
            }
        }
    }

    /// Hand out queued input while no event waits to be read.
    fn dispatch_input(&mut self) {
        while self.outstanding.is_none() {
            let Some(input) = self.queue.pop_front() else { return };
            let target = match &input {
                Input::Motion { x, y, .. } => self.model.route_pointer(*x, *y, None, None),
                Input::Button { down, button, x, y, .. } => self.model.route_pointer(
                    *x, *y, down.then_some(*button), (!down).then_some(*button)),
                Input::Key { .. } | Input::Text(_) => self.model.key_target(),
            };
            let Some(slot) = target else { continue };
            let Some(client) = self.clients.get(&slot) else { continue };
            let token = self.next_token;
            self.next_token += 1;
            client.send(&ServerMsg::Input { token, input });
            if !client.unresponsive {
                self.outstanding = Some((token, slot, Instant::now()));
            }
        }
    }

    fn check_stall(&mut self) {
        let late = self.calls.expire(Instant::now());
        self.refuse(late);
        if let Some((_, slot, since)) = self.outstanding
            && since.elapsed() >= INPUT_STALL {
            warn!("[desktop] client {slot} does not read its input: input goes past it");
            if let Some(c) = self.clients.get_mut(&slot) { c.unresponsive = true; }
            self.outstanding = None;
        }
    }

    /// A scripted dump: once the queued input has been read, ask every
    /// client for its current pictures, then capture the desktop.
    fn advance_dump(&mut self) {
        let Some(dump) = self.dump.as_mut() else { return };
        let late = Instant::now() >= dump.deadline;
        if dump.flush.is_none() {
            if (!self.queue.is_empty() || self.outstanding.is_some()) && !late {
                return;
            }
            let token = self.next_token;
            self.next_token += 1;
            let waiting: HashSet<u32> = self.clients.keys().copied().collect();
            for c in self.clients.values() {
                c.send(&ServerMsg::Flush { token });
            }
            dump.flush = Some((token, waiting));
            if !late { return; }
        }
        if dump.flush.as_ref().is_some_and(|(_, w)| !w.is_empty()) && !late {
            return;
        }
        let path = self.dump.take().map(|d| d.path).expect("dump in progress");
        self.full = true;
        self.compose();
        let (pixels, w, h) = self.comp.snapshot();
        if let Err(e) = gui_script::write_ppm(&path, &pixels, w, h) {
            eprintln!("warpine: cannot write {}: {e}", path.display());
        }
    }

    fn on_incoming(&mut self, m: Incoming) {
        match m {
            Incoming::Connected { slot, conn } => {
                let c = Client { conn, pixmaps: HashMap::new(), unresponsive: false };
                c.send(&ServerMsg::Clipboard(self.clipboard.clone()));
                // The desktop window may have been resized since Welcome.
                let (w, h) = self.comp.size();
                if (w, h) != crate::loader::constants::desktop_size() {
                    c.send(&ServerMsg::Resized { width: w, height: h });
                }
                if self.announced_active != 0 { c.send(&ServerMsg::Active { frame: self.announced_active }); }
                if self.quitting.is_some() { c.send(&ServerMsg::Quit); }
                self.clients.insert(slot, c);
                self.ever_connected = true;
                self.last_gone = None;
            }
            Incoming::Gone { slot } => self.remove_client(slot),
            Incoming::Message { slot, msg, fd } => {
                if !self.clients.contains_key(&slot) { return; }
                if let Err(why) = self.on_message(slot, msg, fd) {
                    warn!("[desktop] client {slot}: {why}; disconnecting it");
                    if let Some(c) = self.clients.get(&slot) { c.conn.shutdown(); }
                    self.remove_client(slot);
                }
            }
        }
    }

    fn on_message(&mut self, slot: u32, msg: ClientMsg, fd: Option<OwnedFd>) -> Result<(), String> {
        let owns = |f: u32| handle_in_slot(f, slot);
        match msg {
            ClientMsg::Hello { .. } => return Err("a second Hello".into()),
            ClientMsg::NewPixmap { id, width, height } => {
                let fd = fd.ok_or("a pixmap without its memfd")?;
                let pix = SharedPixmap::open(fd, width, height).map_err(|e| format!("bad pixmap: {e}"))?;
                let c = self.clients.get_mut(&slot).expect("checked");
                if c.pixmaps.len() >= MAX_CLIENT_PIXMAPS && !c.pixmaps.contains_key(&id) {
                    return Err("too many pixmaps".into());
                }
                c.pixmaps.insert(id, pix);
                return Ok(());
            }
            _ if fd.is_some() => return Err("an unexpected descriptor".into()),
            ClientMsg::DropPixmap { id } => {
                self.clients.get_mut(&slot).expect("checked").pixmaps.remove(&id);
            }
            ClientMsg::Update(u) => {
                let fx = self.model.update(slot, &u, owns);
                self.apply(fx);
            }
            ClientMsg::Ack { token, active } => {
                if let Some(c) = self.clients.get_mut(&slot) { c.unresponsive = false; }
                if self.outstanding.is_some_and(|(t, s, _)| t == token && s == slot) {
                    self.outstanding = None;
                }
                let fx = self.model.acknowledged(slot, active, owns);
                self.apply(fx);
            }
            ClientMsg::Clipboard(text) => {
                if let Err(e) = self.video.clipboard().set_clipboard_text(&text) {
                    warn!("[desktop] cannot set the host clipboard: {e}");
                }
                self.clipboard = text.clone();
                for (s, c) in &self.clients {
                    if *s != slot { c.send(&ServerMsg::Clipboard(text.clone())); }
                }
            }
            ClientMsg::Capture(on) => {
                self.model.set_capture(slot, on);
                self.mouse.capture(on);
            }
            ClientMsg::Pointer(p) => {
                let shape = match p {
                    Pointer::Hidden => None,
                    Pointer::System(i) => Some(PointerShape::System(
                        *SYSTEM_POINTERS.get(i as usize).ok_or("unknown system pointer")?)),
                    Pointer::Image { width, height, hot_x, hot_y, rgba } => Some(PointerShape::Image(
                        Arc::new(PointerImage { width, height, hot_x, hot_y, rgba }))),
                };
                self.pointer.apply(shape);
            }
            ClientMsg::WarpPointer { x, y } => {
                if self.script.is_none() {
                    self.mouse.warp_mouse_in_window(self.comp.window(), x as f32, y as f32);
                }
            }
            ClientMsg::TextArea { area, cursor } => {
                let rect = sdl3::rect::Rect::new(area.x, area.y, area.w.max(1) as u32, area.h.max(1) as u32);
                self.video.text_input().set_rect(self.comp.window(), rect, cursor);
            }
            ClientMsg::Call { id, hwnd, call } if hwnd == SERVER_ADDRESS => {
                // About the session: answered here (calls.rs).
                let reply = match call {
                    WinCall::Clients | WinCall::Stacking =>
                        server_answer(&call, self.clients.keys().copied(), self.model.stacking()),
                    _ => self.services.answer(slot, &call),
                };
                if let Some(c) = self.clients.get(&slot) {
                    c.send(&ServerMsg::Answered { id, reply });
                }
            }
            ClientMsg::Call { id, hwnd, call } => {
                let clients = &self.clients;
                let expires = call.expires();
                match self.calls.call(slot, id, hwnd, Instant::now(), expires, |c| clients.contains_key(&c)) {
                    Route::Forward { callee, id } => {
                        self.clients[&callee].send(&ServerMsg::Called { id, hwnd, call });
                    }
                    Route::Refuse => self.refuse(vec![(slot, id)]),
                }
            }
            ClientMsg::Answer { id, reply } => {
                // An answer that is not this client's to give (late, or for
                // another client's call) is dropped.
                if let Some((caller, id)) = self.calls.answer(slot, id)
                    && let Some(c) = self.clients.get(&caller) {
                    c.send(&ServerMsg::Answered { id, reply });
                }
            }
            ClientMsg::FlushDone { token } => {
                if let Some(Dump { flush: Some((t, waiting)), .. }) = self.dump.as_mut()
                    && *t == token {
                    waiting.remove(&slot);
                }
            }
        }
        Ok(())
    }

    fn apply(&mut self, fx: Effects) {
        if let Some((client, frame, new_active)) = fx.deactivate
            && let Some(c) = self.clients.get(&client) {
            c.send(&ServerMsg::Deactivate { frame, new_active });
        }
        self.damage.extend(fx.damage);
        let active = self.model.active().map_or(0, |(_, f)| f);
        if active != self.announced_active {
            self.announced_active = active;
            self.broadcast(&ServerMsg::Active { frame: active });
        }
    }

    /// Tell the callers of `calls` that they failed.
    fn refuse(&self, calls: Vec<Refusal>) {
        for (caller, id) in calls {
            if let Some(c) = self.clients.get(&caller) {
                c.send(&ServerMsg::Answered { id, reply: WinReply::Failed });
            }
        }
    }

    fn remove_client(&mut self, slot: u32) {
        if self.clients.remove(&slot).is_none() { return; }
        let fx = self.model.remove_client(slot);
        self.apply(fx);
        self.services.client_gone(slot);
        let refused = self.calls.client_gone(slot);
        self.refuse(refused);
        if self.outstanding.is_some_and(|(_, s, _)| s == slot) {
            self.outstanding = None;
        }
        if let Some(Dump { flush: Some((_, waiting)), .. }) = self.dump.as_mut() {
            waiting.remove(&slot);
        }
        self.slots.lock_or_recover().remove(&slot);
        if self.clients.is_empty() {
            self.last_gone = Some(Instant::now());
        }
    }

    fn broadcast(&self, m: &ServerMsg) {
        for c in self.clients.values() {
            c.send(m);
        }
    }

    fn begin_quit(&mut self) {
        if self.quitting.is_none() {
            self.quitting = Some(Instant::now());
            self.broadcast(&ServerMsg::Quit);
        }
    }

    fn sync_clipboard(&mut self) {
        if let Ok(text) = self.video.clipboard().clipboard_text()
            && text != self.clipboard {
            self.clipboard = text;
            self.broadcast(&ServerMsg::Clipboard(self.clipboard.clone()));
        }
    }

    fn compose(&mut self) {
        let (w, h) = self.comp.size();
        let mut damage = Damage::new(w as i32, h as i32);
        if std::mem::take(&mut self.full) {
            damage.add_all();
        }
        for a in self.damage.drain(..) {
            damage.add(Rect::at(a.x, a.y, a.w, a.h));
        }
        if damage.is_empty() { return; }
        let list = self.model.draw_list();
        let items: Vec<Placed> = list.iter().filter_map(|(slot, it)| {
            let pixmap = self.clients.get(slot)?.pixmaps.get(&it.pixmap)?;
            Some(Placed { pixmap, x: it.x, y: it.y, parts: &it.parts })
        }).collect();
        self.comp.compose(&items, &damage);
        self.last_compose = Instant::now();
    }

    fn should_exit(&self) -> bool {
        if let Some(q) = self.quitting {
            return self.clients.is_empty() || q.elapsed() >= QUIT_TIMEOUT;
        }
        if !self.ever_connected {
            return self.started.elapsed() >= FIRST_CLIENT_TIMEOUT;
        }
        self.clients.is_empty() && self.last_gone.is_some_and(|t| t.elapsed() >= DESKTOP_LINGER)
    }

    /// Stop listening and remove the socket, unless a client connected
    /// meanwhile. Under the desktop lock, which a starting client holds
    /// until its handshake is done, so none can be left connecting.
    fn try_exit(&mut self) -> bool {
        let lock = DesktopLock::take(&self.dir).ok();
        while let Ok(m) = self.rx.try_recv() {
            self.on_incoming(m);
        }
        if self.quitting.is_none() && !self.clients.is_empty() {
            return false;
        }
        self.listener.shutdown();
        let _ = std::fs::remove_file(self.dir.join(SOCKET_NAME));
        let _ = std::fs::remove_file(self.dir.join(crate::desktop::client::LOCK_NAME));
        drop(lock);
        // Empty once the session's last process left (ADR-0032).
        let _ = std::fs::remove_dir(&self.dir);
        true
    }
}

/// Accept connections: check the peer, do the handshake, give a slot and
/// read the client's messages on a thread of its own.
fn spawn_acceptor(listener: Arc<Listener>, tx: Sender<Incoming>, slots: Arc<Mutex<BTreeSet<u32>>>,
                  wake: Arc<GuiWake>) -> std::io::Result<()> {
    std::thread::Builder::new().name("desktop-accept".into()).spawn(move || {
        let me = own_uid();
        while let Ok(conn) = listener.accept() {
            if conn.peer_uid().ok() != Some(me) {
                warn!("[desktop] refused a connection from another user");
                continue;
            }
            let Some(slot) = handshake(&conn, &slots) else { continue };
            let conn = Arc::new(conn);
            if tx.send(Incoming::Connected { slot, conn: conn.clone() }).is_err() { return; }
            wake.wake();
            let (tx, wake) = (tx.clone(), wake.clone());
            let _ = std::thread::Builder::new().name(format!("desktop-client-{slot}")).spawn(move || {
                loop {
                    let msg = match conn.recv() {
                        Ok(Some((packet, fd))) => ClientMsg::decode(&packet).map(|m| (m, fd)).ok(),
                        _ => None,
                    };
                    let Some((msg, fd)) = msg else {
                        let _ = tx.send(Incoming::Gone { slot });
                        wake.wake();
                        return;
                    };
                    if tx.send(Incoming::Message { slot, msg, fd }).is_err() { return; }
                    wake.wake();
                }
            });
        }
    })?;
    Ok(())
}

/// Read `Hello`, give the lowest free slot and send `Welcome`.
fn handshake(conn: &Conn, slots: &Mutex<BTreeSet<u32>>) -> Option<u32> {
    let (packet, fd) = conn.recv().ok()??;
    if fd.is_some() { return None; }
    let ClientMsg::Hello { version } = ClientMsg::decode(&packet).ok()? else { return None };
    let (w, h) = crate::loader::constants::desktop_size();
    if version != PROTOCOL_VERSION {
        let _ = conn.send(&ServerMsg::Welcome { version: PROTOCOL_VERSION, slot: 0, width: w, height: h }.encode(), None);
        return None;
    }
    let slot = {
        let mut used = slots.lock_or_recover();
        let slot = (0..MAX_HANDLE_SLOTS).find(|s| !used.contains(s))?;
        used.insert(slot);
        slot
    };
    if conn.send(&ServerMsg::Welcome { version: PROTOCOL_VERSION, slot, width: w, height: h }.encode(), None).is_err() {
        slots.lock_or_recover().remove(&slot);
        return None;
    }
    Some(slot)
}

/// This process's user id, as the owner of its `/proc` entry.
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map_or(u32::MAX, |m| m.uid())
}
