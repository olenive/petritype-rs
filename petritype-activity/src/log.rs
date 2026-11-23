//! The event log: what a net's model took, entry by entry and in order,
//! for a reader who wants the order in which things happened rather than
//! how long they took. [`Entry`] is one thing the model acts on, [`EventLog`]
//! keeps the entries in a ring with the [`Standing`] state its oldest kept
//! entry starts from, and the [`Roster`] of the tokens it can name there,
//! and [`EventPlayer`] replays the log one entry at a time, each shown for a
//! fixed dwell.

use std::collections::{BTreeMap, VecDeque};
#[cfg(feature = "serde")]
use std::io::{self, Write};
use std::mem::size_of;
use std::time::{Duration, SystemTime};

use petritype::{Event, PlaceId, TokenId, TransitionId};

use crate::delta::{Delta, PlaceDelta, TransitionDelta};
use crate::grid::Grid;
use crate::model::Activity;
use crate::record::{file_stem, model_on, unix_ms};
use crate::settings::ActivitySettings;
use crate::tokens::{Look, Roster, staged_failures};

/// One thing an [`Activity`] acts on, as an [`EventLog`] keeps it: each of
/// the core's events that moves a level, by ids only, and the two
/// corrections a host makes when it sets the model from a mirror's state.
///
/// The core's [`Event`] is emitted and never read back, so the log keeps
/// entries of its own: [`from_event`](Entry::from_event) makes one from an
/// event, and [`Activity::apply_entry`] applies it by the rules, and the
/// code, [`Activity::apply`] applies the event by. Token ids are kept, so
/// that a reader can follow a token through the log.
///
/// With the `serde` feature it is `Serialize` and `Deserialize`, tagged by
/// variant under `"event"` in `snake_case` with ids as bare numbers, the
/// form the core writes its events in: a [`Fired`](Entry::Fired) reads
/// `{"event":"fired","transition":0,"consumed":[[0,17]],"produced":[[1,18]]}`
/// and a [`Count`](Entry::Count) `{"event":"count","place":2,"count":3}`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "event", rename_all = "snake_case"))]
pub enum Entry {
    /// A firing in one go: [`Event::Fired`].
    Fired {
        /// The transition that fired.
        transition: TransitionId,
        /// The tokens taken, and the places they were in.
        consumed: Vec<(PlaceId, TokenId)>,
        /// The tokens written, and the places they went to.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// The start of a staged firing: [`Event::Launched`].
    Launched {
        /// The transition now in flight.
        transition: TransitionId,
        /// The tokens taken, and the places they were in.
        consumed: Vec<(PlaceId, TokenId)>,
    },
    /// The end of a staged firing: [`Event::Distributed`].
    Distributed {
        /// The transition that completed.
        transition: TransitionId,
        /// The tokens written, and the places they went to.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// A firing that failed: [`Event::Failed`].
    Failed {
        /// The transition that failed.
        transition: TransitionId,
        /// The tokens the firing took, and the places they were in.
        consumed: Vec<(PlaceId, TokenId)>,
        /// Whether those tokens were put back.
        restored: bool,
        /// The failure, as the event rendered it.
        error: String,
    },
    /// A token added from outside the net: [`Event::Added`].
    Added {
        /// The place it is now in.
        place: PlaceId,
        /// The new token.
        token: TokenId,
    },
    /// A token taken out from outside the net: [`Event::Removed`].
    Removed {
        /// The place it was in.
        place: PlaceId,
        /// The retired token.
        token: TokenId,
    },
    /// A token changed in place: [`Event::Mutated`].
    Mutated {
        /// The place holding it.
        place: PlaceId,
        /// The token, whose id is unchanged.
        token: TokenId,
    },
    /// A place's standing count, as the host read it from a mirror's state:
    /// what [`Activity::set_count`] takes.
    Count {
        /// The place.
        place: PlaceId,
        /// Its count.
        count: usize,
    },
    /// Whether a transition stands in flight, as the host read it from a
    /// mirror's state: what [`Activity::set_in_flight`] takes.
    InFlight {
        /// The transition.
        transition: TransitionId,
        /// Whether it is in flight.
        on: bool,
    },
}

impl Entry {
    /// The entry for one of the core's events, or `None` for an event the
    /// model takes nothing from: [`Toggled`](Event::Toggled),
    /// [`ParamChanged`](Event::ParamChanged), [`Resynced`](Event::Resynced),
    /// [`Nested`](Event::Nested) and any variant the core's enum gains.
    pub fn from_event(event: &Event) -> Option<Entry> {
        Some(match event {
            Event::Fired {
                transition,
                consumed,
                produced,
            } => Entry::Fired {
                transition: *transition,
                consumed: consumed.clone(),
                produced: produced.clone(),
            },
            Event::Launched {
                transition,
                consumed,
            } => Entry::Launched {
                transition: *transition,
                consumed: consumed.clone(),
            },
            Event::Distributed {
                transition,
                produced,
            } => Entry::Distributed {
                transition: *transition,
                produced: produced.clone(),
            },
            Event::Failed {
                transition,
                consumed,
                restored,
                error,
            } => Entry::Failed {
                transition: *transition,
                consumed: consumed.clone(),
                restored: *restored,
                error: error.clone(),
            },
            Event::Added { place, token } => Entry::Added {
                place: *place,
                token: *token,
            },
            Event::Removed { place, token } => Entry::Removed {
                place: *place,
                token: *token,
            },
            Event::Mutated { place, token } => Entry::Mutated {
                place: *place,
                token: *token,
            },
            _ => return None,
        })
    }
}

/// The counts and in-flight transitions standing at one moment: each place
/// holding tokens with its count, and each transition in flight, both in
/// the order of their ids, with no place at a count of 0.
///
/// [`Activity::standing_state`] reads it off a model, an [`EventLog`] keeps
/// the one its oldest kept entry starts from as its
/// [`base`](EventLog::base), and [`advance`](Standing::advance) moves it on
/// by an entry, by the model's count rule. With the `serde` feature it is
/// `Serialize` and `Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Standing {
    /// Each place holding tokens, with its count, in the order of their ids.
    pub counts: Vec<(PlaceId, usize)>,
    /// Each transition in flight, in the order of their ids.
    pub in_flight: Vec<TransitionId>,
}

impl Standing {
    /// Moves the state on by one entry, by the count rule
    /// [`Activity::apply_entry`] follows, and nothing of its levels:
    ///
    /// - [`Fired`](Entry::Fired) takes its consumed tokens and adds its
    ///   produced ones.
    /// - [`Launched`](Entry::Launched) takes its consumed tokens and puts the
    ///   transition in flight.
    /// - [`Distributed`](Entry::Distributed) adds its produced tokens and
    ///   takes the transition out of flight.
    /// - [`Failed`](Entry::Failed) of a transition in flight adds its
    ///   consumed tokens back when they were restored, and moves no count
    ///   when they were lost, since its launch took them. Of a transition not
    ///   in flight it takes them, and adds them back when restored. Either
    ///   way the transition is out of flight.
    /// - [`Added`](Entry::Added) adds one, [`Removed`](Entry::Removed) takes
    ///   one, and [`Mutated`](Entry::Mutated) moves nothing.
    /// - [`Count`](Entry::Count) and [`InFlight`](Entry::InFlight) set what
    ///   they name.
    ///
    /// A count never goes below 0. The state knows no net, so it follows an
    /// entry naming a node the model does not know, which the model ignores.
    ///
    /// [`worked_back`](Standing::worked_back) undoes what this does.
    pub fn advance(&mut self, entry: &Entry) {
        match entry {
            Entry::Fired {
                consumed, produced, ..
            } => {
                self.take(consumed);
                self.add(produced);
            }
            Entry::Launched {
                transition,
                consumed,
            } => {
                self.take(consumed);
                self.set_in_flight(*transition, true);
            }
            Entry::Distributed {
                transition,
                produced,
            } => {
                self.add(produced);
                self.set_in_flight(*transition, false);
            }
            Entry::Failed {
                transition,
                consumed,
                restored,
                ..
            } => {
                if !self.is_in_flight(*transition) {
                    self.take(consumed);
                }
                if *restored {
                    self.add(consumed);
                }
                self.set_in_flight(*transition, false);
            }
            Entry::Added { place, .. } => self.set_count(*place, self.count(*place) + 1),
            Entry::Removed { place, .. } => {
                self.set_count(*place, self.count(*place).saturating_sub(1));
            }
            Entry::Mutated { .. } => {}
            Entry::Count { place, count } => self.set_count(*place, *count),
            Entry::InFlight { transition, on } => self.set_in_flight(*transition, *on),
        }
    }

