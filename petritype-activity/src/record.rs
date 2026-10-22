//! The record: a net's activity over time, as the model's deltas in bins
//! of a fixed width. [`Recorder`] bins them as they come, [`Record`] holds
//! them with the grid they were taken against, and replays, coarsens,
//! saves and lays them out as a [`Tensor`]; [`Player`] scrubs one.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype_layout::LayoutNode;

use crate::delta::Delta;
use crate::grid::Grid;
use crate::model::Activity;
use crate::settings::ActivitySettings;

/// One bin of a record: what the model applied in one bin's width of time,
/// and the range of event-log positions it was built from.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Bin {
    /// What the model applied.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Delta::is_empty"))]
    pub delta: Delta,
    /// The first and last event-log position marked in the bin
    /// ([`Recorder::mark`]), or `None` when none was. The host says what a
    /// position is: the running count of events applied in process, or the
    /// stream's sequence number over the wire. It is the link from a cell
    /// at a moment to the firings behind it in the fuller log: an
    /// [`EventLog`](crate::EventLog) kept with the same positions finds the
    /// bin's first entry by [`position_at`](crate::EventLog::position_at).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub log: Option<(u64, u64)>,
}

/// The first and last of two event-log ranges together.
fn union(a: Option<(u64, u64)>, b: Option<(u64, u64)>) -> Option<(u64, u64)> {
    match (a, b) {
        (Some((a0, a1)), Some((b0, b1))) => Some((a0.min(b0), a1.max(b1))),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Bins a model's deltas as they come: each frame's
/// [`take_delta`](Activity::take_delta) goes into the open bin with the
/// time that passed, and a bin closes when its width of time is up.
///
/// The closed bins are kept in a ring of `capacity` bins, or all of them;
/// [`record`](Recorder::record) takes a [`Record`] of what is kept. When the
/// ring drops its oldest bin, the counts and in-flight states that bin ends
/// in pass to the next one wherever it names no state of its own, so that a
/// replay of what is kept starts from the counts that stood.
///
/// A recorder's width and capacity are fixed, so a record is always
/// uniform: a host that changes either starts a fresh recorder. A recorder
/// started on a model already running takes the model's
/// [`standing`](Activity::standing) state first, so that its record starts
/// from the counts that stood.
///
/// ```
/// use std::time::Duration;
///
/// use petritype::{NetBuilder, TakeEdge};
/// use petritype_activity::{Activity, ActivitySettings, Grid, Recorder};
/// use petritype_layout::Layout;
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.seed(inbox, [1]);
/// b.transition("Go", (TakeEdge(inbox),), (out,), |n: u32| (n,));
/// let mut net = b.build()?;
/// let mut layout = Layout::default();
/// layout.compute(&net);
/// let grid = Grid::from_layout(&layout, &net.topology());
///
/// let mut activity = Activity::from_net(&net, ActivitySettings::default());
/// let mut recorder = Recorder::new(Duration::from_millis(100), None, None);
/// let frame = Duration::from_millis(50);
/// for _ in 0..4 {
///     net.step()?;
///     for event in net.drain_events() {
///         activity.apply(&event);
///     }
///     activity.tick(frame);
///     recorder.observe(activity.take_delta(), frame);
/// }
/// let record = recorder.record(&grid, Some(activity.settings()));
/// assert_eq!(record.len(), 2);
/// assert_eq!(record.bins[0].delta.firings, 1, "one step fired, three found nothing");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Recorder {
    bin_width: Duration,
    capacity: Option<usize>,
    /// When the oldest bin ever recorded opened, in milliseconds since the
    /// Unix epoch.
    start_unix_ms: Option<u64>,
    bins: VecDeque<Bin>,
    open: Bin,
    /// How far into the open bin the clock is.
    clock: Duration,
    dropped: u64,
}

impl Recorder {
    /// A recorder of bins `bin_width` wide, rounded to whole milliseconds
    /// and at least one, keeping the last `capacity` bins (at least one), or
    /// every bin for `None`. `start` is when the first bin opens, for the
    /// record's [`start_unix_ms`](Record::start_unix_ms) and its file name;
    /// `None` leaves the record undated.
    pub fn new(bin_width: Duration, capacity: Option<usize>, start: Option<SystemTime>) -> Self {
        let ms = bin_width.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        Self {
            bin_width: Duration::from_millis(ms),
            capacity: capacity.map(|n| n.max(1)),
            start_unix_ms: start.map(unix_ms),
            bins: VecDeque::new(),
            open: Bin::default(),
            clock: Duration::ZERO,
            dropped: 0,
        }
    }

    /// Takes a frame: `delta` goes into the open bin, and the clock moves
    /// on by `elapsed`. Each bin boundary the clock crosses closes the open
    /// bin and opens the next, so a long frame closes several, those after
    /// the first empty.
    pub fn observe(&mut self, delta: Delta, elapsed: Duration) {
        if self.open.delta.is_empty() {
            self.open.delta = delta;
        } else {
            self.open.delta.absorb(&delta);
        }
        self.clock += elapsed;
        let width = self.bin_width.as_nanos();
        let crossed = self.clock.as_nanos() / width;
        if crossed == 0 {
            return;
        }
        self.clock = Duration::from_nanos((self.clock.as_nanos() % width) as u64);
        let open = std::mem::take(&mut self.open);
        self.close(open);
        let mut empty = u64::try_from(crossed - 1).unwrap_or(u64::MAX);
        // More empty bins than the ring keeps: what it holds now is gone,
        // and so are the empty ones beyond its capacity, all at once.
        if let Some(capacity) = self.capacity
            && empty > capacity as u64
        {
            let mut standing = Delta::default();
            for bin in &self.bins {
                standing = standing_of(&standing, &bin.delta);
            }
            self.dropped += self.bins.len() as u64 + (empty - capacity as u64);
            self.bins.clear();
            self.bins.push_back(Bin {
                delta: standing,
                log: None,
            });
            empty = capacity as u64 - 1;
        }
        for _ in 0..empty {
            self.close(Bin::default());
        }
    }

    /// Marks an event-log position in the open bin: its range takes the
    /// position as its first or last if it lies outside.
    pub fn mark(&mut self, position: u64) {
        self.open.log = union(self.open.log, Some((position, position)));
    }

    /// How many bins the ring has dropped since the recorder began: a kept
    /// bin's absolute index is this plus its index in the
    /// [record](Recorder::record).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// A [`Record`] of the closed bins kept, oldest first, on `grid`, with
    /// the `settings` in force for a reader to see. Its start is the
    /// recorder's moved on by the bins dropped. The open bin is not in it.
    pub fn record(&self, grid: &Grid, settings: Option<&ActivitySettings>) -> Record {
        let width_ms = self.bin_width_ms();
        Record {
            grid: grid.clone(),
            bin_width_ms: width_ms,
            start_unix_ms: self.start_of(self.dropped),
            settings: settings.cloned(),
            bins: self.bins.iter().cloned().collect(),
        }
    }

    /// Drops every bin and the open one, keeping the width and the
    /// capacity: the recorder begins again from here, its start moved on by
    /// the time recorded, and nothing dropped.
    pub fn clear(&mut self) {
        let elapsed = (self.dropped + self.bins.len() as u64) * self.bin_width_ms()
            + self.clock.as_millis() as u64;
        self.start_unix_ms = self.start_unix_ms.map(|start| start + elapsed);
        self.bins.clear();
        self.open = Bin::default();
        self.clock = Duration::ZERO;
        self.dropped = 0;
    }

    /// How many closed bins are kept.
    pub fn len(&self) -> usize {
        self.bins.len()
    }

    /// Whether no bin has closed, or every one was cleared.
    pub fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    /// How wide each bin is, in whole milliseconds.
    pub fn bin_width(&self) -> Duration {
        self.bin_width
    }

    /// How many closed bins the ring keeps, or `None` for every one.
    pub fn capacity(&self) -> Option<usize> {
        self.capacity
    }

    /// A kept bin by its absolute index, as [`dropped`](Recorder::dropped)
    /// counts.
    pub(crate) fn bin(&self, absolute: u64) -> Option<&Bin> {
        let index = absolute.checked_sub(self.dropped)?;
        self.bins.get(usize::try_from(index).ok()?)
    }

    fn bin_width_ms(&self) -> u64 {
        self.bin_width.as_millis() as u64
    }

    /// When the bin of absolute index `absolute` opened.
    fn start_of(&self, absolute: u64) -> Option<u64> {
        self.start_unix_ms
            .map(|start| start + absolute * self.bin_width_ms())
    }

    /// A bin closed into the ring, the oldest dropped past the capacity
    /// with its standing state passed on.
    fn close(&mut self, bin: Bin) {
        self.bins.push_back(bin);
        while self.capacity.is_some_and(|c| self.bins.len() > c) {
            let Some(oldest) = self.bins.pop_front() else {
                break;
            };
            self.dropped += 1;
            if let Some(next) = self.bins.front_mut() {
                oldest.delta.carry_into(&mut next.delta);
            }
        }
    }
}

/// The standing state, counts and in-flight states with no hit, of
/// `earlier` followed by `later`.
fn standing_of(earlier: &Delta, later: &Delta) -> Delta {
    let mut standing = Delta::default();
    later.carry_into(&mut standing);
    earlier.carry_into(&mut standing);
    standing
}

/// Milliseconds since the Unix epoch, 0 for a time before it.
pub(crate) fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A fresh model of the net `grid` was taken from, every level at 0, run
/// by `settings`: the grid's cells give the nodes and their names.
pub(crate) fn model_on(grid: &Grid, settings: ActivitySettings) -> Activity {
    let mut places = Vec::new();
    let mut transitions = Vec::new();
    for cell in grid.cells() {
        match cell.node {
            LayoutNode::Place(id) => places.push((id, cell.name.clone())),
            LayoutNode::Transition(id) => transitions.push((id, cell.name.clone())),
        }
    }
    Activity::named(places, transitions, settings)
}

/// The stem of a file of the net `net` started at `start_unix_ms`:
/// `<net>-<YYYYMMDD-HHMMSS>` in UTC, or `<net>-undated`. A character a file
/// name cannot hold on some system, a slash, a backslash, a colon,
/// `*?"<>|` or a control character, is written as `_`, and an empty name
/// as `net`.
pub(crate) fn file_stem(net: &str, start_unix_ms: Option<u64>) -> String {
    let mut name: String = net
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if name.is_empty() {
        name.push_str("net");
    }
    let start = start_unix_ms.map_or_else(|| "undated".to_owned(), utc_stamp);
    format!("{name}-{start}")
}

/// A net's activity over time: the grid it was taken against, the bin
/// width, when it started, the settings in force when it was recorded, and
/// the bins, oldest first.
///
/// The thumbnail at any bin is a pure function of the record and the
/// settings it is replayed under: [`replay`](Record::replay) folds the bins
/// through a fresh model, each applied and then ticked by the width, and
/// [`Player`] folds to any bin from a checkpoint. Under the firings clock
/// the picture is the same at any bin width.
///
/// With the `serde` feature it is written as JSON by `write_json`, the grid
/// with its cells' nodes and names, and read back by `read_json`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Record {
    /// Where each node's cell goes, and each node's name: the key to the
    /// record's ids and to its [tensor](Record::tensor).
    pub grid: Grid,
    /// How wide each bin is, in milliseconds.
    pub bin_width_ms: u64,
    /// When the first bin opened, in milliseconds since the Unix epoch, or
    /// `None` for an undated record.
    pub start_unix_ms: Option<u64>,
    /// The settings in force when it was recorded, for a reader. A replay
    /// takes its own.
    pub settings: Option<ActivitySettings>,
    /// The bins, oldest first.
    pub bins: Vec<Bin>,
}

