// SPDX-License-Identifier: GPL-3.0-only
//
// API call ring buffer.
//
// Records the last `CAPACITY` OS/2 API calls in a bounded circular buffer
// stored in `SharedState`.  Every dispatch in `api_dispatch.rs` pushes one
// record unconditionally (not gated on DEBUG level), so crash dumps include
// the API call history even in release / info-only builds.
//
// The buffer is intentionally simple: a `VecDeque<ApiCallRecord>` capped at
// `CAPACITY` entries.  Oldest records are dropped when the buffer is full.

use std::collections::VecDeque;

/// Maximum number of API call records retained in the ring buffer.
pub const CAPACITY: usize = 256;

/// One captured OS/2 API call.
#[derive(Clone, Debug)]
pub struct ApiCallRecord {
    /// Warpine flat ordinal (encodes subsystem + local ordinal).
    pub ordinal:  u32,
    /// DLL name (e.g. `"DOSCALLS"`, `"PMWIN"`).
    pub module:   &'static str,
    /// Human-readable API name (e.g. `"DosWrite"`).
    pub name:     &'static str,
    /// Strace-style formatted call string produced by `api_trace::format_call`.
    pub call_str: String,
    /// Return value placed in EAX (0 for `ApiResult::Callback`).
    pub ret_val:  u32,
    /// Monotonically increasing call sequence number (wraps at u64::MAX).
    pub seq:      u64,
}

/// Bounded ring buffer of [`ApiCallRecord`]s.
pub struct ApiRingBuffer {
    records:  VecDeque<ApiCallRecord>,
    next_seq: u64,
}

impl ApiRingBuffer {
    pub fn new() -> Self {
        ApiRingBuffer {
            records:  VecDeque::with_capacity(CAPACITY),
            next_seq: 0,
        }
    }

    /// Push a new record.  Drops the oldest entry when the buffer is full.
    pub fn push(&mut self, mut record: ApiCallRecord) {
        record.seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        if self.records.len() >= CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// Iterate records in chronological order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &ApiCallRecord> {
        self.records.iter()
    }

    /// Number of records currently in the buffer.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when no records have been captured yet.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Snapshot the current contents as a `Vec` (oldest-first).
    pub fn snapshot(&self) -> Vec<ApiCallRecord> {
        self.records.iter().cloned().collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(ordinal: u32, name: &'static str, ret: u32) -> ApiCallRecord {
        ApiCallRecord {
            ordinal,
            module: "DOSCALLS",
            name,
            call_str: format!("{}()", name),
            ret_val: ret,
            seq: 0, // overwritten by push()
        }
    }

    #[test]
    fn test_push_and_len() {
        let mut ring = ApiRingBuffer::new();
        assert!(ring.is_empty());
        ring.push(make_record(282, "DosWrite", 0));
        ring.push(make_record(281, "DosRead", 0));
        assert_eq!(ring.len(), 2);
    }

    #[test]
    fn test_seq_numbers_are_monotonic() {
        let mut ring = ApiRingBuffer::new();
        for i in 0..5 {
            ring.push(make_record(i, "Dos?", 0));
        }
        let seqs: Vec<u64> = ring.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_oldest_evicted_when_full() {
        let mut ring = ApiRingBuffer::new();
        for i in 0..=(CAPACITY as u32) {
            ring.push(make_record(i, "Dos?", 0));
        }
        assert_eq!(ring.len(), CAPACITY);
        // seq 0 (ordinal 0) should have been evicted; first entry is now seq 1
        assert_eq!(ring.iter().next().unwrap().seq, 1);
        assert_eq!(ring.iter().next().unwrap().ordinal, 1);
    }

    #[test]
    fn test_snapshot_preserves_order() {
        let mut ring = ApiRingBuffer::new();
        ring.push(make_record(282, "DosWrite", 0));
        ring.push(make_record(281, "DosRead",  0));
        ring.push(make_record(257, "DosClose", 0));
        let snap = ring.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].name, "DosWrite");
        assert_eq!(snap[1].name, "DosRead");
        assert_eq!(snap[2].name, "DosClose");
    }

    #[test]
    fn test_ret_val_preserved() {
        let mut ring = ApiRingBuffer::new();
        ring.push(make_record(282, "DosWrite", 5));
        assert_eq!(ring.iter().next().unwrap().ret_val, 5);
    }

    #[test]
    fn test_seq_wraps_gracefully() {
        let mut ring = ApiRingBuffer::new();
        ring.next_seq = u64::MAX;
        ring.push(make_record(1, "A", 0));
        ring.push(make_record(2, "B", 0));
        let seqs: Vec<u64> = ring.iter().map(|r| r.seq).collect();
        assert_eq!(seqs[0], u64::MAX);
        assert_eq!(seqs[1], 0); // wrapped
    }

    #[test]
    fn test_call_str_stored() {
        let mut ring = ApiRingBuffer::new();
        let mut rec = make_record(282, "DosWrite", 0);
        rec.call_str = "DosWrite(hFile=1, pBuf=0x1000, cbBuf=0x5, pcbActual=0x2000)".to_string();
        ring.push(rec);
        assert!(ring.iter().next().unwrap().call_str.contains("hFile=1"));
    }
}
