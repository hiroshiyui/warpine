// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::AtomicBool;
use std::thread;


pub struct OS2Message {
    pub hwnd: u32,
    pub msg: u32,
    pub mp1: u32,
    pub mp2: u32,
    pub time: u32,
    pub x: i16,
    pub y: i16,
}

#[allow(non_camel_case_types)]
pub struct PM_MsgQueue {
    pub messages: VecDeque<OS2Message>,
    pub cond: Arc<Condvar>,
    pub lock: Arc<Mutex<bool>>,
}

pub struct WindowClass {
    pub name: String,
    pub pfn_wp: u32,
    pub style: u32,
}

pub struct OS2Window {
    pub handle: u32,
    pub class_name: String,
    pub pfn_wp: u32,
    pub parent: u32,
    pub hmq: u32,
    pub text: String,
    pub window_ulong: HashMap<i32, u32>,
    pub window_ushort: HashMap<i32, u16>,
    pub id: u32,
    pub children: Vec<u32>,
    pub x: i32,
    pub y: i32,
    pub cx: i32,
    pub cy: i32,
    pub visible: bool,
}

pub struct PresentationSpace {
    pub hps: u32,
    pub hwnd: u32,
    pub color: u32,
    pub current_pos: (i32, i32),
}

pub struct AccelEntry {
    pub flags: u16,
    pub key: u16,
    pub cmd: u16,
}

pub struct WindowManager {
    classes: HashMap<String, WindowClass>,
    windows: HashMap<u32, OS2Window>,
    pub ps_map: HashMap<u32, PresentationSpace>,
    pub msg_queues: HashMap<u32, Arc<Mutex<PM_MsgQueue>>>,
    pub frame_to_client: HashMap<u32, u32>,
    pub tid_to_hmq: HashMap<u32, u32>,
    pub gui_tx: Option<crate::gui::GUISender>,
    pub timers: HashMap<(u32, u32), (Arc<AtomicBool>, Option<thread::JoinHandle<()>>)>,
    pub clipboard: HashMap<u32, u32>,
    pub clipboard_open: bool,
    /// Currently captured window handle (0 = none).
    pub capture_hwnd: u32,
    /// Text-format clipboard content, kept in sync with the host SDL2 clipboard.
    pub clipboard_text: String,
    accel_tables: HashMap<u32, Vec<AccelEntry>>,
    window_accel: HashMap<u32, u32>, // hwnd → haccel
    next_hwnd: u32,
    next_hps: u32,
    next_hmq: u32,
    next_haccel: u32,
}