impl Record {
    /// How many bins it holds.
    pub fn len(&self) -> usize {
        self.bins.len()
    }

    /// Whether it holds no bin.
    pub fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    /// How much time it covers: its bins times their width.
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.bin_width_ms.saturating_mul(self.bins.len() as u64))
    }

    /// How wide each bin is.
    pub fn bin_width(&self) -> Duration {
        Duration::from_millis(self.bin_width_ms)
    }

    /// A fresh model of the record's net, every level at 0, run by
    /// `settings`: the grid's cells give the nodes and their names.
    pub fn model(&self, settings: ActivitySettings) -> Activity {
        model_on(&self.grid, settings)
    }

    /// The bins folded one by one through a [fresh model](Record::model)
    /// run by `settings`.
    pub fn replay(&self, settings: ActivitySettings) -> Replay<'_> {
        Replay {
            record: self,
            activity: self.model(settings),
            next: 0,
        }
    }

    /// Consecutive groups of `factor` bins merged into one, as
    /// [`Delta::absorb`] merges them, their log ranges joined, and the
    /// width times `factor`; a last group shorter than `factor` makes a
    /// last bin of its own. Exact by construction: every count is the sum
    /// of the group's, and every state the group's last. A `factor` of 0 is
    /// taken as 1.
    pub fn coarsen(&self, factor: usize) -> Record {
        let factor = factor.max(1);
        let bins = self
            .bins
            .chunks(factor)
            .map(|group| {
                let mut merged = Bin::default();
                for bin in group {
                    merged.delta.absorb(&bin.delta);
                    merged.log = union(merged.log, bin.log);
                }
                merged
            })
            .collect();
        Record {
            grid: self.grid.clone(),
            bin_width_ms: self.bin_width_ms.saturating_mul(factor as u64),
            start_unix_ms: self.start_unix_ms,
            settings: self.settings.clone(),
            bins,
        }
    }

    /// The file name for a record of the net `net`:
    /// `<net>-<start>.record.json`, the start as `YYYYMMDD-HHMMSS` in UTC, or
    /// `undated`. A character a file name cannot hold on some system, a
    /// slash, a backslash, a colon, `*?"<>|` or a control character, is
    /// written as `_`, and an empty name as `net`.
    pub fn file_name(&self, net: &str) -> String {
        format!("{}.record.json", file_stem(net, self.start_unix_ms))
    }

    /// The record laid out as time by rows by columns by channels, a
    /// [`Tensor`]: a frame per bin, a pixel per cell of the grid, and the
    /// eight [channels](Tensor::CHANNELS) per pixel.
    pub fn tensor(&self) -> Tensor {
        let (rows, columns) = (self.grid.rows(), self.grid.columns());
        let channels = Tensor::CHANNELS.len();
        let frame = rows * columns * channels;
        let mut data = vec![0.0_f32; self.bins.len() * frame];
        let width = self.bin_width_ms.max(1) as f64;
        // Each place's standing count, carried from the last bin that set
        // it.
        let mut counts: BTreeMap<petritype::PlaceId, usize> = BTreeMap::new();
        for (t, bin) in self.bins.iter().enumerate() {
            for (&place, e) in &bin.delta.places {
                counts.insert(place, e.count);
            }
            for cell in self.grid.cells() {
                let at = t * frame + (cell.row * columns + cell.column) * channels;
                let pixel = &mut data[at..at + channels];
                match cell.node {
                    LayoutNode::Transition(id) => {
                        if let Some(e) = bin.delta.transitions.get(&id) {
                            pixel[0] = e.fires as f32;
                            pixel[1] = e.distributes as f32;
                            pixel[2] = e.failures as f32;
                            pixel[3] = (e.in_flight_ms / width) as f32;
                        }
                    }
                    LayoutNode::Place(id) => {
                        if let Some(e) = bin.delta.places.get(&id) {
                            pixel[4] = e.pulse_hits as f32;
                            pixel[5] = e.ember_hits as f32;
                            pixel[6] = e.balance as f32;
                        }
                        pixel[7] = counts.get(&id).copied().unwrap_or(0) as f32;
                    }
                }
            }
        }
        Tensor {
            shape: [self.bins.len(), rows, columns, channels],
            data,
        }
    }
}