    /// The state that stood before `entries`, when this one stands after
    /// them: each is undone, the last first, by the rule
    /// [`advance`](Standing::advance) did it by. It is how a host begins a
    /// log on a net it first reads after some of the net's entries have
    /// come, such as an inner net whose first event it sees before it can
    /// read the net, or whose state it asks for and reads after more events
    /// came: read the state, work it back through those entries, and the log
    /// begins on the counts that stood before the first of them.
    ///
    /// A failure is undone as the failure of a staged firing when a launch
    /// of its transition comes before it among the entries with no landing
    /// in between, and as a firing in one go otherwise: a staged firing
    /// launched before the first entry and failing among them is more than
    /// the entries can tell. A correction is undone as far as it can be: an
    /// [`InFlight`](Entry::InFlight) was the other way before, a host
    /// logging one only when it changes something, and a
    /// [`Count`](Entry::Count) keeps the count, the one before it being
    /// unknown.
    ///
    /// ```
    /// use petritype::{PlaceId, TokenId, TransitionId};
    /// use petritype_activity::{Entry, Standing};
    ///
    /// let (queue, done) = (PlaceId::new(0), PlaceId::new(1));
    /// let fired = Entry::Fired {
    ///     transition: TransitionId::new(0),
    ///     consumed: vec![(queue, TokenId::new(1))],
    ///     produced: vec![(done, TokenId::new(2))],
    /// };
    /// let before = Standing { counts: vec![(queue, 2)], in_flight: vec![] };
    /// let mut after = before.clone();
    /// after.advance(&fired);
    /// assert_eq!(after.worked_back(&[fired]), before);
    /// ```
    pub fn worked_back(&self, entries: &[Entry]) -> Standing {
        let staged = staged_failures(entries);
        let mut standing = self.clone();
        for (entry, staged) in entries.iter().zip(staged).rev() {
            match entry {
                Entry::Fired {
                    consumed, produced, ..
                } => {
                    standing.take(produced);
                    standing.add(consumed);
                }
                Entry::Launched {
                    transition,
                    consumed,
                } => {
                    standing.add(consumed);
                    standing.set_in_flight(*transition, false);
                }
                Entry::Distributed {
                    transition,
                    produced,
                } => {
                    standing.take(produced);
                    standing.set_in_flight(*transition, true);
                }
                Entry::Failed {
                    transition,
                    consumed,
                    restored,
                    ..
                } => {
                    if staged {
                        if *restored {
                            standing.take(consumed);
                        }
                        standing.set_in_flight(*transition, true);
                    } else if !*restored {
                        standing.add(consumed);
                    }
                }
                Entry::Added { place, .. } => {
                    standing.set_count(*place, standing.count(*place).saturating_sub(1));
                }
                Entry::Removed { place, .. } => {
                    standing.set_count(*place, standing.count(*place).saturating_add(1));
                }
                Entry::Mutated { .. } | Entry::Count { .. } => {}
                Entry::InFlight { transition, on } => standing.set_in_flight(*transition, !on),
            }
        }
        standing
    }

    /// A place's count: 0 for a place it does not name.
    pub fn count(&self, place: PlaceId) -> usize {
        self.counts
            .binary_search_by_key(&place, |&(p, _)| p)
            .map_or(0, |at| self.counts[at].1)
    }

    /// Whether a transition is in flight.
    pub fn is_in_flight(&self, transition: TransitionId) -> bool {
        self.in_flight.binary_search(&transition).is_ok()
    }

    /// The state as a [`Delta`] with no hit, as
    /// [`Activity::standing`] gives it.
    pub(crate) fn to_delta(&self) -> Delta {
        let mut delta = Delta::default();
        for &(place, count) in &self.counts {
            delta.places.insert(
                place,
                PlaceDelta {
                    count,
                    ..PlaceDelta::default()
                },
            );
        }
        for &transition in &self.in_flight {
            delta.transitions.insert(
                transition,
                TransitionDelta {
                    in_flight: true,
                    ..TransitionDelta::default()
                },
            );
        }
        delta
    }

    /// A model run by `settings` on `grid`'s nodes, every level at 0, with
    /// these counts and flights set and its account empty.
    fn model_on(&self, grid: &Grid, settings: ActivitySettings) -> Activity {
        let mut activity = model_on(grid, settings);
        for &(place, count) in &self.counts {
            activity.set_count(place, count);
        }
        for &transition in &self.in_flight {
            activity.set_in_flight(transition, true);
        }
        activity.take_delta();
        activity
    }

    /// One token taken from each place `tokens` names.
    fn take(&mut self, tokens: &[(PlaceId, TokenId)]) {
        for &(place, _) in tokens {
            self.set_count(place, self.count(place).saturating_sub(1));
        }
    }

    /// One token added to each place `tokens` names.
    fn add(&mut self, tokens: &[(PlaceId, TokenId)]) {
        for &(place, _) in tokens {
            self.set_count(place, self.count(place).saturating_add(1));
        }
    }

    fn set_count(&mut self, place: PlaceId, count: usize) {
        match self.counts.binary_search_by_key(&place, |&(p, _)| p) {
            Ok(at) if count == 0 => {
                self.counts.remove(at);
            }
            Ok(at) => self.counts[at].1 = count,
            Err(at) if count > 0 => self.counts.insert(at, (place, count)),
            Err(_) => {}
        }
    }

    fn set_in_flight(&mut self, transition: TransitionId, on: bool) {
        match (self.in_flight.binary_search(&transition), on) {
            (Ok(at), false) => {
                self.in_flight.remove(at);
            }
            (Err(at), true) => self.in_flight.insert(at, transition),
            _ => {}
        }
    }
}

/// One entry as an [`EventLog`] keeps it: the host's position for it, when
/// it came, the entry, and how the tokens it put down or changed looked.
///
/// The position is the host's to give. A host that keeps the logs of
/// several nets, a net and the nets inside it, numbers every entry of every
/// one on one sequence, so that a position in one log names the same moment
/// in every other ([`EventLog::index_at`]).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Logged {
    /// The host's position for the entry, the number a
    /// [`Recorder`](crate::Recorder) marks its bin with
    /// ([`mark`](crate::Recorder::mark)), so that a bin's log range names
    /// the entries it was built from.
    pub seq: u64,
    /// When it came, in milliseconds from the log's start on the wall
    /// clock: for a reader and for the time shown beside it. A replay goes
    /// at a pace of its own.
    pub at_ms: u64,
    /// What the model took.
    pub entry: Entry,
    /// How each token the entry put down or changed looked after it, for
    /// those the host could read: a look is kept once for each change of a
    /// token. Empty for an entry that put down and changed nothing, and for
    /// a host that keeps no looks. With the `serde` feature it is left out
    /// when empty, and a log written before it existed reads with none.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub looks: Vec<(TokenId, Look)>,
}

impl Logged {
    /// About how many bytes it holds, its token lists, its error and its
    /// looks' labels among them: what a host counts against a budget.
    pub fn bytes(&self) -> usize {
        let pairs = size_of::<(PlaceId, TokenId)>();
        let entry = match &self.entry {
            Entry::Fired {
                consumed, produced, ..
            } => (consumed.len() + produced.len()) * pairs,
            Entry::Launched { consumed, .. } => consumed.len() * pairs,
            Entry::Distributed { produced, .. } => produced.len() * pairs,
            Entry::Failed {
                consumed, error, ..
            } => consumed.len() * pairs + error.len(),
            _ => 0,
        };
        let looks: usize = self
            .looks
            .iter()
            .map(|(_, look)| size_of::<TokenId>() + look.bytes())
            .sum();
        size_of::<Logged>() + entry + looks
    }
}

