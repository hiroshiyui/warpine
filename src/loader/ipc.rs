// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Condvar};
use super::mutex_ext::MutexExt;

pub struct EventSemaphore {
    pub posted: bool,
    _attr: u32,
    _name: Option<String>,
}

pub struct MutexSemaphore {
    pub owner_tid: Option<u32>,
    pub request_count: u32,
    _attr: u32,
    _name: Option<String>,
}

#[derive(Clone)]
pub enum SemHandle {
    Event(u32),
    Mutex(u32),
}

pub struct MuxWaitRecord {
    pub hsem: SemHandle,
    pub user: u32,
}

pub struct MuxWaitSemaphore {
    pub records: Vec<MuxWaitRecord>,
    pub wait_all: bool,
    _attr: u32,
    _name: Option<String>,
}

pub struct SemaphoreManager {
    event_sems: HashMap<u32, Arc<(Mutex<EventSemaphore>, Condvar)>>,
    mutex_sems: HashMap<u32, Arc<(Mutex<MutexSemaphore>, Condvar)>>,
    mux_sems: HashMap<u32, Arc<MuxWaitSemaphore>>,
    next_handle: u32,
}

impl SemaphoreManager {
    pub fn new() -> Self {
        SemaphoreManager {
            event_sems: HashMap::new(),
            mutex_sems: HashMap::new(),
            mux_sems: HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn create_event(&mut self, name: Option<String>, attr: u32, posted: bool) -> u32 {
        let h = self.next_handle;
        self.event_sems.insert(h, Arc::new((Mutex::new(EventSemaphore { posted, _attr: attr, _name: name }), Condvar::new())));
        self.next_handle += 1;
        h
    }

    pub fn get_event(&self, h: u32) -> Option<Arc<(Mutex<EventSemaphore>, Condvar)>> {
        self.event_sems.get(&h).cloned()
    }

    pub fn close_event(&mut self, h: u32) -> bool {
        self.event_sems.remove(&h).is_some()
    }

    pub fn create_mutex(&mut self, name: Option<String>, attr: u32, state: bool) -> u32 {
        let h = self.next_handle;
        let owner_tid = if state { Some(0) } else { None };
        let request_count = if state { 1 } else { 0 };
        self.mutex_sems.insert(h, Arc::new((Mutex::new(MutexSemaphore { owner_tid, request_count, _attr: attr, _name: name }), Condvar::new())));
        self.next_handle += 1;
        h
    }

    pub fn get_mutex(&self, h: u32) -> Option<Arc<(Mutex<MutexSemaphore>, Condvar)>> {
        self.mutex_sems.get(&h).cloned()
    }

    pub fn close_mutex(&mut self, h: u32) -> bool {
        self.mutex_sems.remove(&h).is_some()
    }

    pub fn create_mux(&mut self, name: Option<String>, attr: u32, records: Vec<MuxWaitRecord>, wait_all: bool) -> u32 {
        let h = self.next_handle;
        self.mux_sems.insert(h, Arc::new(MuxWaitSemaphore { records, wait_all, _attr: attr, _name: name }));
        self.next_handle += 1;
        h
    }

    pub fn get_mux(&self, h: u32) -> Option<Arc<MuxWaitSemaphore>> {
        self.mux_sems.get(&h).cloned()
    }

    pub fn close_mux(&mut self, h: u32) -> bool {
        self.mux_sems.remove(&h).is_some()
    }

    pub fn open_event_by_name(&self, name: &str) -> Option<u32> {
        for (&h, arc) in &self.event_sems {
            let sem = arc.0.lock_or_recover();
            if sem._name.as_deref() == Some(name) {
                return Some(h);
            }
        }
        None
    }

    pub fn open_mutex_by_name(&self, name: &str) -> Option<u32> {
        for (&h, arc) in &self.mutex_sems {
            let sem = arc.0.lock_or_recover();
            if sem._name.as_deref() == Some(name) {
                return Some(h);
            }
        }
        None
    }
}

pub struct QueueEntry {
    pub data: Vec<u8>,
    pub event: u32,
    pub priority: u32,
}

pub struct OS2Queue {
    pub name: String,
    pub items: VecDeque<QueueEntry>,
    pub attr: u32,
    pub cond: Arc<Condvar>,
    pub cond_lock: Arc<Mutex<bool>>,
}

pub struct QueueManager {
    queues: HashMap<u32, Arc<Mutex<OS2Queue>>>,
    next_handle: u32,
}

impl QueueManager {
    pub fn new() -> Self {
        QueueManager { queues: HashMap::new(), next_handle: 1 }
    }
    pub fn create(&mut self, name: String, attr: u32) -> u32 {
        let h = self.next_handle;
        self.queues.insert(h, Arc::new(Mutex::new(OS2Queue {
            name, items: VecDeque::new(), attr,
            cond: Arc::new(Condvar::new()), cond_lock: Arc::new(Mutex::new(false)),
        })));
        self.next_handle += 1;
        h
    }
    pub fn get(&self, h: u32) -> Option<Arc<Mutex<OS2Queue>>> {
        self.queues.get(&h).cloned()
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.queues.remove(&h).is_some()
    }
    pub fn find_by_name(&self, name: &str) -> Option<u32> {
        for (&h, q_arc) in &self.queues {
            if q_arc.lock_or_recover().name == name {
                return Some(h);
            }
        }
        None
    }
}