#[cfg(feature = "serde")]
impl Record {
    /// Writes the record as JSON, in one line: a record is large, so it is
    /// not pretty printed. The writer is buffered here.
    pub fn write_json(&self, w: impl Write) -> io::Result<()> {
        let mut w = io::BufWriter::new(w);
        serde_json::to_writer(&mut w, self)?;
        w.flush()
    }

    /// Reads a record written by [`write_json`](Record::write_json). The
    /// reader is buffered here.
    pub fn read_json(r: impl io::Read) -> io::Result<Record> {
        Ok(serde_json::from_reader(io::BufReader::new(r))?)
    }
}

/// `YYYYMMDD-HHMMSS` in UTC, from milliseconds since the Unix epoch.
fn utc_stamp(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let (days, of_day) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60
    )
}

/// The proleptic Gregorian date of a day counted from 1970-01-01, as year,
/// month (1 to 12) and day (1 to 31): Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A record's bins folded one at a time through a model it owns: each
/// [`next_bin`](Replay::next_bin) applies the next bin's delta and ticks by
/// the width, and lends the model as it then stands.
///
/// It lends rather than implementing [`Iterator`], whose items cannot
/// borrow the iterator itself:
///
/// ```
/// # use petritype_activity::{ActivitySettings, Record};
/// # fn levels(record: &Record) {
/// let mut replay = record.replay(ActivitySettings::default());
/// while let Some(activity) = replay.next_bin() {
///     let _ = activity.firings();
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Replay<'a> {
    record: &'a Record,
    activity: Activity,
    next: usize,
}

impl Replay<'_> {
    /// The next bin folded in, and the model after it; `None` past the
    /// last.
    pub fn next_bin(&mut self) -> Option<&Activity> {
        let bin = self.record.bins.get(self.next)?;
        self.activity.apply_delta(&bin.delta);
        self.activity.tick(self.record.bin_width());
        self.next += 1;
        Some(&self.activity)
    }

    /// The model as it stands: after the bins folded so far.
    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// How many bins have been folded.
    pub fn folded(&self) -> usize {
        self.next
    }
}

/// A scrubber over a record: [`at`](Player::at) is the model after any bin,
/// folded from the nearest checkpoint at or before it, so a jump anywhere
/// costs at most [`checkpoint_every`](Player::checkpoint_every) bins. The
/// checkpoints are made on the first pass over the bins and dropped when the
/// settings change.
///
/// A player holds a copy of its record. One made
/// [`following`](Player::following) a [`Recorder`] can
/// [`follow`](Player::follow) it as it grows and drops, its bins keeping
/// their absolute indices ([`first`](Player::first) plus the index), so a
/// host can play back a ring while it records.
#[derive(Debug, Clone)]
pub struct Player {
    record: Record,
    /// The absolute index of the record's first bin.
    first: u64,
    settings: ActivitySettings,
    every: usize,
    /// The model before the bin of each absolute index, folded.
    checkpoints: BTreeMap<u64, Activity>,
    /// The model after the bin of an absolute index, as `at` last left it.
    current: Option<(u64, Activity)>,
    /// The model before any bin: every level at 0.
    fresh: Activity,
}

impl Player {
    /// How many bins apart the checkpoints are unless a host says.
    pub const CHECKPOINT_EVERY: usize = 64;

    /// A player of a copy of `record`, run by `settings`, with a checkpoint
    /// every `checkpoint_every` bins (at least one).
    pub fn new(record: &Record, settings: ActivitySettings, checkpoint_every: usize) -> Self {
        Self {
            fresh: record.model(settings.clone()),
            record: record.clone(),
            first: 0,
            settings,
            every: checkpoint_every.max(1),
            checkpoints: BTreeMap::new(),
            current: None,
        }
    }