/// The entries a net's model took, in order, each with the host's position
/// and its time: a ring of the last `capacity` entries, or every one.
///
/// When the ring drops its oldest entry, the [`base`](EventLog::base)
/// advances by it, so the base is always the standing state the oldest
/// kept entry starts from and a replay of what is kept starts from the
/// counts that stood. An entry's absolute index is
/// [`first`](EventLog::first) plus its place in the ring, and stays the
/// same while entries drop behind it.
///
/// Beside the base it keeps the [`Roster`] of the tokens it can name at the
/// oldest kept entry, [`base_tokens`](EventLog::base_tokens), advanced with
/// the base, so that a reader can name the tokens of a place at any kept
/// entry and say how each looked ([`EventPlayer::tokens_at`]).
///
/// An entry with two short token lists takes on the order of 150 bytes, so
/// a hundred thousand take some 15 MB; [`bytes`](EventLog::bytes) counts
/// what a log holds, labels included, for a host that bounds its logs by
/// memory. With the `serde` feature the log is written as JSON by
/// `write_json` and read back by `read_json`.
///
/// ```
/// use std::time::Duration;
///
/// use petritype::{NetBuilder, TakeEdge};
/// use petritype_activity::{Activity, ActivitySettings, Entry, EventLog};
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.seed(inbox, [1, 2]);
/// b.transition("Go", (TakeEdge(inbox),), (out,), |n: u32| (n,));
/// let mut net = b.build()?;
///
/// let mut activity = Activity::from_net(&net, ActivitySettings::default());
/// let mut log = EventLog::new(Some(1), None, activity.standing_state());
/// let mut seq = 0;
/// for _ in 0..2 {
///     net.step()?;
///     for event in net.drain_events() {
///         if let Some(entry) = Entry::from_event(&event) {
///             activity.apply_entry(&entry);
///             log.push(entry, seq, Duration::from_millis(100 * seq));
///             seq += 1;
///         }
///     }
/// }
/// assert_eq!((log.len(), log.dropped(), log.first()), (1, 1, 1));
/// // The base stands after the first firing, which the ring dropped.
/// assert_eq!(log.base().count(out.id()), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventLog {
    /// How many entries the ring keeps, or `None` for every one.
    capacity: Option<usize>,
    /// When the log began, in milliseconds since the Unix epoch.
    start_unix_ms: Option<u64>,
    /// The standing state the oldest kept entry starts from.
    base: Standing,
    /// The tokens named at the oldest kept entry, and their looks. A log
    /// written before it existed reads with none.
    #[cfg_attr(feature = "serde", serde(default))]
    tokens: Roster,
    /// How many entries are no longer kept.
    dropped: u64,
    entries: VecDeque<Logged>,
    /// What the kept entries hold, counted as they come and go.
    #[cfg_attr(feature = "serde", serde(skip))]
    held: usize,
}

impl EventLog {
    /// A log keeping the last `capacity` entries (at least one), or every
    /// entry for `None`, begun at `start`, for its file name and the
    /// entries' times (`None` leaves it undated), on `base`, the counts and
    /// flights standing when it began: a model's
    /// [`standing_state`](Activity::standing_state). It names no token at
    /// its start until [`with_tokens`](EventLog::with_tokens) says.
    pub fn new(capacity: Option<usize>, start: Option<SystemTime>, base: Standing) -> Self {
        Self {
            capacity: capacity.map(|n| n.max(1)),
            start_unix_ms: start.map(unix_ms),
            base,
            tokens: Roster::default(),
            dropped: 0,
            entries: VecDeque::new(),
            held: 0,
        }
    }

    /// The same log, naming `tokens` at its start: the tokens its host
    /// could read in each place when it began, with their looks.
    pub fn with_tokens(mut self, tokens: Roster) -> Self {
        self.tokens = tokens;
        self
    }

    /// Adds an entry at the host's position `seq`, `at` after the log's
    /// start, with no look. Past the capacity the oldest entry is dropped
    /// and the base advanced by it.
    pub fn push(&mut self, entry: Entry, seq: u64, at: Duration) {
        self.push_with(entry, seq, at, Vec::new());
    }

    /// Adds an entry as [`push`](EventLog::push) does, with how each token
    /// it put down or changed looked after it: the looks a reader names
    /// those tokens by ([`Logged::looks`]).
    pub fn push_with(&mut self, entry: Entry, seq: u64, at: Duration, looks: Vec<(TokenId, Look)>) {
        let logged = Logged {
            seq,
            at_ms: u64::try_from(at.as_millis()).unwrap_or(u64::MAX),
            entry,
            looks,
        };
        self.held += logged.bytes();
        self.entries.push_back(logged);
        while self.capacity.is_some_and(|c| self.entries.len() > c) {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.held = self.held.saturating_sub(oldest.bytes());
            self.base.advance(&oldest.entry);
            self.tokens.advance(&oldest);
            self.dropped += 1;
        }
    }

    /// About how many bytes the log holds: its entries, their looks'
    /// labels, its base and the roster of tokens it names at its start.
    pub fn bytes(&self) -> usize {
        size_of::<EventLog>()
            + self.held
            + self.tokens.bytes()
            + (self.base.counts.len() + self.base.in_flight.len()) * 16
    }

    /// The tokens named at the oldest kept entry, with their looks: the
    /// roster the log began with, advanced by every entry dropped since.
    pub fn base_tokens(&self) -> &Roster {
        &self.tokens
    }

    /// The absolute index of the last kept entry at or before the host's
    /// position `seq`, the moment a reader of several logs on one sequence
    /// shows this one at; `None` when every kept entry is after it, or none
    /// is kept, where the log stands at its base.
    pub fn index_at(&self, seq: u64) -> Option<u64> {
        index_at(&self.entries, self.dropped, seq)
    }

    /// How many entries are no longer kept, dropped past the capacity or
    /// cleared.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The absolute index of the oldest entry kept, which is
    /// [`dropped`](EventLog::dropped): an entry's absolute index is this
    /// plus its place in the ring.
    pub fn first(&self) -> u64 {
        self.dropped
    }

    /// How many entries are kept.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entry is kept.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries the ring keeps, or `None` for every one.
    pub fn capacity(&self) -> Option<usize> {
        self.capacity
    }

    /// A kept entry by its absolute index.
    pub fn get(&self, index: u64) -> Option<&Logged> {
        let at = index.checked_sub(self.dropped)?;
        self.entries.get(usize::try_from(at).ok()?)
    }

    /// The kept entries, oldest first.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, Logged> {
        self.entries.iter()
    }

    /// The absolute index of the first kept entry at or after the host's
    /// position `seq`, or `None` when every kept entry is before it: the
    /// entry a bin's log range starts at.
    pub fn position_at(&self, seq: u64) -> Option<u64> {
        position_at(&self.entries, self.dropped, seq)
    }

    /// Drops every entry kept and sets the base to `standing`, the counts
    /// and flights that stand now, naming no token. The capacity and the
    /// start stay, and the entries cleared count as dropped, so that an
    /// absolute index is never used twice and an [`EventPlayer`] following
    /// the log sees them go.
    pub fn clear(&mut self, standing: Standing) {
        self.dropped += self.entries.len() as u64;
        self.entries.clear();
        self.held = 0;
        self.base = standing;
        self.tokens = Roster::default();
    }

    /// The standing state the oldest kept entry starts from: the state the
    /// log began on, advanced by every entry dropped since.
    pub fn base(&self) -> &Standing {
        &self.base
    }

    /// When the log began, in milliseconds since the Unix epoch, or `None`
    /// for an undated log. Every entry's [`at_ms`](Logged::at_ms) is from
    /// here.
    pub fn start_unix_ms(&self) -> Option<u64> {
        self.start_unix_ms
    }

    /// The file name for a log of the net `net`:
    /// `<net>-<start>.events.json`, the start as `YYYYMMDD-HHMMSS` in UTC, or
    /// `undated`, by the rule [`Record::file_name`](crate::Record::file_name)
    /// follows, so that a record and a log begun at the same moment share a
    /// stem.
    pub fn file_name(&self, net: &str) -> String {
        format!("{}.events.json", file_stem(net, self.start_unix_ms))
    }
}

#[cfg(feature = "serde")]
impl EventLog {
    /// Writes the log as JSON, one value in one line holding the capacity,
    /// the start, the base, how many entries were dropped and the entries:
    /// a log is large, so it is not pretty printed. The writer is buffered
    /// here.
    pub fn write_json(&self, w: impl Write) -> io::Result<()> {
        let mut w = io::BufWriter::new(w);
        serde_json::to_writer(&mut w, self)?;
        w.flush()
    }

