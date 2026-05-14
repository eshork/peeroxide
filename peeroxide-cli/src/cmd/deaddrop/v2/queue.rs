//! Shared, dedup'd, priority work queue for the v2 sender.
//!
//! A single dispatcher pulls `(ChunkId, PublishUnit, subscribers)` triples
//! out of the queue, acquires a permit from the shared `ConcurrencyState`,
//! and spawns a put. Triggers (initial publish, refresh tick, need-list
//! response) only *enqueue* — they never spawn put tasks themselves.
//!
//! Each trigger gets an [`Operation`] whose [`Operation::await_done`]
//! resolves when every chunk it asked for has been put — whether by this
//! trigger's enqueue or by an overlapping one that arrived first. That's
//! how a single physical put can satisfy a need-list response *and* a
//! concurrent refresh tick simultaneously.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use super::publish::PublishUnit;
use crate::cmd::deaddrop::progress::state::ProgressState;

/// Identifies one chunk in the tree. Stable across re-enqueues.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum ChunkId {
    /// Index into `BuiltTree::data_chunks`.
    Data(usize),
    /// Index into `BuiltTree::index_chunks`.
    Index(usize),
    /// The root index chunk.
    Root,
}

/// Priority lane. High drains before Normal.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum Lane {
    Normal = 0,
    High = 1,
}

/// One operation's hook on a chunk: progress state to advance + a remaining
/// counter to decrement + a notify to fire when the operation finishes.
#[derive(Clone)]
pub struct Subscriber {
    state: Arc<ProgressState>,
    remaining: Arc<AtomicUsize>,
    done: Arc<Notify>,
}

struct Entry {
    unit: PublishUnit,
    lane: Lane,
    subs: Vec<Subscriber>,
}

struct Inner {
    queued: HashMap<ChunkId, Entry>,
    high: VecDeque<ChunkId>,
    normal: VecDeque<ChunkId>,
    /// Chunks currently being put. Subscribers attached while a chunk is in
    /// flight are recorded here so they get fired on completion.
    inflight: HashMap<ChunkId, Vec<Subscriber>>,
}

pub struct WorkQueue {
    inner: Mutex<Inner>,
    have_work: Notify,
}

impl WorkQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                queued: HashMap::new(),
                high: VecDeque::new(),
                normal: VecDeque::new(),
                inflight: HashMap::new(),
            }),
            have_work: Notify::new(),
        })
    }

    /// Enqueue one chunk on behalf of `sub`. If the chunk is already queued
    /// or in flight, `sub` attaches to the existing entry instead of
    /// triggering a duplicate put. A lane upgrade (Normal → High) re-pushes
    /// the id onto the High deque; stale Normal-deque ids are skipped at
    /// pop time.
    pub(super) async fn enqueue(
        &self,
        id: ChunkId,
        unit: PublishUnit,
        lane: Lane,
        sub: Subscriber,
    ) {
        let mut inner = self.inner.lock().await;
        if let Some(subs) = inner.inflight.get_mut(&id) {
            subs.push(sub);
            return;
        }
        if let Some(entry) = inner.queued.get_mut(&id) {
            entry.subs.push(sub);
            if lane > entry.lane {
                entry.lane = lane;
                inner.high.push_back(id);
            }
            return;
        }
        inner.queued.insert(id, Entry { unit, lane, subs: vec![sub] });
        match lane {
            Lane::High => inner.high.push_back(id),
            Lane::Normal => inner.normal.push_back(id),
        }
        drop(inner);
        self.have_work.notify_one();
    }

    /// Pop the next chunk (High lane first). Awaits if the queue is empty.
    /// Returns the unit to publish and the subscribers that should be
    /// notified on completion.
    pub(super) async fn pop(&self) -> (ChunkId, PublishUnit, Vec<Subscriber>) {
        loop {
            {
                let mut inner = self.inner.lock().await;
                while let Some(id) = inner.high.pop_front() {
                    if let Some(entry) = inner.queued.remove(&id) {
                        inner.inflight.insert(id, entry.subs.clone());
                        return (id, entry.unit, entry.subs);
                    }
                }
                while let Some(id) = inner.normal.pop_front() {
                    if let Some(entry) = inner.queued.remove(&id) {
                        if entry.lane == Lane::High {
                            // Was upgraded after being pushed onto Normal;
                            // the High-deque copy will handle it. Re-insert
                            // and skip this stale entry.
                            inner.queued.insert(id, entry);
                            continue;
                        }
                        inner.inflight.insert(id, entry.subs.clone());
                        return (id, entry.unit, entry.subs);
                    }
                }
            }
            self.have_work.notified().await;
        }
    }

    /// Mark a put complete. Fires every subscriber that was registered
    /// either at pop time or while the chunk was in flight.
    pub async fn mark_done(&self, id: ChunkId, bytes: u64, is_data: bool) {
        let subs = {
            let mut inner = self.inner.lock().await;
            inner.inflight.remove(&id).unwrap_or_default()
        };
        for sub in subs {
            if is_data {
                sub.state.inc_data(bytes);
            } else {
                sub.state.inc_index();
            }
            if sub.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                sub.done.notify_waiters();
            }
        }
    }
}

/// Trigger-side handle that registers chunks of interest and awaits their
/// completion. Each trigger (initial, refresh, need-list) creates one of
/// these, enqueues its chunks, then awaits.
pub struct Operation {
    pub state: Arc<ProgressState>,
    remaining: Arc<AtomicUsize>,
    done: Arc<Notify>,
}

impl Operation {
    pub fn new(state: Arc<ProgressState>, chunk_count: usize) -> Self {
        Self {
            state,
            remaining: Arc::new(AtomicUsize::new(chunk_count)),
            done: Arc::new(Notify::new()),
        }
    }

    pub fn subscriber(&self) -> Subscriber {
        Subscriber {
            state: self.state.clone(),
            remaining: self.remaining.clone(),
            done: self.done.clone(),
        }
    }

    /// Block until every chunk this operation subscribed to has been
    /// marked done. Fast-path returns immediately if `chunk_count == 0`
    /// or all subscriptions completed before the await.
    pub async fn await_done(&self) {
        if self.remaining.load(Ordering::Acquire) == 0 {
            return;
        }
        // `notify_waiters` fires once when `remaining` reaches zero; check
        // again after registering to avoid the race where it fires between
        // the load above and the registration below.
        let notified = self.done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.remaining.load(Ordering::Acquire) == 0 {
            return;
        }
        notified.await;
    }
}