    /// A player of what `recorder` keeps, on `grid`, run by `settings`, with
    /// a checkpoint every `checkpoint_every` bins: one that knows the
    /// absolute index of its first bin, to [`follow`](Player::follow) the
    /// recorder as it records.
    pub fn following(
        recorder: &Recorder,
        grid: &Grid,
        settings: ActivitySettings,
        checkpoint_every: usize,
    ) -> Self {
        let mut player = Player::new(&recorder.record(grid, None), settings, checkpoint_every);
        player.first = recorder.dropped();
        player
    }

    /// The model after the bin at `index`, counted from the record's
    /// first, clamped to the last; the fresh model when there is no bin.
    pub fn at(&mut self, index: usize) -> &Activity {
        if self.record.bins.is_empty() {
            return &self.fresh;
        }
        let last = self.first + self.record.bins.len() as u64 - 1;
        let target = (self.first + index as u64).min(last);
        // Where to fold from: the model as `at` last left it, if it stands
        // at or before the target with every bin after it kept; else the
        // nearest checkpoint; else the fresh model at the first bin.
        let from_current = self
            .current
            .as_ref()
            .is_some_and(|(after, _)| *after <= target && *after + 1 >= self.first);
        let checkpoint = self
            .checkpoints
            .range(self.first..=target + 1)
            .next_back()
            .map(|(&before, _)| before);
        let (mut next, mut activity) = match (from_current, checkpoint) {
            (true, Some(before)) if before > self.current.as_ref().map_or(0, |c| c.0 + 1) => {
                (before, self.checkpoints[&before].clone())
            }
            (true, _) => {
                let (after, activity) = self.current.take().expect("checked above");
                (after + 1, activity)
            }
            (false, Some(before)) => (before, self.checkpoints[&before].clone()),
            (false, None) => (self.first, self.fresh.clone()),
        };
        let width = self.record.bin_width();
        while next <= target {
            let bin = &self.record.bins[(next - self.first) as usize];
            activity.apply_delta(&bin.delta);
            activity.tick(width);
            next += 1;
            if next % self.every as u64 == 0 && !self.checkpoints.contains_key(&next) {
                self.checkpoints.insert(next, activity.clone());
            }
        }
        &self.current.insert((target, activity)).1
    }

    /// The record played, as the player holds it.
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// How many bins the record holds.
    pub fn len(&self) -> usize {
        self.record.bins.len()
    }

    /// Whether the record holds no bin.
    pub fn is_empty(&self) -> bool {
        self.record.bins.is_empty()
    }

    /// The absolute index of the record's first bin: 0 for a record played
    /// as given, and the recorder's bins dropped for one that
    /// [follows](Player::follow) it.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// How many bins apart the checkpoints are.
    pub fn checkpoint_every(&self) -> usize {
        self.every
    }

    /// The settings the bins are folded under.
    pub fn settings(&self) -> &ActivitySettings {
        &self.settings
    }

    /// Folds under new settings from now on: a change drops every
    /// checkpoint, which the next pass makes again.
    pub fn set_settings(&mut self, settings: ActivitySettings) {
        if settings != self.settings {
            self.checkpoints.clear();
            self.current = None;
            self.fresh = self.record.model(settings.clone());
            self.settings = settings;
        }
    }

    /// Brings the record up to `recorder`'s, the one the player was made
    /// [`following`](Player::following): the bins it closed since are
    /// added, and those it dropped are dropped here too, with the
    /// checkpoints before them. The bins kept keep their absolute indices. A
    /// recorder cleared since, or of another width, gives its bins afresh.
    pub fn follow(&mut self, recorder: &Recorder) {
        let width_ms = recorder.bin_width_ms();
        let (theirs, end) = (
            recorder.dropped(),
            recorder.dropped() + recorder.len() as u64,
        );
        if theirs < self.first || width_ms != self.record.bin_width_ms {
            self.record.bins = recorder.bins.iter().cloned().collect();
            self.record.bin_width_ms = width_ms;
            self.first = theirs;
            self.checkpoints.clear();
            self.current = None;
        } else {
            let dropped = theirs > self.first;
            let gone = (theirs - self.first).min(self.record.bins.len() as u64) as usize;
            self.record.bins.drain(..gone);
            self.first = theirs;
            // The recorder's oldest kept bin carries the state of those it
            // dropped.
            if dropped
                && let (Some(mine), Some(its)) =
                    (self.record.bins.first_mut(), recorder.bin(theirs))
            {
                mine.clone_from(its);
            }
            let have = self.first + self.record.bins.len() as u64;
            for absolute in have..end {
                if let Some(bin) = recorder.bin(absolute) {
                    self.record.bins.push(bin.clone());
                }
            }
            let first = self.first;
            self.checkpoints.retain(|&before, _| before >= first);
            if self
                .current
                .as_ref()
                .is_some_and(|(after, _)| *after + 1 < first)
            {
                self.current = None;
            }
        }
        self.record.start_unix_ms = recorder.start_of(self.first);
    }
}

/// A record laid out for a classifier: time by rows by columns by
/// channels, row-major `f32`, as [`Record::tensor`] makes it. The grid's
/// cells, with their nodes' names, are the key to the rows and columns;
/// empty cells are zero.
///
/// The channels, in order ([`CHANNELS`](Tensor::CHANNELS)):
///
/// 0. `fires`: a transition's fires and launches in the bin.
/// 1. `distributes`: its distributes.
/// 2. `failures`: its failures.
/// 3. `in_flight`: the fraction of the bin it spent in flight, as the
///    model's ticks counted it. A frame longer than a bin puts all its time
///    in the bin open when it began, so the fraction there can pass 1.
/// 4. `pulse_hits`: a place's tokens taken, added or changed in place.
/// 5. `ember_hits`: the events that touched it.
/// 6. `balance`: the tokens its flow counted, added less taken.
/// 7. `count`: its standing count at the bin's end, carried from the last
///    bin that set it.
///
/// A transition's cell has only the first four, and a place's only the
/// last four.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    /// Bins, rows, columns and channels.
    pub shape: [usize; 4],
    /// Every value, row-major: the channel varies fastest, then the column,
    /// the row and the bin.
    pub data: Vec<f32>,
}

impl Tensor {
    /// The channels' names, in order.
    pub const CHANNELS: [&'static str; 8] = [
        "fires",
        "distributes",
        "failures",
        "in_flight",
        "pulse_hits",
        "ember_hits",
        "balance",
        "count",
    ];