    /// Reads a log written by [`write_json`](EventLog::write_json). The
    /// reader is buffered here.
    pub fn read_json(r: impl io::Read) -> io::Result<EventLog> {
        let mut log: EventLog = serde_json::from_reader(io::BufReader::new(r))?;
        log.held = log.entries.iter().map(Logged::bytes).sum();
        Ok(log)
    }
}

/// The absolute index of the first of `entries`, the first of which has
/// absolute index `first`, at or after position `seq`.
fn position_at(entries: &VecDeque<Logged>, first: u64, seq: u64) -> Option<u64> {
    let at = entries.partition_point(|logged| logged.seq < seq);
    (at < entries.len()).then(|| first + at as u64)
}

/// The absolute index of the last of `entries`, the first of which has
/// absolute index `first`, at or before position `seq`.
fn index_at(entries: &VecDeque<Logged>, first: u64, seq: u64) -> Option<u64> {
    let after = entries.partition_point(|logged| logged.seq <= seq);
    (after > 0).then(|| first + after as u64 - 1)
}

/// A replay of an [`EventLog`] by entry rather than by time: each entry is
/// shown for a fixed `dwell`, so that an observer reads the order in which
/// things happened, whatever the real gaps between them were.
///
/// [`at`](EventPlayer::at) is the model after the entry at an absolute
/// index, folded from a fresh model on the log's base: for each entry, the
/// model [ticks](Activity::tick) by the dwell, except before the log's very
/// first entry, and then [applies](Activity::apply_entry) it. So the entry
/// shown is at full brightness and those before it have decayed by whole
/// dwells on the wall clock. The firings clock moves by the firings in the
/// entries alone, never by the dwell.
///
/// The fold starts from the nearest checkpoint at or before the index, so a
/// jump anywhere costs at most
/// [`checkpoint_every`](EventPlayer::checkpoint_every) entries; the
/// checkpoints are made on the first pass and dropped when the settings or
/// the dwell change. A player holds a copy of the log's entries, and
/// [`follow`](EventPlayer::follow) brings it up to the log as it grows and
/// drops, its entries keeping their absolute indices. A checkpoint made
/// before entries dropped keeps the levels they left, which a fold from the
/// new base, on counts alone, does not have.
///
/// Beside the levels it folds the log's [`Roster`]
/// ([`tokens_at`](EventPlayer::tokens_at)): the tokens named in each place
/// after an entry and how each looked, with checkpoints of its own. Before
/// the first entry held it stands at the log's base
/// ([`before_first`](EventPlayer::before_first),
/// [`tokens_before_first`](EventPlayer::tokens_before_first)), which is where
/// a moment before every entry ([`index_at`](EventPlayer::index_at) `None`)
/// shows it.
#[derive(Debug, Clone)]
pub struct EventPlayer {
    grid: Grid,
    entries: VecDeque<Logged>,
    /// The absolute index of the first entry held.
    first: u64,
    base: Standing,
    settings: ActivitySettings,
    dwell: Duration,
    every: usize,
    /// The model before the entry of each absolute index, folded.
    checkpoints: BTreeMap<u64, Activity>,
    /// The model after the entry of an absolute index, as `at` last left it.
    current: Option<(u64, Activity)>,
    /// The model before the first entry held: the base's counts and flights,
    /// every level at 0.
    fresh: Activity,
    /// The tokens named before the first entry held.
    tokens: Roster,
    /// The roster before the entry of each absolute index, folded.
    token_checkpoints: BTreeMap<u64, Roster>,
    /// The roster after the entry of an absolute index, as `tokens_at` last
    /// left it.
    token_current: Option<(u64, Roster)>,
}

impl EventPlayer {
    /// How many entries apart the checkpoints are unless a host says.
    pub const CHECKPOINT_EVERY: usize = 256;

    /// A player of a copy of `log`'s entries, on the nodes of `grid`, run by
    /// `settings`, each entry shown for `dwell`, with a checkpoint every
    /// `checkpoint_every` entries (at least one).
    pub fn new(
        log: &EventLog,
        grid: &Grid,
        settings: ActivitySettings,
        dwell: Duration,
        checkpoint_every: usize,
    ) -> Self {
        Self {
            fresh: log.base.model_on(grid, settings.clone()),
            grid: grid.clone(),
            entries: log.entries.clone(),
            first: log.first(),
            base: log.base.clone(),
            settings,
            dwell,
            every: checkpoint_every.max(1),
            checkpoints: BTreeMap::new(),
            current: None,
            tokens: log.tokens.clone(),
            token_checkpoints: BTreeMap::new(),
            token_current: None,
        }
    }

    /// The model before the first entry held: the log's base, its counts
    /// and flights, every level at 0. A moment before every entry held
    /// shows it.
    pub fn before_first(&self) -> &Activity {
        &self.fresh
    }

    /// The tokens named before the first entry held, with their looks: the
    /// log's [`base_tokens`](EventLog::base_tokens).
    pub fn tokens_before_first(&self) -> &Roster {
        &self.tokens
    }

    /// The tokens named after the entry at absolute index `index`, clamped
    /// to the entries held, with how each looked then; the roster before the
    /// first entry when none is held. Folded from the nearest checkpoint, as
    /// [`at`](EventPlayer::at) folds the levels.
    pub fn tokens_at(&mut self, index: u64) -> &Roster {
        if self.entries.is_empty() {
            return &self.tokens;
        }
        let last = self.first + self.entries.len() as u64 - 1;
        let target = index.clamp(self.first, last);
        let from_current = self
            .token_current
            .as_ref()
            .is_some_and(|(after, _)| *after <= target && *after + 1 >= self.first);
        let checkpoint = self
            .token_checkpoints
            .range(self.first..=target + 1)
            .next_back()
            .map(|(&before, _)| before);
        let current_next = self.token_current.as_ref().map_or(0, |c| c.0 + 1);
        let (mut next, mut roster) = match (from_current, checkpoint) {
            (true, Some(before)) if before > current_next => {
                (before, self.token_checkpoints[&before].clone())
            }
            (true, _) => {
                let (after, roster) = self.token_current.take().expect("checked above");
                (after + 1, roster)
            }
            (false, Some(before)) => (before, self.token_checkpoints[&before].clone()),
            (false, None) => (self.first, self.tokens.clone()),
        };
        while next <= target {
            roster.advance(&self.entries[(next - self.first) as usize]);
            next += 1;
            if next % self.every as u64 == 0 && !self.token_checkpoints.contains_key(&next) {
                self.token_checkpoints.insert(next, roster.clone());
            }
        }
        &self.token_current.insert((target, roster)).1
    }

    /// The absolute index of the last entry held at or before the host's
    /// position `seq`, as [`EventLog::index_at`] finds it: `None` when every
    /// entry held is after it, where the player stands
    /// [`before_first`](EventPlayer::before_first).
    pub fn index_at(&self, seq: u64) -> Option<u64> {
        index_at(&self.entries, self.first, seq)
    }

    /// The model after the entry at absolute index `index`, clamped to the
    /// entries held; the fresh model on the base when there is none.
    pub fn at(&mut self, index: u64) -> &Activity {
        if self.entries.is_empty() {
            return &self.fresh;
        }
        let last = self.first + self.entries.len() as u64 - 1;
        let target = index.clamp(self.first, last);
        // Where to fold from: the model as `at` last left it, if it stands
        // at or before the target with every entry after it held; else the
        // nearest checkpoint; else the fresh model at the first entry.
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
        while next <= target {
            let logged = &self.entries[(next - self.first) as usize];
            if next > 0 {
                activity.tick(self.dwell);
            }
            activity.apply_entry(&logged.entry);
            next += 1;
            if next % self.every as u64 == 0 && !self.checkpoints.contains_key(&next) {
                self.checkpoints.insert(next, activity.clone());
            }
        }
        &self.current.insert((target, activity)).1
    }

