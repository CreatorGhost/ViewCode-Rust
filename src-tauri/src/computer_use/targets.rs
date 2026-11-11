//! Per-thread window ids, element refs and screenshot ids. The agent only ever sees these
//! small integers; the helper's handles stay on this side.
//!
//! The numbers are monotonic and never reused for the life of the app process, even after
//! the thread's session stops (`reset` keeps the counters), so a number from an earlier
//! session cannot land on a different target. Each run starts its numbers at a random
//! base, so a number an agent quotes from before a restart (resumed sessions keep their
//! history) reads as unknown instead of landing on whatever this run minted under it.
//!
//! What goes stale: a newer observation of a window retires that window's older refs,
//! only a window's newest screenshot accepts coordinates, and a window missing from a
//! listing takes its refs and shot with it. The table keeps what the agent was shown
//! (role, label, the bounds a shot covered) so the helper can re-read the live control or
//! window at dispatch and refuse when they differ.

use super::protocol::{DriverElement, DriverWindow, ElementIdentity, Rect};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowRecord {
    pub(crate) id: u64,
    pub(crate) handle: String,
    pub(crate) app: String,
    pub(crate) app_identifier: Option<String>,
    pub(crate) pid: u32,
    pub(crate) title: String,
    pub(crate) focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefRecord {
    pub(crate) reference: u64,
    pub(crate) window_id: u64,
    pub(crate) observation: u64,
    pub(crate) handle: String,
    pub(crate) role: String,
    pub(crate) label: String,
}

impl RefRecord {
    /// The role and label the agent was shown, which the helper compares with the live control.
    pub(crate) fn identity(&self) -> ElementIdentity {
        ElementIdentity { role: self.role.clone(), label: self.label.clone() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShotRecord {
    pub(crate) shot: u64,
    pub(crate) window_id: u64,
    /// The helper's window handle the image was taken of.
    pub(crate) handle: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Logical screen rect the image covers.
    pub(crate) bounds: Rect,
    pub(crate) max_size: u32,
}

/// What a new shot is recorded from; the table adds the ids.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NewShot {
    pub(crate) handle: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bounds: Rect,
    pub(crate) max_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShotRetired {
    /// Not the window's newest screenshot any more (or retired after the window moved).
    NewerShot,
    WindowClosed,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ShotLookup<'a> {
    Found { shot: &'a ShotRecord, window: &'a WindowRecord },
    /// Never minted for this thread in this run.
    Unknown,
    Retired(ShotRetired),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefRetired {
    /// A newer observation of the window, or the per-thread cap, retired it.
    NewerObservation,
    WindowClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefLookup<'a> {
    Found { reference: &'a RefRecord, window: &'a WindowRecord },
    /// Never minted for this thread in this run.
    Unknown,
    Retired(RefRetired),
}

/// Refs kept per thread; the oldest observations go first.
const MAX_REFS_PER_THREAD: usize = 2000;

/// First number of this app run; ids before it belong to another run. Random so a number
/// quoted from before a restart is unlikely to fall inside this run's range.
fn run_id_base() -> u64 {
    static BASE: OnceLock<u64> = OnceLock::new();
    // uuid is the randomness already in the tree; 1..100_000 hundreds, as in ViewCode.
    *BASE.get_or_init(|| (uuid::Uuid::new_v4().as_u128() % 99_999 + 1) as u64 * 100)
}

#[derive(Debug)]
pub(crate) struct ThreadTargets {
    first_id: u64,
    next_window_id: u64,
    next_ref: u64,
    next_observation: u64,
    next_shot: u64,
    /// Only each window's newest shot is kept: older ones can never be used.
    latest_shot_by_window: BTreeMap<u64, ShotRecord>,
    windows: BTreeMap<u64, WindowRecord>,
    window_id_by_handle: HashMap<String, u64>,
    refs: HashMap<u64, RefRecord>,
    latest_observation_by_window: HashMap<u64, u64>,
    /// Observation order for the cap: oldest first.
    observations: VecDeque<Vec<u64>>,
}

impl Default for ThreadTargets {
    fn default() -> Self {
        Self::starting_at(run_id_base())
    }
}

impl ThreadTargets {
    pub(crate) fn starting_at(first_id: u64) -> Self {
        Self {
            first_id,
            next_window_id: first_id,
            next_ref: first_id,
            next_observation: 1,
            next_shot: first_id,
            latest_shot_by_window: BTreeMap::new(),
            windows: BTreeMap::new(),
            window_id_by_handle: HashMap::new(),
            refs: HashMap::new(),
            latest_observation_by_window: HashMap::new(),
            observations: VecDeque::new(),
        }
    }

    /// Reconciles a full helper listing. A handle keeps its id while it still names the
    /// same process and app; windows missing from the listing are closed and their ids
    /// stop resolving. Titles and focus are re-read here, so approvals name the live ones.
    pub(crate) fn record_windows(&mut self, listed: &[DriverWindow]) -> Vec<WindowRecord> {
        let mut seen = HashSet::new();
        let mut records = Vec::with_capacity(listed.len());
        for window in listed {
            let existing = self.window_id_by_handle.get(&window.handle).and_then(|id| self.windows.get(id));
            let id = match existing {
                Some(existing) if existing.pid == window.pid && existing.app == window.app => existing.id,
                _ => {
                    self.next_window_id += 1;
                    self.next_window_id - 1
                }
            };
            let record = WindowRecord { id, handle: window.handle.clone(), app: window.app.clone(), app_identifier: window.app_identifier.clone(), pid: window.pid, title: window.title.clone(), focused: window.focused };
            self.windows.insert(id, record.clone());
            self.window_id_by_handle.insert(window.handle.clone(), id);
            seen.insert(id);
            records.push(record);
        }
        let closed: Vec<u64> = self.windows.keys().copied().filter(|id| !seen.contains(id)).collect();
        for id in closed {
            self.close_window(id);
        }
        records
    }

    pub(crate) fn window(&self, id: u64) -> Option<&WindowRecord> {
        self.windows.get(&id)
    }

    pub(crate) fn close_window(&mut self, id: u64) {
        let Some(record) = self.windows.remove(&id) else { return };
        if self.window_id_by_handle.get(&record.handle) == Some(&id) {
            self.window_id_by_handle.remove(&record.handle);
        }
        self.latest_observation_by_window.remove(&id);
        self.latest_shot_by_window.remove(&id);
    }

    /// Mints a shot id; it becomes the window's only usable shot for coordinates.
    pub(crate) fn record_shot(&mut self, window_id: u64, shot: NewShot) -> ShotRecord {
        let record = ShotRecord { shot: self.next_shot, window_id, handle: shot.handle, width: shot.width, height: shot.height, bounds: shot.bounds, max_size: shot.max_size };
        self.next_shot += 1;
        self.latest_shot_by_window.insert(window_id, record.clone());
        record
    }

    /// The window's newest shot, if any (used to repeat its size after an action).
    pub(crate) fn latest_shot(&self, window_id: u64) -> Option<&ShotRecord> {
        self.latest_shot_by_window.get(&window_id)
    }

    /// The window moved since this shot: coordinates need a fresh one.
    pub(crate) fn retire_shot(&mut self, shot: u64) {
        self.latest_shot_by_window.retain(|_, record| record.shot != shot);
    }

    pub(crate) fn lookup_shot(&self, shot: u64) -> ShotLookup<'_> {
        if shot < self.first_id || shot >= self.next_shot {
            return ShotLookup::Unknown;
        }
        match self.latest_shot_by_window.values().find(|record| record.shot == shot) {
            Some(record) => match self.windows.get(&record.window_id) {
                Some(window) => ShotLookup::Found { shot: record, window },
                None => ShotLookup::Retired(ShotRetired::WindowClosed),
            },
            None => ShotLookup::Retired(ShotRetired::NewerShot),
        }
    }

    /// Mints refs for one observation, in order; older refs of that window stop resolving.
    pub(crate) fn record_observation(&mut self, window_id: u64, elements: &[DriverElement]) -> Vec<u64> {
        let observation = self.next_observation;
        self.next_observation += 1;
        self.latest_observation_by_window.insert(window_id, observation);
        let mut minted = Vec::with_capacity(elements.len());
        for element in elements {
            let reference = self.next_ref;
            self.next_ref += 1;
            self.refs.insert(reference, RefRecord { reference, window_id, observation, handle: element.handle.clone(), role: element.role.clone(), label: element.label.clone() });
            minted.push(reference);
        }
        self.observations.push_back(minted.clone());
        while self.refs.len() > MAX_REFS_PER_THREAD && self.observations.len() > 1 {
            for reference in self.observations.pop_front().unwrap_or_default() {
                self.refs.remove(&reference);
            }
        }
        minted
    }

    pub(crate) fn lookup_ref(&self, reference: u64) -> RefLookup<'_> {
        if reference < self.first_id || reference >= self.next_ref {
            return RefLookup::Unknown;
        }
        let Some(record) = self.refs.get(&reference) else { return RefLookup::Retired(RefRetired::NewerObservation) };
        let Some(window) = self.windows.get(&record.window_id) else { return RefLookup::Retired(RefRetired::WindowClosed) };
        if self.latest_observation_by_window.get(&record.window_id) != Some(&record.observation) {
            return RefLookup::Retired(RefRetired::NewerObservation);
        }
        RefLookup::Found { reference: record, window }
    }

    /// Forgets every window, ref and shot but keeps the counters, so numbers are never reused.
    pub(crate) fn reset(&mut self) {
        self.windows.clear();
        self.window_id_by_handle.clear();
        self.refs.clear();
        self.latest_observation_by_window.clear();
        self.latest_shot_by_window.clear();
        self.observations.clear();
    }
}