    /// The value at a bin, row, column and channel, or `None` outside the
    /// shape.
    pub fn get(&self, bin: usize, row: usize, column: usize, channel: usize) -> Option<f32> {
        let [t, r, c, k] = self.shape;
        if bin >= t || row >= r || column >= c || channel >= k {
            return None;
        }
        self.data
            .get(((bin * r + row) * c + column) * k + channel)
            .copied()
    }

    /// Writes the tensor in NumPy's `.npy` form, version 1.0, which
    /// `numpy.load` reads with no code of ours: the magic `\x93NUMPY`, the
    /// version, the header's length as a little-endian `u16`, the header
    /// `{'descr': '<f4', 'fortran_order': False, 'shape': (t, r, c, 8), }`
    /// padded with spaces to end in a newline at a multiple of 64 bytes
    /// from the start, then the data as little-endian `f32`.
    pub fn write_npy(&self, w: impl Write) -> io::Result<()> {
        let [t, r, c, k] = self.shape;
        let mut header =
            format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({t}, {r}, {c}, {k}), }}");
        // The magic (6), the version (2) and the header's length (2) come
        // first; the newline ends the header.
        let unpadded = 10 + header.len() + 1;
        header.extend(std::iter::repeat_n(
            ' ',
            unpadded.next_multiple_of(64) - unpadded,
        ));
        header.push('\n');
        let length = u16::try_from(header.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "header too long"))?;
        let mut w = io::BufWriter::new(w);
        w.write_all(b"\x93NUMPY")?;
        w.write_all(&[1, 0])?;
        w.write_all(&length.to_le_bytes())?;
        w.write_all(header.as_bytes())?;
        for value in &self.data {
            w.write_all(&value.to_le_bytes())?;
        }
        w.flush()
    }
}

#[cfg(test)]
mod tests {
    use petritype::{Event, Net, NetBuilder, PlaceId, TakeEdge, TransitionId};
    use petritype_layout::Layout;

    use super::*;
    use crate::delta::{PlaceDelta, TransitionDelta};
    use crate::settings::Clock;
    use crate::traffic_light::{TrafficLight, build_traffic_light};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn drain(net: &mut Net, activity: &mut Activity) {
        let events: Vec<Event> = net.drain_events().collect();
        for event in &events {
            activity.apply(event);
        }
    }

    fn grid_of(net: &Net, layout: Layout) -> Grid {
        let mut layout = layout;
        layout.compute(net);
        Grid::from_layout(&layout, &net.topology())
    }

    /// Every level, count and state of every node, for comparing two
    /// models.
    fn levels(
        a: &Activity,
    ) -> Vec<(
        Option<crate::PlaceActivity>,
        Option<crate::TransitionActivity>,
    )> {
        let n = a.place_count().max(a.transition_count()) as u32;
        (0..n)
            .map(|i| (a.place(PlaceId::new(i)), a.transition(TransitionId::new(i))))
            .collect()
    }

    /// Settings with both layers on `clock`.
    fn on(clock: Clock) -> ActivitySettings {
        let mut settings = ActivitySettings::default();
        settings.pulse.clock = clock;
        settings.ember.clock = clock;
        settings
    }

    /// The traffic light run for `frames` frames of 100 ms, one bin each:
    /// each frame ticks the live model, then on two frames in three steps
    /// the net and on the third adds a car, and the recorder takes the
    /// frame. The live model after each frame, and the record.
    fn traffic(settings: &ActivitySettings, frames: usize) -> (Vec<Activity>, Record) {
        let mut light = build_traffic_light().expect("it builds");
        let grid = grid_of(&light.net, Layout::default());
        let mut live = Activity::from_net(&light.net, settings.clone());
        let mut recorder = Recorder::new(ms(100), None, None);
        let mut lives = Vec::new();
        for frame in 0..frames {
            live.tick(ms(100));
            if frame % 3 == 2 {
                light.net.put(light.waiting, format!("Car {frame}"));
            } else {
                light.net.step().expect("a step");
            }
            drain(&mut light.net, &mut live);
            recorder.observe(live.take_delta(), ms(100));
            lives.push(live.clone());
        }
        (lives, recorder.record(&grid, Some(settings)))
    }

    #[test]
    fn a_replay_matches_the_live_model_at_every_bin_boundary_on_either_clock() {
        // The wall clock: within one bin's decay, each layer by its own.
        let settings = on(Clock::Wall);
        let (lives, record) = traffic(&settings, 30);
        assert_eq!(record.len(), 30);
        let pulse = settings.pulse.decay(Clock::Wall, 0.1);
        let ember = settings.ember.decay(Clock::Wall, 0.1);
        let within = |live: f32, replayed: f32, decay: f32| {
            (live - replayed).abs() <= live.abs() * (1.0 - decay) + 1e-6
        };
        let mut replay = record.replay(settings.clone());
        let mut k = 0;
        while let Some(replayed) = replay.next_bin() {
            let live = &lives[k];
            for ((live_place, live_transition), (place, transition)) in
                levels(live).into_iter().zip(levels(replayed))
            {
                if let Some((l, r)) = live_place.zip(place) {
                    assert!(within(l.pulse, r.pulse, pulse), "bin {k}: {l:?} {r:?}");
                    assert!(within(l.ember, r.ember, ember), "bin {k}: {l:?} {r:?}");
                    assert!(within(l.flow, r.flow, ember), "bin {k}: {l:?} {r:?}");
                    assert_eq!(l.count, r.count, "bin {k}");
                }
                if let Some((l, r)) = live_transition.zip(transition) {
                    assert!(within(l.pulse, r.pulse, pulse), "bin {k}: {l:?} {r:?}");
                    assert!(within(l.ember, r.ember, ember), "bin {k}: {l:?} {r:?}");
                    assert_eq!(l.in_flight, r.in_flight, "bin {k}");
                }
            }
            assert_eq!(replayed.firings(), live.firings());
            k += 1;
        }
        assert_eq!((k, replay.folded()), (30, 30));

        // The firings clock: each bin holds one firing or one car, so every
        // level is the live one exactly; only the staleness dim, on the wall
        // clock, is within a bin's fade.
        let settings = on(Clock::Firings);
        let (lives, record) = traffic(&settings, 30);
        let fade = settings
            .stale_half_life_secs
            .map_or(1.0, |h| crate::settings::halving(h, 0.1));
        let mut replay = record.replay(settings);
        for live in &lives {
            let replayed = replay.next_bin().expect("a bin per frame");
            assert_eq!(levels(replayed), levels(live));
            assert_eq!(replayed.firings(), live.firings());
            assert!(within(live.freshness(), replayed.freshness(), fade));
        }
        assert!(replay.next_bin().is_none());
    }