    /// Brings the entries up to `log`'s: those it took since are added, and
    /// those it dropped are dropped here too, with the checkpoints before
    /// them, the base taken from the log. The entries held keep their
    /// absolute indices. A log whose first entry is older than the player's,
    /// another log, gives its entries afresh.
    pub fn follow(&mut self, log: &EventLog) {
        let (theirs, end) = (log.first(), log.first() + log.len() as u64);
        if theirs < self.first {
            self.entries = log.entries.clone();
            self.first = theirs;
            self.rebase(log.base.clone());
            self.tokens = log.tokens.clone();
            self.checkpoints.clear();
            self.current = None;
            self.token_checkpoints.clear();
            self.token_current = None;
            return;
        }
        if theirs > self.first {
            let gone = (theirs - self.first).min(self.entries.len() as u64) as usize;
            self.entries.drain(..gone);
            self.first = theirs;
            self.rebase(log.base.clone());
            self.tokens = log.tokens.clone();
            let first = self.first;
            self.checkpoints.retain(|&before, _| before >= first);
            self.token_checkpoints.retain(|&before, _| before >= first);
            if self
                .current
                .as_ref()
                .is_some_and(|(after, _)| *after + 1 < first)
            {
                self.current = None;
            }
            if self
                .token_current
                .as_ref()
                .is_some_and(|(after, _)| *after + 1 < first)
            {
                self.token_current = None;
            }
        }
        let have = self.first + self.entries.len() as u64;
        for index in have..end {
            if let Some(logged) = log.get(index) {
                self.entries.push_back(logged.clone());
            }
        }
    }

    /// A kept entry by its absolute index.
    pub fn get(&self, index: u64) -> Option<&Logged> {
        let at = index.checked_sub(self.first)?;
        self.entries.get(usize::try_from(at).ok()?)
    }

    /// The absolute index of the first entry held at or after the host's
    /// position `seq`, as [`EventLog::position_at`] finds it.
    pub fn position_at(&self, seq: u64) -> Option<u64> {
        position_at(&self.entries, self.first, seq)
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The absolute index of the first entry held.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// The settings the entries are folded under.
    pub fn settings(&self) -> &ActivitySettings {
        &self.settings
    }

    /// How long each entry is shown: the wall time ticked between two.
    pub fn dwell(&self) -> Duration {
        self.dwell
    }

    /// How many entries apart the checkpoints are.
    pub fn checkpoint_every(&self) -> usize {
        self.every
    }

    /// Folds under new settings from now on: a change drops every
    /// checkpoint, which the next pass makes again.
    pub fn set_settings(&mut self, settings: ActivitySettings) {
        if settings != self.settings {
            self.settings = settings;
            self.refold();
        }
    }

    /// Shows each entry for `dwell` from now on: a change drops every
    /// checkpoint, which the next pass makes again.
    pub fn set_dwell(&mut self, dwell: Duration) {
        if dwell != self.dwell {
            self.dwell = dwell;
            self.refold();
        }
    }

    /// The fresh model on a new base.
    fn rebase(&mut self, base: Standing) {
        self.fresh = base.model_on(&self.grid, self.settings.clone());
        self.base = base;
    }

    /// Every fold made so far dropped, and the fresh model made again.
    fn refold(&mut self) {
        self.checkpoints.clear();
        self.current = None;
        self.fresh = self.base.model_on(&self.grid, self.settings.clone());
    }
}

#[cfg(test)]
mod tests {
    use petritype::{MutateEdge, Net, NetBuilder, NetId, TakeAllEdge, TakeEdge};
    use petritype_layout::Layout;

    use super::*;
    use crate::settings::Clock;
    use crate::traffic_light::build_traffic_light;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn drain(net: &mut Net, events: &mut Vec<Event>) {
        events.extend(net.drain_events());
    }

    /// The grid of a net with no node.
    fn empty_grid() -> Grid {
        grid_of(&NetBuilder::new().build().expect("an empty net"))
    }

    fn grid_of(net: &Net) -> Grid {
        let mut layout = Layout::default();
        layout.compute(net);
        Grid::from_layout(&layout, &net.topology())
    }

    /// Every level, count and state of every node, for comparing two
    /// models whose accounts may differ.
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

    /// A run of a net: the model at its start, read off the net, and the
    /// events it logged after.
    struct Run {
        start: Activity,
        grid: Grid,
        events: Vec<Event>,
    }

    /// The traffic light stepped, with cars from outside: fires, adds, and
    /// a light that loops through its transition.
    fn traffic_run() -> Run {
        let mut light = build_traffic_light().expect("it builds");
        let start = Activity::from_net(&light.net, ActivitySettings::default());
        let grid = grid_of(&light.net);
        let mut events = Vec::new();
        for frame in 0..12 {
            if frame % 3 == 2 {
                light.net.put(light.waiting, format!("Car {frame}"));
            } else {
                light.net.step().expect("a step");
            }
            drain(&mut light.net, &mut events);
        }
        Run {
            start,
            grid,
            events,
        }
    }

    /// A parcel line after petritype's parcel distribution example, its
    /// weights as tokens, run by launches and distributes: `Sort` and
    /// `Stage` in turn, a truck that takes the whole staging area, leaves
    /// every fourth parcel for the next and counts its loads in a fleet it
    /// changes in place, an inspection that refuses a heavy parcel and puts
    /// it back, and a scale that loses one.
    fn parcel_run() -> Run {
        let mut b = NetBuilder::new();
        let receiving = b.place::<u32>("Receiving");
        let sorting = b.place::<u32>("Sorting");
        let staging = b.place::<u32>("Staging Area");
        let dispatched = b.place::<u32>("Dispatched");
        let fleet = b.place::<u32>("Fleet");
        let inspection = b.place::<u32>("Inspection");
        let scale = b.place::<u32>("Scale");
        b.seed(receiving, [5, 12, 30, 8]);
        b.seed(fleet, [0]);
        let sort = b
            .transition("Sort", (TakeEdge(receiving),), (sorting,), |p: u32| (p,))
            .id();
        let stage = b
            .transition("Stage", (TakeEdge(sorting),), (staging,), |p: u32| (p,))
            .id();
        let dispatch = b
            .transition(
                "Dispatch Truck",
                (TakeAllEdge(staging), MutateEdge(fleet)),
                (dispatched, staging),
                |parcels: Vec<u32>, loads: &mut u32| {
                    *loads += 1;
                    let (left, gone): (Vec<u32>, Vec<u32>) =
                        parcels.into_iter().partition(|p| p % 4 == 0);
                    (gone, left)
                },
            )
            .id();
        let refuse = |p: u32| -> Result<(u32,), std::fmt::Error> {
            if p > 100 {
                Err(std::fmt::Error)
            } else {
                Ok((p,))
            }
        };
        let inspect = b
            .transition("Inspect", (TakeEdge(inspection),), (receiving,), refuse)
            .restore_tokens_on_failure()
            .id();
        let weigh = b
            .transition("Weigh", (TakeEdge(scale),), (receiving,), refuse)
            .id();
        let mut net = b.build().expect("the net builds");
        let start = Activity::from_net(&net, ActivitySettings::default());
        let grid = grid_of(&net);
        let mut events = Vec::new();
        let staged = |net: &mut Net, events: &mut Vec<Event>, t: TransitionId| {
            let firing = net.launch(t).expect("it launches");
            drain(net, events);
            let _ = net.distribute(firing.run());
            drain(net, events);
        };
        net.fire(sort).expect("Sort fires");
        drain(&mut net, &mut events);
        staged(&mut net, &mut events, stage);
        staged(&mut net, &mut events, sort);
        staged(&mut net, &mut events, sort);
        staged(&mut net, &mut events, stage);
        staged(&mut net, &mut events, stage);
        staged(&mut net, &mut events, dispatch);
        let heavy = net.put(inspection, 150);
        net.put(inspection, 20);
        drain(&mut net, &mut events);
        // Refused and put back, staged and in one go.
        staged(&mut net, &mut events, inspect);
        net.fire(inspect).expect_err("refused");
        drain(&mut net, &mut events);
        net.remove(inspection.id(), heavy)
            .expect("the heavy one goes");
        net.fire(inspect).expect("the light one passes");
        drain(&mut net, &mut events);
        // Lost, staged and in one go.
        net.put(scale, 500);
        drain(&mut net, &mut events);
        staged(&mut net, &mut events, weigh);
        net.put(scale, 300);
        net.fire(weigh).expect_err("lost");
        drain(&mut net, &mut events);
        Run {
            start,
            grid,
            events,
        }
    }