impl WindowManager {
    pub fn new() -> Self {
        WindowManager {
            classes: HashMap::new(),
            windows: HashMap::new(),
            ps_map: HashMap::new(),
            msg_queues: HashMap::new(),
            frame_to_client: HashMap::new(),
            tid_to_hmq: HashMap::new(),
            gui_tx: None,
            timers: HashMap::new(),
            clipboard: HashMap::new(),
            clipboard_open: false,
            capture_hwnd: 0,
            clipboard_text: String::new(),
            accel_tables: HashMap::new(),
            window_accel: HashMap::new(),
            next_hwnd: 0x1000,
            next_hps: 0x2000,
            next_hmq: 0x3000,
            next_haccel: 0x4000,
        }
    }
    pub fn register_class(&mut self, name: String, pfn_wp: u32, style: u32) {
        self.classes.insert(name.clone(), WindowClass { name, pfn_wp, style });
    }
    pub fn get_class(&self, name: &str) -> Option<&WindowClass> {
        self.classes.get(name)
    }
    pub fn create_window(&mut self, class_name: String, parent: u32, hmq: u32) -> u32 {
        let h = self.next_hwnd;
        let pfn_wp = self.classes.get(&class_name).map(|c| c.pfn_wp).unwrap_or(0);
        self.windows.insert(h, OS2Window {
            handle: h, class_name, pfn_wp, parent, hmq,
            text: String::new(),
            window_ulong: HashMap::new(),
            window_ushort: HashMap::new(),
            id: 0,
            children: Vec::new(),
            x: 0, y: 0, cx: 0, cy: 0,
            visible: false,
        });
        // Register as child of parent
        if parent != 0 {
            if let Some(parent_win) = self.windows.get_mut(&parent) {
                parent_win.children.push(h);
            }
        }
        self.next_hwnd += 1;
        h
    }
    pub fn get_window(&self, h: u32) -> Option<&OS2Window> {
        self.windows.get(&h)
    }
    pub fn get_window_mut(&mut self, h: u32) -> Option<&mut OS2Window> {
        self.windows.get_mut(&h)
    }
    pub fn get_ps_hwnd(&self, hps: u32) -> u32 {
        self.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0)
    }
    pub fn create_ps(&mut self, hwnd: u32) -> u32 {
        let h = self.next_hps;
        self.ps_map.insert(h, PresentationSpace { hps: h, hwnd, color: 0, current_pos: (0, 0) });
        self.next_hps += 1;
        h
    }
    pub fn create_mq(&mut self) -> u32 {
        let h = self.next_hmq;
        self.msg_queues.insert(h, Arc::new(Mutex::new(PM_MsgQueue {
            messages: VecDeque::new(),
            cond: Arc::new(Condvar::new()),
            lock: Arc::new(Mutex::new(false)),
        })));
        self.next_hmq += 1;
        h
    }
    pub fn get_mq(&self, h: u32) -> Option<Arc<Mutex<PM_MsgQueue>>> {
        self.msg_queues.get(&h).cloned()
    }
    pub fn find_hmq_for_hwnd(&self, hwnd: u32) -> Option<u32> {
        if let Some(win) = self.windows.get(&hwnd) {
            if win.hmq != 0 {
                return Some(win.hmq);
            }
        }
        // Search through tid_to_hmq for a match
        for (_tid, &hmq) in &self.tid_to_hmq {
            if self.msg_queues.contains_key(&hmq) {
                return Some(hmq);
            }
        }
        None
    }
    /// Reverse lookup: given a client hwnd, find the frame hwnd.
    /// Returns the client hwnd itself if no mapping exists.
    pub fn client_to_frame(&self, client_hwnd: u32) -> u32 {
        self.frame_to_client.iter()
            .find(|&(_, &client)| client == client_hwnd)
            .map(|(&frame, _)| frame)
            .unwrap_or(client_hwnd)
    }

    /// Stop all running timers and join their threads.
    pub fn stop_all_timers(&mut self) {
        for (_, (running, handle)) in self.timers.drain() {
            running.store(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = handle {
                let _ = h.join();
            }
        }
    }

    pub fn add_accel_table(&mut self, entries: Vec<AccelEntry>) -> u32 {
        let h = self.next_haccel;
        self.accel_tables.insert(h, entries);
        self.next_haccel += 1;
        h
    }

    pub fn set_window_accel(&mut self, hwnd: u32, haccel: u32) {
        if haccel == 0 {
            self.window_accel.remove(&hwnd);
        } else {
            self.window_accel.insert(hwnd, haccel);
        }
    }

    pub fn translate_accel(&self, hwnd: u32, key: u16, flags: u16) -> Option<u16> {
        let haccel = self.window_accel.get(&hwnd)?;
        let entries = self.accel_tables.get(haccel)?;
        for entry in entries {
            if entry.key == key && (entry.flags & flags) == entry.flags {
                return Some(entry.cmd);
            }
        }
        None
    }

    pub fn find_child_by_id(&self, parent: u32, id: u32) -> Option<u32> {
        if let Some(win) = self.windows.get(&parent) {
            for &child_hwnd in &win.children {
                if let Some(child) = self.windows.get(&child_hwnd) {
                    if child.id == id {
                        return Some(child_hwnd);
                    }
                }
            }
        }
        None
    }
}