    /// A delta that only sets `place`'s count to `count`, with `firings`.
    fn counted(place: u32, count: usize, firings: u32) -> Delta {
        let mut delta = Delta {
            firings,
            ..Delta::default()
        };
        delta.places.insert(
            PlaceId::new(place),
            PlaceDelta {
                count,
                ..PlaceDelta::default()
            },
        );
        delta
    }

    #[test]
    fn the_ring_drops_the_oldest_bin_past_its_capacity_and_carries_its_state() {
        let mut recorder = Recorder::new(ms(100), Some(3), None);
        let mut first = counted(0, 5, 1);
        first.transitions.insert(
            TransitionId::new(0),
            TransitionDelta {
                fires: 1,
                in_flight: true,
                ..TransitionDelta::default()
            },
        );
        recorder.observe(first, ms(100));
        for i in 1..5 {
            recorder.observe(counted(1, i, 1), ms(100));
        }
        assert_eq!((recorder.len(), recorder.dropped()), (3, 2));
        let grid = Grid::from_layout(
            &Layout::default(),
            &NetBuilder::new().build().unwrap().topology(),
        );
        let record = recorder.record(&grid, None);
        assert_eq!(record.bins[0].delta.places[&PlaceId::new(1)].count, 2);
        // The first bin's state passed on with no hit: place 0 still holds
        // five, and the transition is still in flight.
        assert_eq!(
            record.bins[0].delta.places[&PlaceId::new(0)],
            PlaceDelta {
                count: 5,
                ..PlaceDelta::default()
            }
        );
        assert_eq!(
            record.bins[0].delta.transitions[&TransitionId::new(0)],
            TransitionDelta {
                in_flight: true,
                ..TransitionDelta::default()
            }
        );
        assert_eq!(
            record.bins[0].delta.firings, 1,
            "the bin's own firing, not the dropped ones'"
        );

        // A long frame closes several bins, those after the first empty.
        recorder.observe(counted(1, 9, 1), ms(350));
        assert_eq!((recorder.len(), recorder.dropped()), (3, 5));
        let record = recorder.record(&grid, None);
        assert_eq!(record.bins[0].delta.places[&PlaceId::new(1)].count, 9);
        assert!(record.bins[1].delta.is_empty() && record.bins[2].delta.is_empty());
        // 50 ms into the open bin: 50 more close it.
        recorder.observe(Delta::default(), ms(50));
        assert_eq!(recorder.dropped(), 6);

        // A frame longer than the whole ring: three empty bins, the first
        // carrying every state that stood, and the rest dropped.
        recorder.observe(counted(2, 1, 1), Duration::from_secs(10));
        assert_eq!((recorder.len(), recorder.dropped()), (3, 6 + 100));
        let record = recorder.record(&grid, None);
        let standing = &record.bins[0].delta;
        assert_eq!(standing.firings, 0);
        let counts: Vec<(u32, usize)> = standing
            .places
            .iter()
            .map(|(p, e)| (p.index(), e.count))
            .collect();
        assert_eq!(counts, [(0, 5), (1, 9), (2, 1)]);
        assert!(standing.transitions[&TransitionId::new(0)].in_flight);

        // With no capacity, every bin is kept.
        let mut all = Recorder::new(ms(100), None, None);
        all.observe(counted(0, 1, 1), ms(450));
        assert_eq!((all.len(), all.dropped()), (4, 0));
        assert_eq!(all.capacity(), None);
        assert_eq!(all.bin_width(), ms(100));
    }

    #[test]
    fn marks_give_a_bin_its_log_range_and_clearing_moves_the_start() {
        let start = UNIX_EPOCH + Duration::from_millis(1_000_000);
        let mut recorder = Recorder::new(ms(100), Some(2), Some(start));
        recorder.mark(4);
        recorder.mark(7);
        recorder.mark(5);
        recorder.observe(Delta::default(), ms(100));
        recorder.observe(Delta::default(), ms(100));
        recorder.mark(8);
        recorder.observe(Delta::default(), ms(100));
        let grid = Grid::from_layout(
            &Layout::default(),
            &NetBuilder::new().build().unwrap().topology(),
        );
        let record = recorder.record(&grid, None);
        assert_eq!(
            record.bins.iter().map(|b| b.log).collect::<Vec<_>>(),
            [None, Some((8, 8))]
        );
        assert_eq!(
            record.start_unix_ms,
            Some(1_000_100),
            "moved on by the bin dropped"
        );
        let both = Record {
            bins: vec![
                Bin {
                    delta: Delta::default(),
                    log: Some((4, 7)),
                },
                Bin {
                    delta: Delta::default(),
                    log: None,
                },
                Bin {
                    delta: Delta::default(),
                    log: Some((8, 9)),
                },
            ],
            ..record.clone()
        };
        assert_eq!(
            both.coarsen(2)
                .bins
                .iter()
                .map(|b| b.log)
                .collect::<Vec<_>>(),
            [Some((4, 7)), Some((8, 9))]
        );
        assert_eq!(both.coarsen(3).bins[0].log, Some((4, 9)));

        recorder.observe(Delta::default(), ms(30));
        recorder.clear();
        assert!(recorder.is_empty());
        assert_eq!(recorder.dropped(), 0);
        assert_eq!(recorder.record(&grid, None).start_unix_ms, Some(1_000_330));
    }

    #[test]
    fn coarsening_sums_exactly_and_folds_to_the_same_on_the_firings_clock() {
        // A step in the first frame of each three, and a car after a
        // `Change`: one firing in each group of three bins, and every place
        // touched once in it.
        let settings = on(Clock::Firings);
        let TrafficLight {
            mut net,
            waiting,
            change,
            ..
        } = build_traffic_light().expect("it builds");
        let grid = grid_of(&net, Layout::default());
        let mut live = Activity::from_net(&net, settings.clone());
        let mut recorder = Recorder::new(ms(100), None, None);
        let mut changed = false;
        for frame in 0..9 {
            live.tick(ms(100));
            if frame % 3 == 0 {
                changed = net.step().expect("a step") == Some(change);
            } else if frame % 3 == 1 && changed {
                net.put(waiting, format!("Car {frame}"));
            }
            drain(&mut net, &mut live);
            recorder.observe(live.take_delta(), ms(100));
        }
        let fine = recorder.record(&grid, Some(&settings));
        assert_eq!(fine.len(), 9);
        let coarse = fine.coarsen(3);
        assert_eq!((coarse.len(), coarse.bin_width_ms), (3, 300));
        assert_eq!(coarse.duration(), fine.duration());
        for (k, bin) in coarse.bins.iter().enumerate() {
            let mut sum = Delta::default();
            for b in &fine.bins[3 * k..3 * k + 3] {
                sum.absorb(&b.delta);
            }
            assert_eq!(bin.delta, sum, "bin {k}");
            assert_eq!(bin.delta.firings, 1);
        }
        let mut fine_replay = fine.replay(settings.clone());
        let mut coarse_replay = coarse.replay(settings);
        for k in 0..3 {
            for _ in 0..3 {
                fine_replay.next_bin();
            }
            let coarse_now = coarse_replay.next_bin().expect("a coarse bin");
            assert_eq!(
                levels(coarse_now),
                levels(fine_replay.activity()),
                "bin {k}"
            );
            assert_eq!(coarse_now.firings(), fine_replay.activity().firings());
        }
        // A short last group is a bin of its own.
        let short = fine.coarsen(4);
        assert_eq!(short.len(), 3);
        assert_eq!(short.bins[2], fine.bins[8]);
        assert_eq!(fine.coarsen(0), fine);
    }