    /// The entries of a run, with the corrections a host makes after a
    /// resync: a count set, a transition put in flight with no launch
    /// seen, its failure, and a flight set and cleared.
    fn entries_of(run: &Run) -> Vec<Entry> {
        let mut entries: Vec<Entry> = run.events.iter().filter_map(Entry::from_event).collect();
        let place = PlaceId::new(0);
        let transition = TransitionId::new(0);
        entries.extend([
            Entry::Count { place, count: 9 },
            Entry::InFlight {
                transition,
                on: true,
            },
            Entry::Failed {
                transition,
                consumed: vec![(place, TokenId::new(999))],
                restored: false,
                error: "lost at a resync".to_owned(),
            },
            Entry::InFlight {
                transition,
                on: true,
            },
            Entry::Failed {
                transition,
                consumed: vec![(place, TokenId::new(999))],
                restored: true,
                error: "back at a resync".to_owned(),
            },
            Entry::InFlight {
                transition,
                on: true,
            },
            Entry::InFlight {
                transition,
                on: false,
            },
            Entry::Count { place, count: 0 },
        ]);
        entries
    }

    #[test]
    fn an_entry_is_made_from_every_event_the_model_acts_on_and_none_from_the_rest() {
        let (p, t, k) = (PlaceId::new(1), TransitionId::new(2), TokenId::new(3));
        let pairs = [
            (
                Event::Fired {
                    transition: t,
                    consumed: vec![(p, k)],
                    produced: vec![(p, TokenId::new(4))],
                },
                Entry::Fired {
                    transition: t,
                    consumed: vec![(p, k)],
                    produced: vec![(p, TokenId::new(4))],
                },
            ),
            (
                Event::Launched {
                    transition: t,
                    consumed: vec![(p, k)],
                },
                Entry::Launched {
                    transition: t,
                    consumed: vec![(p, k)],
                },
            ),
            (
                Event::Distributed {
                    transition: t,
                    produced: vec![(p, k)],
                },
                Entry::Distributed {
                    transition: t,
                    produced: vec![(p, k)],
                },
            ),
            (
                Event::Failed {
                    transition: t,
                    consumed: vec![(p, k)],
                    restored: true,
                    error: "no".to_owned(),
                },
                Entry::Failed {
                    transition: t,
                    consumed: vec![(p, k)],
                    restored: true,
                    error: "no".to_owned(),
                },
            ),
            (
                Event::Added { place: p, token: k },
                Entry::Added { place: p, token: k },
            ),
            (
                Event::Removed { place: p, token: k },
                Entry::Removed { place: p, token: k },
            ),
            (
                Event::Mutated { place: p, token: k },
                Entry::Mutated { place: p, token: k },
            ),
        ];
        for (event, entry) in &pairs {
            assert_eq!(Entry::from_event(event).as_ref(), Some(entry), "{event:?}");
        }
        let nested_id: NetId = NetBuilder::new().build().expect("an empty net").id();
        let ignored = [
            Event::Toggled {
                transition: t,
                enabled: false,
            },
            Event::ParamChanged { type_name: "u32" },
            Event::Resynced,
            Event::Nested {
                place: p,
                token: k,
                net: String::new(),
                nested_id,
                event: Box::new(pairs[0].0.clone()),
            },
        ];
        for event in &ignored {
            assert_eq!(Entry::from_event(event), None, "{event:?}");
        }
    }

    #[test]
    fn a_model_fed_entries_equals_the_model_fed_events() {
        for run in [traffic_run(), parcel_run()] {
            let (mut by_event, mut by_entry) = (run.start.clone(), run.start.clone());
            assert!(run.events.iter().any(|e| matches!(e, Event::Fired { .. })));
            for event in &run.events {
                by_event.apply(event);
                if let Some(entry) = Entry::from_event(event) {
                    by_entry.apply_entry(&entry);
                }
                assert_eq!(by_entry, by_event, "{event:?}");
                by_event.tick(ms(70));
                by_entry.tick(ms(70));
            }
        }
        // The parcel run has every kind of firing and outside change.
        let kinds: Vec<&str> = parcel_run()
            .events
            .iter()
            .map(|e| match e {
                Event::Fired { .. } => "fired",
                Event::Launched { .. } => "launched",
                Event::Distributed { .. } => "distributed",
                Event::Failed { restored: true, .. } => "restored",
                Event::Failed { .. } => "lost",
                Event::Added { .. } => "added",
                Event::Removed { .. } => "removed",
                Event::Mutated { .. } => "mutated",
                _ => "other",
            })
            .collect();
        for kind in [
            "fired",
            "launched",
            "distributed",
            "restored",
            "lost",
            "added",
            "removed",
            "mutated",
        ] {
            assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
        }
    }

    #[test]
    fn the_corrections_set_the_count_and_the_flight_with_no_hit() {
        let run = traffic_run();
        let mut a = run.start.clone();
        let (place, transition) = (PlaceId::new(1), TransitionId::new(0));
        a.apply_entry(&Entry::Count { place, count: 4 });
        let p = a.place(place).unwrap();
        assert_eq!((p.count, p.pulse, p.ember), (4, 0.0, 0.0));
        a.apply_entry(&Entry::InFlight {
            transition,
            on: true,
        });
        let t = a.transition(transition).unwrap();
        assert_eq!((t.in_flight, t.pulse, t.ember), (true, 1.0, 0.0));
        assert_eq!(a.firings(), 0, "no firing");
        a.apply_entry(&Entry::InFlight {
            transition,
            on: false,
        });
        assert!(!a.transition(transition).unwrap().in_flight);
    }

    #[test]
    fn the_standing_state_advances_as_the_model_does_with_and_without_a_ring_that_drops() {
        for run in [traffic_run(), parcel_run()] {
            let entries = entries_of(&run);
            let mut live = run.start.clone();
            let mut standing = live.standing_state();
            assert_eq!(standing.to_delta(), live.standing(), "the walk is shared");
            let mut states = vec![standing.clone()];
            let mut ring = EventLog::new(Some(3), None, live.standing_state());
            let mut all = EventLog::new(None, None, live.standing_state());
            for (seq, entry) in entries.iter().enumerate() {
                live.apply_entry(entry);
                standing.advance(entry);
                assert_eq!(standing, live.standing_state(), "{entry:?}");
                states.push(standing.clone());
                ring.push(entry.clone(), seq as u64, ms(seq as u64));
                all.push(entry.clone(), seq as u64, ms(seq as u64));
                // The ring's base is the state its oldest kept entry starts
                // from, and the base advanced by what is kept is the state
                // that stands.
                assert_eq!(ring.base(), &states[ring.first() as usize]);
                let mut kept = ring.base().clone();
                for logged in ring.iter() {
                    kept.advance(&logged.entry);
                }
                assert_eq!(kept, live.standing_state());
            }
            assert_eq!(all.dropped(), 0);
            assert_eq!(all.base(), &states[0]);
            assert!(ring.dropped() > 0);
        }
    }

    #[test]
    fn the_ring_drops_past_its_capacity_and_a_player_on_it_starts_from_the_base() {
        let run = parcel_run();
        let entries = entries_of(&run);
        let mut live = run.start.clone();
        let mut log = EventLog::new(Some(4), None, live.standing_state());
        let mut after = Vec::new();
        for (seq, entry) in entries.iter().take(10).enumerate() {
            live.apply_entry(entry);
            after.push(live.standing_state());
            log.push(entry.clone(), seq as u64, ms(10 * seq as u64));
        }
        assert_eq!((log.len(), log.dropped(), log.first()), (4, 6, 6));
        assert_eq!(log.capacity(), Some(4));
        assert!(log.get(5).is_none() && log.get(10).is_none());
        assert_eq!(log.get(6).map(|l| l.seq), Some(6));
        assert_eq!(log.iter().map(|l| l.seq).collect::<Vec<_>>(), [6, 7, 8, 9]);
        assert_eq!(
            log.base(),
            &after[5],
            "the state the oldest kept starts from"
        );

        let mut player = EventPlayer::new(
            &log,
            &run.grid,
            ActivitySettings::default(),
            ms(300),
            EventPlayer::CHECKPOINT_EVERY,
        );
        assert_eq!((player.first(), player.len()), (6, 4));
        // At its first index the player stands on the base, moved on by
        // that entry, and so at every index after.
        for index in 6..10 {
            assert_eq!(
                player.at(index).standing_state(),
                after[index as usize],
                "{index}"
            );
        }
        assert_eq!(player.at(0).standing_state(), after[6], "clamped");
        // A ring of one keeps one, and an empty log plays its base.
        let empty = EventLog::new(Some(0), None, after[3].clone());
        assert_eq!(empty.capacity(), Some(1));
        let mut player = EventPlayer::new(&empty, &run.grid, ActivitySettings::default(), ms(1), 1);
        assert_eq!(player.at(5).standing_state(), after[3]);
    }