    #[test]
    fn a_player_folds_to_any_bin_as_the_replay_does() {
        let settings = on(Clock::Wall);
        let (_, record) = traffic(&settings, 40);
        let mut replay = record.replay(settings.clone());
        let mut folds = Vec::new();
        while let Some(a) = replay.next_bin() {
            folds.push((levels(a), a.freshness()));
        }
        let mut player = Player::new(&record, settings.clone(), 8);
        for index in [39, 3, 3, 4, 17, 0, 25, 24, 39, 8, 7] {
            let a = player.at(index);
            assert_eq!((levels(a), a.freshness()), folds[index], "bin {index}");
        }
        assert_eq!(levels(player.at(1000)), folds[39].0, "clamped to the last");
        assert_eq!(player.checkpoint_every(), 8);
        // New settings fold anew.
        let mut slow = settings.clone();
        slow.pulse.half_life_secs = 10.0;
        player.set_settings(slow.clone());
        let slower = levels(player.at(10));
        let mut again = record.replay(slow);
        for _ in 0..11 {
            again.next_bin();
        }
        assert_eq!(slower, levels(again.activity()));
        assert_ne!(slower, folds[10].0);
        // No bins: the fresh model.
        let empty = Record {
            bins: Vec::new(),
            ..record
        };
        assert_eq!(Player::new(&empty, settings, 64).at(5).firings(), 0);
    }

    #[test]
    fn a_player_follows_a_recorder_as_it_records_and_drops() {
        let mut light = build_traffic_light().expect("it builds");
        let grid = grid_of(&light.net, Layout::default());
        let settings = ActivitySettings::default();
        let mut live = Activity::from_net(&light.net, settings.clone());
        let mut recorder = Recorder::new(ms(100), Some(8), None);
        let frame = |light: &mut TrafficLight, live: &mut Activity, recorder: &mut Recorder| {
            live.tick(ms(100));
            light.net.step().expect("a step");
            drain(&mut light.net, live);
            recorder.observe(live.take_delta(), ms(100));
        };
        for _ in 0..5 {
            frame(&mut light, &mut live, &mut recorder);
        }
        let mut player = Player::following(&recorder, &grid, settings, 4);
        assert_eq!((player.first(), player.len()), (0, 5));
        player.at(4);
        for _ in 0..10 {
            frame(&mut light, &mut live, &mut recorder);
        }
        player.follow(&recorder);
        assert_eq!((player.first(), player.len()), (7, 8));
        assert_eq!(player.record().bins, recorder.record(&grid, None).bins);
        // The standing counts carried into the oldest kept bin: the light's
        // one token, whatever the fold began from.
        let light_id = light.light.id();
        assert_eq!(player.at(0).place(light_id).map(|p| p.count), Some(1));
        let newest = player.at(7).clone();
        for place in [light_id, light.waiting.id(), light.passed.id()] {
            assert_eq!(
                newest.place(place).map(|p| p.count),
                live.place(place).map(|p| p.count)
            );
        }
        // A cleared recorder gives its bins afresh.
        recorder.clear();
        frame(&mut light, &mut live, &mut recorder);
        player.follow(&recorder);
        assert_eq!((player.first(), player.len()), (0, 1));
    }
    /// Two clusters and a hub, as the grid's tests have them, with tokens
    /// to move.
    #[cfg(feature = "serde")]
    fn clustered() -> Net {
        let mut b = NetBuilder::new();
        let a_in = b.place::<u32>("A/In");
        let a_out = b.place::<u32>("A/Out");
        let b_in = b.place::<u32>("B/In");
        let b_mid = b.place::<u32>("B/Mid");
        let b_out = b.place::<u32>("B/Out");
        let hub = b.place::<u32>("Hub");
        b.seed(a_in, [1]);
        b.seed(b_in, [2]);
        b.seed(hub, [3, 4]);
        b.transition(
            "A/Go",
            (TakeEdge(a_in), TakeEdge(hub)),
            (a_out,),
            |n: u32, _: u32| (n,),
        );
        b.transition(
            "B/Go",
            (TakeEdge(b_in), TakeEdge(hub)),
            (b_mid,),
            |n: u32, _: u32| (n,),
        );
        b.transition("B/Next", (TakeEdge(b_mid),), (b_out,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_record_with_a_clustered_grid_round_trips_as_json() {
        use petritype_layout::{Clustering, Ghosting};

        let mut net = clustered();
        let grid = grid_of(
            &net,
            Layout::default()
                .with_clustering(Clustering::NamePrefix('/'))
                .with_ghosting(Ghosting::named(["Hub"])),
        );
        let settings = ActivitySettings::default();
        let mut live = Activity::from_net(&net, settings.clone());
        let start = UNIX_EPOCH + Duration::from_millis(1_792_499_696_789);
        let mut recorder = Recorder::new(ms(100), Some(100), Some(start));
        let next = net.transition_named("B/Next").unwrap();
        for frame in 0..6 {
            if frame == 0 {
                // A staged firing, in flight over a frame and a third.
                let firing = net
                    .launch(net.transition_named("B/Go").unwrap())
                    .expect("B/Go launches");
                drain(&mut net, &mut live);
                live.tick(ms(130));
                net.distribute(firing.run()).expect("it lands");
            } else {
                let _ = net.step().expect("a step");
            }
            drain(&mut net, &mut live);
            live.tick(ms(70));
            recorder.observe(live.take_delta(), ms(200));
            recorder.mark(frame);
        }
        let record = recorder.record(&grid, Some(&settings));
        assert!(
            record
                .bins
                .iter()
                .any(|b| b.delta.transitions.get(&next).is_some_and(|t| t.fires > 0))
        );
        assert!(
            record
                .bins
                .iter()
                .any(|b| b.delta.transitions.values().any(|t| t.in_flight_ms > 0.0))
        );
        assert!(
            record.bins.iter().any(|b| b.delta.is_empty()),
            "and an empty bin"
        );
        let mut text = Vec::new();
        record.write_json(&mut text).expect("it writes");
        let back = Record::read_json(text.as_slice()).expect("and reads back");
        assert_eq!(back, record);
        assert_eq!(
            back.grid.cell_of(LayoutNode::Transition(next)),
            record.grid.cell_of(LayoutNode::Transition(next))
        );
        let text = String::from_utf8(text).expect("JSON is text");
        assert!(!text.contains('\n'), "on one line");
        assert!(text.contains(r#"{"transition":"#), "{text}");
        assert!(
            text.contains(r#""bins":[{},"#) || text.contains(r#",{},"#) || text.contains(r#"{}]"#),
            "{text}"
        );

        // A grid that puts a cell outside itself is refused.
        let bad = text.replacen(r#""columns":"#, r#""columns":0,"was":"#, 1);
        assert!(Record::read_json(bad.as_bytes()).is_err());
    }

    #[test]
    fn the_tensor_lays_out_a_cell_per_pixel_and_eight_channels() {
        // Two sources into one transition: two columns, three rows, and two
        // empty positions.
        let mut b = NetBuilder::new();
        let left = b.place::<u32>("Left");
        let right = b.place::<u32>("Right");
        let out = b.place::<u32>("Out");
        b.seed(left, [1, 2]);
        b.seed(right, [3, 4]);
        let join = b
            .transition(
                "Join",
                (TakeEdge(left), TakeEdge(right)),
                (out,),
                |a: u32, _: u32| (a,),
            )
            .id();
        let mut net = b.build().expect("the net builds");
        let grid = grid_of(&net, Layout::default());
        assert_eq!((grid.columns(), grid.rows()), (2, 3));
        let mut live = Activity::from_net(&net, ActivitySettings::default());
        let mut recorder = Recorder::new(ms(100), None, None);
        // A fire; a launch in flight over the bin; its distribute; nothing.
        net.step().expect("Join fires");
        drain(&mut net, &mut live);
        recorder.observe(live.take_delta(), ms(100));
        let firing = net.launch(join).expect("Join launches");
        drain(&mut net, &mut live);
        live.tick(ms(50));
        recorder.observe(live.take_delta(), ms(100));
        net.distribute(firing.run()).expect("it lands");
        drain(&mut net, &mut live);
        recorder.observe(live.take_delta(), ms(100));
        recorder.observe(Delta::default(), ms(100));
        let record = recorder.record(&grid, None);
        let tensor = record.tensor();
        assert_eq!(tensor.shape, [4, 3, 2, 8]);
        assert_eq!(tensor.data.len(), 4 * 3 * 2 * 8);
        let at = |node| {
            let cell = grid.cell_of(node).expect("a cell");
            (cell.row, cell.column)
        };
        let channels = |bin, (row, column)| -> Vec<f32> {
            (0..8)
                .map(|k| tensor.get(bin, row, column, k).unwrap())
                .collect()
        };
        let (j, l, o) = (
            at(LayoutNode::Transition(join)),
            at(LayoutNode::Place(left.id())),
            at(LayoutNode::Place(out.id())),
        );
        assert_eq!(channels(0, j), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(channels(0, l), [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, -1.0, 1.0]);
        assert_eq!(channels(0, o), [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            channels(1, j),
            [1.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0],
            "in flight half the bin"
        );
        assert_eq!(
            channels(1, l),
            [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0],
            "the flow waits"
        );
        assert_eq!(channels(2, j), [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(channels(2, l), [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0]);
        assert_eq!(channels(2, o), [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0]);
        assert_eq!(channels(3, o)[7], 2.0, "the count carried into a quiet bin");
        // The empty positions are zero throughout.
        let empty: Vec<(usize, usize)> = (0..3)
            .flat_map(|r| (0..2).map(move |c| (r, c)))
            .filter(|&(r, c)| grid.at(c, r).is_none())
            .collect();
        assert_eq!(empty.len(), 2);
        for bin in 0..4 {
            for &position in &empty {
                assert_eq!(channels(bin, position), [0.0; 8]);
            }
        }
        assert_eq!(tensor.get(4, 0, 0, 0), None);
        assert_eq!(Tensor::CHANNELS[3], "in_flight");

        // The `.npy` form: the magic, version 1.0, a padded header and the
        // data.
        let mut bytes = Vec::new();
        tensor.write_npy(&mut bytes).expect("it writes");
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        assert_eq!(&bytes[6..8], &[1, 0]);
        let length = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        assert_eq!((10 + length) % 64, 0, "the preamble is whole 64-byte lines");
        let header = std::str::from_utf8(&bytes[10..10 + length]).expect("the header is text");
        assert!(header.ends_with('\n'));
        assert_eq!(
            header.trim_end(),
            "{'descr': '<f4', 'fortran_order': False, 'shape': (4, 3, 2, 8), }"
        );
        let data = &bytes[10 + length..];
        assert_eq!(data.len(), 4 * 3 * 2 * 8 * 4);
        let values: Vec<f32> = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect();
        assert_eq!(values, tensor.data);
    }

    #[test]
    fn a_file_name_is_the_net_and_the_start_in_utc() {
        let grid = Grid::from_layout(
            &Layout::default(),
            &NetBuilder::new().build().unwrap().topology(),
        );
        let dated = |ms| Record {
            grid: grid.clone(),
            bin_width_ms: 100,
            start_unix_ms: ms,
            settings: None,
            bins: Vec::new(),
        };
        assert_eq!(
            dated(Some(1_792_499_696_789)).file_name("parcel"),
            "parcel-20261020-123456.record.json"
        );
        assert_eq!(
            dated(Some(1_709_251_199_000)).file_name("leap"),
            "leap-20240229-235959.record.json"
        );
        assert_eq!(
            dated(Some(946_598_407_000)).file_name("Traffic light"),
            "Traffic light-19991231-000007.record.json"
        );
        assert_eq!(
            dated(Some(0)).file_name("x"),
            "x-19700101-000000.record.json"
        );
        assert_eq!(
            dated(None).file_name("parcel"),
            "parcel-undated.record.json"
        );
        assert_eq!(dated(None).file_name("a/b:c"), "a_b_c-undated.record.json");
        assert_eq!(dated(None).file_name(""), "net-undated.record.json");
    }
}