    #[test]
    fn position_at_finds_the_first_entry_at_or_after_a_position() {
        let mut log = EventLog::new(Some(4), None, Standing::default());
        let entry = Entry::Mutated {
            place: PlaceId::new(0),
            token: TokenId::new(0),
        };
        for seq in [1, 2, 3, 3, 5, 8] {
            log.push(entry.clone(), seq, Duration::ZERO);
        }
        // Kept: 3, 3, 5 and 8 at absolute indices 2 to 5.
        assert_eq!(log.first(), 2);
        assert_eq!(log.position_at(0), Some(2), "before the oldest kept");
        assert_eq!(log.position_at(3), Some(2));
        assert_eq!(log.position_at(4), Some(4));
        assert_eq!(log.position_at(8), Some(5));
        assert_eq!(log.position_at(9), None, "past the end");
        let player = EventPlayer::new(&log, &empty_grid(), ActivitySettings::default(), ms(1), 1);
        assert_eq!(player.position_at(4), Some(4));
        assert_eq!(player.get(4).map(|l| l.seq), Some(5));
    }

    /// The model after the entry at `index` of `log`, folded straight from
    /// its base with no checkpoint.
    fn straight(
        log: &EventLog,
        grid: &Grid,
        settings: &ActivitySettings,
        dwell: Duration,
        index: u64,
    ) -> Activity {
        let mut a = log.base().model_on(grid, settings.clone());
        for k in log.first()..=index {
            if k > 0 {
                a.tick(dwell);
            }
            a.apply_entry(&log.get(k).expect("kept").entry);
        }
        a
    }

    #[test]
    fn a_player_at_any_index_equals_a_straight_fold_with_the_same_dwell() {
        let run = traffic_run();
        let mut log = EventLog::new(None, None, run.start.standing_state());
        for (seq, entry) in entries_of(&run).into_iter().enumerate() {
            log.push(entry, seq as u64, ms(1000 * seq as u64));
        }
        let n = log.len() as u64;
        assert!(n > 12, "{n}");
        let settings = ActivitySettings::default();
        let dwell = ms(300);
        let mut player = EventPlayer::new(&log, &run.grid, settings.clone(), dwell, 4);
        assert_eq!((player.dwell(), player.checkpoint_every()), (dwell, 4));
        for index in [n - 1, 3, 3, 4, 9, 0, 7, 6, n - 2, 8, 1] {
            let fold = straight(&log, &run.grid, &settings, dwell, index);
            let a = player.at(index);
            assert_eq!(
                (levels(a), a.freshness()),
                (levels(&fold), fold.freshness()),
                "{index}"
            );
            assert_eq!(a.firings(), fold.firings());
        }
        // A new dwell or new settings fold anew.
        player.set_dwell(ms(50));
        let fold = straight(&log, &run.grid, &settings, ms(50), 9);
        assert_eq!(levels(player.at(9)), levels(&fold));
        let mut firings = settings.clone();
        firings.pulse.clock = Clock::Firings;
        player.set_settings(firings.clone());
        let fold = straight(&log, &run.grid, &firings, ms(50), 9);
        assert_eq!(levels(player.at(9)), levels(&fold));
        assert_eq!(player.settings(), &firings);
    }

    #[test]
    fn between_two_entries_the_wall_layers_decay_by_one_dwell_and_the_firings_clock_by_none() {
        let mut light = build_traffic_light().expect("it builds");
        let grid = grid_of(&light.net);
        let start = Activity::from_net(&light.net, ActivitySettings::default());
        let mut events = Vec::new();
        light.net.step().expect("Change fires");
        light.net.put(light.waiting, "Taxi".to_owned());
        drain(&mut light.net, &mut events);
        let mut log = EventLog::new(None, None, start.standing_state());
        // A minute apart on the wall clock, which the replay does not see.
        for (seq, event) in events.iter().enumerate() {
            let entry = Entry::from_event(event).expect("an entry");
            log.push(entry, seq as u64, Duration::from_secs(60 * seq as u64));
        }
        assert_eq!(log.len(), 2);
        let change = light.change;
        let dwell = ms(150);
        let wall = ActivitySettings::default();
        let mut player = EventPlayer::new(&log, &grid, wall.clone(), dwell, 256);
        let shown = player.at(0).transition(change).unwrap();
        assert_eq!(shown.pulse, 1.0, "the entry shown is at full brightness");
        let next = player.at(1).transition(change).unwrap();
        let dt = dwell.as_secs_f32();
        assert!((next.pulse - shown.pulse * wall.pulse.decay(Clock::Wall, dt)).abs() < 1e-6);
        assert!((next.ember - shown.ember * wall.ember.decay(Clock::Wall, dt)).abs() < 1e-6);
        assert_eq!(player.at(1).firings(), 1, "the car is no firing");

        let mut firings = ActivitySettings::default();
        firings.pulse.clock = Clock::Firings;
        firings.ember.clock = Clock::Firings;
        player.set_settings(firings);
        let shown = player.at(0).transition(change).unwrap();
        let next = player.at(1).transition(change).unwrap();
        assert_eq!((next.pulse, next.ember), (shown.pulse, shown.ember));
        let lamp = light.light.id();
        assert_eq!(
            player.at(1).place(lamp).map(|p| p.pulse),
            player.at(0).place(lamp).map(|p| p.pulse)
        );
    }

    #[test]
    fn a_player_follows_a_log_as_it_grows_drops_and_is_cleared() {
        let run = parcel_run();
        let entries = entries_of(&run);
        let mut live = run.start.clone();
        let mut log = EventLog::new(Some(6), None, live.standing_state());
        let settings = ActivitySettings::default();
        let push = |log: &mut EventLog, live: &mut Activity, k: usize| {
            live.apply_entry(&entries[k]);
            log.push(entries[k].clone(), k as u64, ms(k as u64));
        };
        for k in 0..4 {
            push(&mut log, &mut live, k);
        }
        let mut player = EventPlayer::new(&log, &run.grid, settings.clone(), ms(100), 2);
        player.at(3);
        for k in 4..15 {
            push(&mut log, &mut live, k);
        }
        player.follow(&log);
        assert_eq!((player.first(), player.len()), (9, 6));
        assert_eq!(
            (9..15).map(|i| player.get(i).cloned()).collect::<Vec<_>>(),
            log.iter().cloned().map(Some).collect::<Vec<_>>()
        );
        assert_eq!(player.at(14).standing_state(), live.standing_state());
        // Cleared: every entry gone, the base the state that stands, and
        // the indices go on.
        log.clear(live.standing_state());
        assert_eq!((log.len(), log.first()), (0, 15));
        push(&mut log, &mut live, 15);
        player.follow(&log);
        assert_eq!((player.first(), player.len()), (15, 1));
        assert_eq!(player.at(15).standing_state(), live.standing_state());
        // Another log, begun later than this one dropped to, starts afresh.
        let other = EventLog::new(None, None, Standing::default());
        player.follow(&other);
        assert!(player.is_empty());
        assert_eq!(player.first(), 0);
    }

    #[test]
    fn the_standing_state_worked_back_through_entries_is_the_one_they_started_from() {
        for run in [traffic_run(), parcel_run()] {
            // Every run of entries, from each start, ending anywhere: the
            // state after them, worked back, is the state they began on.
            // The corrections at the end name counts the run did not lead
            // to, so they are left out here and tried on their own below.
            let entries: Vec<Entry> = run.events.iter().filter_map(Entry::from_event).collect();
            let mut states = vec![run.start.standing_state()];
            for entry in &entries {
                let mut next = states.last().expect("a state").clone();
                next.advance(entry);
                states.push(next);
            }
            // A run that starts with a firing in flight cannot tell a
            // failure of it from one in one go, which it reads it as; so
            // every start with nothing in flight.
            let mut tried = 0;
            for from in (0..entries.len()).filter(|&from| states[from].in_flight.is_empty()) {
                for to in from..=entries.len() {
                    assert_eq!(
                        states[to].worked_back(&entries[from..to]),
                        states[from],
                        "{from}..{to}"
                    );
                    tried += 1;
                }
            }
            assert!(tried > entries.len(), "{tried}");
        }
        // A correction of a flight was the other way before; one of a count
        // keeps the count.
        let (place, transition) = (PlaceId::new(0), TransitionId::new(0));
        let after = Standing {
            counts: vec![(place, 3)],
            in_flight: vec![transition],
        };
        let back = after.worked_back(&[
            Entry::Count { place, count: 3 },
            Entry::InFlight {
                transition,
                on: true,
            },
        ]);
        assert_eq!(
            back,
            Standing {
                counts: vec![(place, 3)],
                in_flight: vec![]
            }
        );
    }

    #[test]
    fn index_at_finds_the_last_entry_at_or_before_a_moment_and_none_before_the_first() {
        let mut log = EventLog::new(Some(4), None, Standing::default());
        let entry = Entry::Mutated {
            place: PlaceId::new(0),
            token: TokenId::new(0),
        };
        for seq in [1, 2, 3, 3, 5, 8] {
            log.push(entry.clone(), seq, Duration::ZERO);
        }
        // Kept: 3, 3, 5 and 8 at absolute indices 2 to 5.
        assert_eq!(log.index_at(2), None, "before the oldest kept");
        assert_eq!(log.index_at(3), Some(3), "the last of two at a moment");
        assert_eq!(log.index_at(4), Some(3));
        assert_eq!(log.index_at(7), Some(4));
        assert_eq!(log.index_at(100), Some(5));
        let player = EventPlayer::new(&log, &empty_grid(), ActivitySettings::default(), ms(1), 1);
        assert_eq!(player.index_at(5), Some(4));
        assert_eq!(player.index_at(0), None);
        assert_eq!(
            EventLog::new(None, None, Standing::default()).index_at(9),
            None
        );
    }

    #[test]
    fn a_player_names_the_tokens_of_each_place_at_any_entry_with_the_looks_they_had() {
        let run = parcel_run();
        let looked = |token: TokenId| Look::new(&format!("token {}", token.get()), None, 200);
        // The roster at the start names the parcels received, as a host
        // reads them off the net.
        let mut start = Roster::default();
        start.set_place(
            PlaceId::new(0),
            (0..4).map(|n| (TokenId::new(n), Some(looked(TokenId::new(n))))),
        );
        let mut log =
            EventLog::new(Some(5), None, run.start.standing_state()).with_tokens(start.clone());
        let mut rosters = vec![start];
        for (seq, event) in run.events.iter().enumerate() {
            let Some(entry) = Entry::from_event(event) else {
                continue;
            };
            let put: Vec<TokenId> = match &entry {
                Entry::Fired { produced, .. } | Entry::Distributed { produced, .. } => {
                    produced.iter().map(|(_, token)| *token).collect()
                }
                Entry::Added { token, .. } | Entry::Mutated { token, .. } => vec![*token],
                _ => Vec::new(),
            };
            let looks: Vec<(TokenId, Look)> = put.into_iter().map(|t| (t, looked(t))).collect();
            let mut next = rosters.last().expect("a roster").clone();
            next.advance(&Logged {
                seq: seq as u64,
                at_ms: 0,
                entry: entry.clone(),
                looks: looks.clone(),
            });
            rosters.push(next);
            log.push_with(entry, seq as u64, ms(seq as u64), looks);
        }
        assert!(log.dropped() > 0, "the ring dropped entries");
        assert_eq!(
            log.base_tokens(),
            &rosters[log.first() as usize],
            "the base roster advanced"
        );
        let mut player = EventPlayer::new(&log, &run.grid, ActivitySettings::default(), ms(10), 2);
        assert_eq!(player.tokens_before_first(), &rosters[log.first() as usize]);
        for index in (log.first()..log.first() + log.len() as u64).rev() {
            assert_eq!(
                player.tokens_at(index),
                &rosters[index as usize + 1],
                "{index}"
            );
        }
        // The parcels sorted: names and looks moved with them.
        let last = player.tokens_at(u64::MAX).clone();
        assert!(last.places().count() > 0);
        for (_, tokens) in last.places() {
            for token in tokens {
                assert_eq!(last.look(*token), Some(&looked(*token)));
            }
        }
        assert!(player.before_first().place_count() > 0);
    }

    #[test]
    fn a_logs_bytes_count_its_entries_and_their_labels_and_follow_the_ring() {
        // A change of a token the base names none of: what drops leaves
        // the base's counts and roster as they were.
        let entry = || Entry::Mutated {
            place: PlaceId::new(0),
            token: TokenId::new(1),
        };
        let mut log = EventLog::new(Some(2), None, Standing::default());
        let empty = log.bytes();
        log.push(entry(), 0, Duration::ZERO);
        let one = log.bytes();
        assert!(one > empty);
        let label = "x".repeat(150);
        log.push_with(
            entry(),
            1,
            Duration::ZERO,
            vec![(TokenId::new(1), Look::new(&label, None, 200))],
        );
        assert!(log.bytes() >= one + 150, "the label counts");
        log.push(entry(), 2, Duration::ZERO);
        log.push(entry(), 3, Duration::ZERO);
        assert_eq!(log.bytes(), one + (one - empty), "the labelled one dropped");
        log.clear(Standing::default());
        assert_eq!(log.bytes(), empty);
    }

    #[test]
    fn a_file_name_shares_the_records_stem() {
        let start = std::time::UNIX_EPOCH + Duration::from_millis(1_792_499_696_789);
        let dated = EventLog::new(None, Some(start), Standing::default());
        assert_eq!(dated.start_unix_ms(), Some(1_792_499_696_789));
        assert_eq!(
            dated.file_name("parcel"),
            "parcel-20261020-123456.events.json"
        );
        let undated = EventLog::new(None, None, Standing::default());
        assert_eq!(undated.file_name("parcel"), "parcel-undated.events.json");
        assert_eq!(undated.file_name("a/b"), "a_b-undated.events.json");
        let record = |start_unix_ms| crate::Record {
            grid: empty_grid(),
            bin_width_ms: 100,
            start_unix_ms,
            settings: None,
            bins: Vec::new(),
        };
        for (log, record) in [
            (&dated, record(Some(1_792_499_696_789))),
            (&undated, record(None)),
        ] {
            assert_eq!(
                log.file_name("Traffic light").strip_suffix(".events.json"),
                record
                    .file_name("Traffic light")
                    .strip_suffix(".record.json")
            );
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_log_round_trips_as_json_in_the_cores_form() {
        let run = parcel_run();
        // An entry's JSON is the core's for the same event.
        for event in &run.events {
            let entry = Entry::from_event(event).expect("an entry");
            assert_eq!(
                serde_json::to_value(&entry).unwrap(),
                serde_json::to_value(event).unwrap()
            );
        }
        let start = std::time::UNIX_EPOCH + Duration::from_millis(1_792_499_696_789);
        let red = petritype::style::Rgb(200, 30, 40);
        let mut tokens = Roster::default();
        tokens.set_place(
            PlaceId::new(4),
            [(TokenId::new(4), Some(Look::new("fleet", Some(red), 200)))],
        );
        let mut log =
            EventLog::new(Some(8), Some(start), run.start.standing_state()).with_tokens(tokens);
        for (seq, entry) in entries_of(&run).into_iter().enumerate() {
            let looks = match &entry {
                Entry::Mutated { token, .. } => {
                    vec![(*token, Look::new("changed", Some(red), 200))]
                }
                _ => Vec::new(),
            };
            log.push_with(entry, seq as u64, ms(37 * seq as u64), looks);
        }
        assert!(log.dropped() > 0 && !log.base().counts.is_empty());
        let mut text = Vec::new();
        log.write_json(&mut text).expect("it writes");
        let back = EventLog::read_json(text.as_slice()).expect("and reads back");
        assert_eq!(back, log);
        let text = String::from_utf8(text).expect("JSON is text");
        assert!(!text.contains('\n'), "on one line");
        assert!(
            text.contains(r#"{"event":"count","place":0,"count":0}"#),
            "{text}"
        );
        assert!(text.contains(r#""event":"in_flight""#), "{text}");
        assert!(text.contains(r##""colour":"#c81e28""##), "{text}");
        assert!(!text.contains(r#""looks":[]"#), "no looks left out: {text}");
        // A log written before the looks and the roster reads with none.
        let mut old: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        old.as_object_mut().expect("an object").remove("tokens");
        let read = EventLog::read_json(old.to_string().as_bytes()).expect("reads");
        assert!(read.base_tokens().is_empty());
    }
}
