//! [`Activity`]: a pulse and an ember for every place and transition, fed by
//! a net's events and decaying on two clocks.

use std::time::Duration;

use petritype::{Event, Net, PlaceId, Snapshot, TokenId, Topology, TransitionId};
use petritype_layout::LayoutNode;

use crate::delta::{Delta, PlaceDelta, TransitionDelta};
use crate::log::{Entry, Standing};
use crate::settings::{ActivitySettings, Clock, halving};

/// One place's activity at one moment, as [`Activity::place`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct PlaceActivity {
    /// The fast level, 0 to 1.
    pub pulse: f32,
    /// The slow level, 0 to 1.
    pub ember: f32,
    /// The recent balance of tokens added against taken, from -1 (draining)
    /// to 1 (filling), moving on the ember's clock and half-life.
    pub flow: f32,
    /// The standing token count, as the model last heard it.
    pub count: usize,
}

/// One transition's activity at one moment, as [`Activity::transition`]
/// reads it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct TransitionActivity {
    /// The fast level, 0 to 1. Held at 1 while the transition is in flight.
    pub pulse: f32,
    /// The slow level, 0 to 1.
    pub ember: f32,
    /// 1 when the transition has just failed, decaying with the pulse's
    /// clock and half-life. A renderer draws the failed colour while it
    /// lasts.
    pub failure: f32,
    /// Whether the transition is between its launch and its distribute or
    /// failure.
    pub in_flight: bool,
}

/// A net's activity: for every place and transition a fast level, the
/// *pulse*, and a slow one, the *ember*, each in 0 to 1, that a *hit* raises
/// and time lowers; for every place its *flow* and standing count, and for
/// every transition its failure level and whether it is in flight.
///
/// # Feeding it
///
/// Two ways, which can be mixed:
///
/// - [`apply`](Activity::apply) takes the core's [`Event`]s, as a driver or
///   a viewer drains them, and [`apply_entry`](Activity::apply_entry) the
///   same facts as an [`EventLog`](crate::EventLog) keeps them, by the same
///   rules.
/// - The hit interface takes the same facts one by one, for anything that is
///   not a core event, such as a mirror reading wire notifications:
///   [`fired`](Activity::fired), [`launched`](Activity::launched),
///   [`distributed`](Activity::distributed), [`failed`](Activity::failed),
///   [`taken`](Activity::taken), [`added`](Activity::added) and
///   [`mutated`](Activity::mutated) with a count, and the standing state of a node, which is no hit:
///   [`set_count`](Activity::set_count) and
///   [`set_in_flight`](Activity::set_in_flight).
///
/// [`tick`](Activity::tick) advances the wall clock. Every fire and launch
/// advances the firings clock by one for every node, before its own hit, so
/// a firing never decays the hit it gives.
///
/// # Hits
///
/// A hit adds the layer's jump to each level and clamps at 1.
///
/// - A transition takes a hit when it fires or launches, and a smaller one
///   when it distributes, [`distribute_hit`](ActivitySettings::distribute_hit)
///   times each jump. In flight, its pulse is held at 1 and does not decay
///   until it distributes or fails, so a stuck transition reads as steady
///   full brightness. A failure sets its failure level to 1.
/// - A place takes a pulse hit per token taken from it and per token added
///   to it, and one ember hit per event however many tokens move, so a
///   batch flashes the pulse harder and the ember counts events. Hits come
///   from firings and outside changes alike, at the moment of the event: a
///   token leaves at the launch or fire and lands at the distribute or fire.
///   A token changed in place, by a mutating edge or a replace, gives its
///   place one hit and leaves the flow alone, since nothing entered or left.
///   Each token added raises its flow by
///   [`flow_gain`](ActivitySettings::flow_gain) and each taken lowers it,
///   clamped to -1 to 1.
/// - A staged firing moves the flow once, at its distribute or failure,
///   with what its launch took and what it puts back in one balance, so a
///   place a long body empties and refills reads even rather than draining
///   while the body runs. Its hits and counts move at each end as they
///   happen. The hit interface's [`taken`](Activity::taken) and
///   [`added`](Activity::added) move the flow at once.
///
/// # Its account, for a record
///
/// The model keeps a running account of what it applied, a [`Delta`]: the
/// firings, whether the staleness dim was lifted, and per node its hits,
/// its balance, its count and its time in flight.
/// [`take_delta`](Activity::take_delta) hands it over and starts a fresh
/// one, which is what a [`Recorder`](crate::Recorder) bins, and
/// [`apply_delta`](Activity::apply_delta) applies one to a model under that
/// model's settings, which is how a [`Record`](crate::Record) replays. The
/// account holds one entry per node touched since it was last taken, so a
/// model nobody records stays as small as its net.
///
/// A delta keeps how much happened and loses the order it happened in. A
/// model that applies a bin's delta and then ticks by the bin's width
/// stands, at the bin's end, within one bin's decay of the live model on
/// the wall clock, since the live model decayed the bin's earlier hits for
/// part of it and the replay decays them all for the whole of it. On the
/// firings clock it stands where the live model stood when the bin held at
/// most one firing and each place one event, after the firing: the replay
/// decays by the bin's firings before every hit in it. A transition that
/// launches and distributes within one bin replays as one that fired, its
/// pulse raised by its hits rather than held at 1, which is the same at a
/// pulse jump of 1.
///
/// # What it knows of the net
///
/// The ids and names of its places and transitions, from the [`Topology`]
/// it was built from, and nothing else: it holds no reference to the net.
/// An event naming a node it does not know is ignored, so a net whose
/// topology changed wants a new `Activity`.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    settings: ActivitySettings,
    place_names: Vec<String>,
    transition_names: Vec<String>,
    places: Vec<PlaceActivity>,
    transitions: Vec<TransitionActivity>,
    /// How much of the staleness dim's fade is left: 1 when it is lifted,
    /// halving toward 0 while it is in force. The floor is applied on
    /// reading, so a change to it takes effect at once.
    fade: f32,
    /// Fires and launches seen.
    firings: u64,
    /// Per transition, what its launch took, held while it is in flight so
    /// that the flow counts the launch and its distribute as one exchange.
    /// Empty out of flight, and for a launch the model did not see.
    launch_taken: Vec<Vec<(PlaceId, TokenId)>>,
    /// What the model applied since the account was last taken. Each
    /// entry's count and in-flight state are read off the model when it is
    /// taken.
    account: Delta,
}

impl Activity {
    /// An activity for the net `topology` describes, every level at 0, every
    /// count at 0 and nothing in flight.
    ///
    /// A net's seeded tokens are setup and never reported as events, so a
    /// net in process is better built with [`from_net`](Activity::from_net),
    /// and a mirror sets each place's count with
    /// [`set_count`](Activity::set_count).
    pub fn new(topology: &Topology, settings: ActivitySettings) -> Activity {
        Activity::named(
            topology.places.iter().map(|p| (p.id, p.name.clone())),
            topology.transitions.iter().map(|t| (t.id, t.name.clone())),
            settings,
        )
    }

    /// An activity for a net known by its places' and transitions' ids and
    /// names, every level at 0, every count at 0 and nothing in flight: what
    /// [`new`](Activity::new) reads off a [`Topology`], for a feed that has
    /// none, such as a mirror of a net in another process.
    ///
    /// An id is an index, so the model makes room up to the largest id
    /// given; a gap below it is a node with no name, which nothing names.
    pub fn named(
        places: impl IntoIterator<Item = (PlaceId, String)>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
        settings: ActivitySettings,
    ) -> Activity {
        let place_names = by_index(places.into_iter().map(|(id, name)| (id.index(), name)));
        let transition_names =
            by_index(transitions.into_iter().map(|(id, name)| (id.index(), name)));
        Activity {
            settings,
            places: vec![PlaceActivity::default(); place_names.len()],
            transitions: vec![TransitionActivity::default(); transition_names.len()],
            launch_taken: vec![Vec::new(); transition_names.len()],
            place_names,
            transition_names,
            fade: 1.0,
            firings: 0,
            account: Delta::default(),
        }
    }

    /// An activity for `net` as it stands: its topology, each place's count
    /// and the transitions in flight, every level at 0. The net is read
    /// once and not held.
    pub fn from_net(net: &Net, settings: ActivitySettings) -> Activity {
        let mut activity = Activity::new(&net.topology(), settings);
        for (place, info) in net.places() {
            activity.set_count(place, info.count);
        }
        for transition in net.in_flight() {
            activity.set_in_flight(transition, true);
        }
        activity
    }

    /// The same as [`from_net`](Activity::from_net), from a
    /// [`Snapshot`].
    pub fn from_snapshot(snapshot: &Snapshot, settings: ActivitySettings) -> Activity {
        let mut activity = Activity::new(&snapshot.topology(), settings);
        for (place, info) in snapshot.places() {
            activity.set_count(place, info.count);
        }
        for transition in snapshot.in_flight() {
            activity.set_in_flight(transition, true);
        }
        activity
    }

    // ----- the feed -------------------------------------------------------

    /// Takes one of the core's events: the hits it implies, by the rules on
    /// [`Activity`].
    ///
    /// - [`Fired`](Event::Fired): the transition [fires](Activity::fired),
    ///   then each consumed token is taken from its place and each produced
    ///   one added to its place.
    /// - [`Launched`](Event::Launched): the transition
    ///   [launches](Activity::launched), and its consumed tokens are taken:
    ///   their places' hits and counts move now, and the flow waits for the
    ///   firing's end.
    /// - [`Distributed`](Event::Distributed): the transition
    ///   [distributes](Activity::distributed), and its produced tokens are
    ///   added. The flow takes what the launch took and what landed as one
    ///   exchange, so a place the firing empties and refills stays even.
    /// - [`Failed`](Event::Failed): the transition [fails](Activity::failed).
    ///   After a launch the tokens already left: restored ones come back
    ///   with hits and counts and the flow stays where it was, and lost ones
    ///   count in the flow as taken, now. A firing that failed with no launch
    ///   before it (a sequential `step` or `fire`) took its tokens now: they
    ///   are taken, and added back if restored.
    /// - [`Added`](Event::Added) and [`Removed`](Event::Removed): a token
    ///   added to or taken from the place.
    /// - [`Mutated`](Event::Mutated): a token [changed](Activity::mutated)
    ///   in the place.
    ///
    /// A distribute whose launch the model did not see, such as one in
    /// flight when the model was built, moves the flow by only what it
    /// lands, and a failure of such a firing leaves the flow alone.
    ///
    /// Everything else changes no level. [`Nested`](Event::Nested) is an
    /// inner net's event, which a thumbnail of that net takes, not this one;
    /// and after [`Resynced`](Event::Resynced) the counts are unknown until a caller
    /// sets them again with [`set_count`](Activity::set_count).
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Fired {
                transition,
                consumed,
                produced,
            } => self.on_fired(*transition, consumed, produced),
            Event::Launched {
                transition,
                consumed,
            } => self.on_launched(*transition, consumed),
            Event::Distributed {
                transition,
                produced,
            } => self.on_distributed(*transition, produced),
            Event::Failed {
                transition,
                consumed,
                restored,
                ..
            } => self.on_failed(*transition, consumed, *restored),
            Event::Added { place, .. } => self.added(*place, 1),
            Event::Removed { place, .. } => self.taken(*place, 1),
            Event::Mutated { place, .. } => self.mutated(*place, 1),
            // `Event` is `#[non_exhaustive]`; what is left moves no token.
            _ => {}
        }
    }

    /// Takes one [`Entry`] of an [`EventLog`](crate::EventLog): an event's
    /// entry by the same rules, and the same code, as
    /// [`apply`](Activity::apply) takes the event, so a model fed the entries
    /// of a log stands where the model fed the events stood. The two entries
    /// a host adds when it corrects the model are no hit:
    /// [`Count`](Entry::Count) is [`set_count`](Activity::set_count) and
    /// [`InFlight`](Entry::InFlight) is
    /// [`set_in_flight`](Activity::set_in_flight).
    pub fn apply_entry(&mut self, entry: &Entry) {
        match entry {
            Entry::Fired {
                transition,
                consumed,
                produced,
            } => self.on_fired(*transition, consumed, produced),
            Entry::Launched {
                transition,
                consumed,
            } => self.on_launched(*transition, consumed),
            Entry::Distributed {
                transition,
                produced,
            } => self.on_distributed(*transition, produced),
            Entry::Failed {
                transition,
                consumed,
                restored,
                ..
            } => self.on_failed(*transition, consumed, *restored),
            Entry::Added { place, .. } => self.added(*place, 1),
            Entry::Removed { place, .. } => self.taken(*place, 1),
            Entry::Mutated { place, .. } => self.mutated(*place, 1),
            Entry::Count { place, count } => self.set_count(*place, *count),
            Entry::InFlight { transition, on } => self.set_in_flight(*transition, *on),
        }
    }

    /// A [`Fired`](Event::Fired): the transition fires, then each consumed
    /// token is taken from its place and each produced one added to its
    /// place, in one exchange.
    fn on_fired(
        &mut self,
        transition: TransitionId,
        consumed: &[(PlaceId, TokenId)],
        produced: &[(PlaceId, TokenId)],
    ) {
        self.fired(transition);
        self.exchange(consumed, produced);
    }

    /// A [`Launched`](Event::Launched): the transition launches and its
    /// consumed tokens are taken, their hits and counts now, the flow held
    /// for the firing's end.
    fn on_launched(&mut self, transition: TransitionId, consumed: &[(PlaceId, TokenId)]) {
        self.launched(transition);
        self.move_tokens(consumed, &[]);
        if let Some(held) = self.launch_taken.get_mut(transition.index() as usize) {
            held.clear();
            held.extend_from_slice(consumed);
        }
    }

    /// A [`Distributed`](Event::Distributed): the transition distributes and
    /// its produced tokens land, the flow taking what the launch took and
    /// what landed as one exchange.
    fn on_distributed(&mut self, transition: TransitionId, produced: &[(PlaceId, TokenId)]) {
        let held = self.take_launch(transition);
        self.distributed(transition);
        self.move_tokens(&[], produced);
        self.shift_flow(&held, produced);
    }

    /// A [`Failed`](Event::Failed): the transition fails, and its consumed
    /// tokens move by whether a launch took them and whether they were
    /// restored.
    fn on_failed(
        &mut self,
        transition: TransitionId,
        consumed: &[(PlaceId, TokenId)],
        restored: bool,
    ) {
        let launched = self.transition(transition).is_some_and(|t| t.in_flight);
        let held = self.take_launch(transition);
        self.failed(transition);
        match (launched, restored) {
            // Out at the launch and back now: the flow stays.
            (true, true) => self.move_tokens(&[], consumed),
            // Out at the launch and gone: the flow counts them now, by the
            // event's own list, or the launch's when the event has none. A
            // launch the model never saw, a transition in flight at a
            // resync, held nothing.
            (true, false) => {
                let lost = if consumed.is_empty() {
                    &held[..]
                } else {
                    consumed
                };
                self.shift_flow(lost, &[]);
            }
            // Out now, in one go, and back if restored.
            (false, true) => self.exchange(consumed, consumed),
            (false, false) => self.exchange(consumed, &[]),
        }
    }

    /// A transition fired in one go: the firings clock advances by one, the
    /// staleness dim is lifted, and the transition takes a hit.
    pub fn fired(&mut self, transition: TransitionId) {
        self.firing();
        self.hit(transition, 1.0);
        if let Some(e) = self.note_transition(transition) {
            e.fires = e.fires.saturating_add(1);
        }
    }

    /// A transition launched: as [`fired`](Activity::fired), and it is in
    /// flight, its pulse held at 1 until it distributes or fails.
    pub fn launched(&mut self, transition: TransitionId) {
        self.firing();
        self.set_in_flight(transition, true);
        self.hit(transition, 1.0);
        if let Some(e) = self.note_transition(transition) {
            e.fires = e.fires.saturating_add(1);
        }
    }

    /// A transition distributed: no longer in flight, so its pulse decays
    /// again from where it was held, and a hit of
    /// [`distribute_hit`](ActivitySettings::distribute_hit) times each jump.
    /// The staleness dim is lifted; the firings clock does not move, since
    /// the launch counted this firing.
    pub fn distributed(&mut self, transition: TransitionId) {
        self.lift();
        self.set_in_flight(transition, false);
        self.hit(transition, self.settings.distribute_hit);
        if let Some(e) = self.note_transition(transition) {
            e.distributes = e.distributes.saturating_add(1);
        }
    }

    /// A transition's firing failed: no longer in flight, and its failure
    /// level set to 1. A transition that was not in flight failed in one go,
    /// so this counts as its firing as well, with the hit and the clock of
    /// [`fired`](Activity::fired).
    pub fn failed(&mut self, transition: TransitionId) {
        if self.transition(transition).is_some_and(|t| t.in_flight) {
            self.lift();
        } else {
            self.fired(transition);
        }
        self.set_in_flight(transition, false);
        if let Some(t) = self.transition_mut(transition) {
            t.failure = 1.0;
        }
        if let Some(e) = self.note_transition(transition) {
            e.failures = e.failures.saturating_add(1);
        }
    }

    /// `n` tokens were taken from a place: `n` pulse hits and one ember hit,
    /// its flow lowered by `n` times the gain, and its count by `n`, never
    /// below 0.
    pub fn taken(&mut self, place: PlaceId, n: usize) {
        self.touch(place, n, 0);
    }

    /// `n` tokens were added to a place: `n` pulse hits and one ember hit,
    /// its flow raised by `n` times the gain, and its count by `n`.
    pub fn added(&mut self, place: PlaceId, n: usize) {
        self.touch(place, 0, n);
    }

    /// `n` tokens were changed in place, by a mutating edge or a replace:
    /// `n` pulse hits and one ember hit, and the flow and the count as they
    /// were, since nothing entered or left.
    pub fn mutated(&mut self, place: PlaceId, n: usize) {
        self.hit_place(place, n);
    }

    /// The standing count of a place, as read from the net or the wire. No
    /// hit: the base glow follows the count at once.
    pub fn set_count(&mut self, place: PlaceId, total: usize) {
        if let Some(p) = self.places.get_mut(place.index() as usize)
            && p.count != total
        {
            p.count = total;
            self.note_place(place);
        }
    }

    /// Whether a transition stands in flight, as read from the net or the
    /// wire. No hit; in flight, its pulse is held at 1. Out of flight, what
    /// its launch took is no longer held for the flow.
    pub fn set_in_flight(&mut self, transition: TransitionId, in_flight: bool) {
        if let Some(t) = self.transition_mut(transition) {
            let changed = t.in_flight != in_flight;
            t.in_flight = in_flight;
            if in_flight {
                t.pulse = 1.0;
            }
            if changed {
                self.note_transition(transition);
            }
        }
        if !in_flight {
            self.take_launch(transition);
        }
    }

    /// Advances the wall clock by `dt`: every level on that clock decays,
    /// and the staleness dim fades the whole while it is in force. Each
    /// transition in flight counts `dt` in the account as time in flight.
    pub fn tick(&mut self, dt: Duration) {
        let seconds = dt.as_secs_f32();
        let ms = dt.as_secs_f64() * 1000.0;
        if ms > 0.0 {
            for (index, t) in self.transitions.iter().enumerate() {
                if t.in_flight
                    && let Ok(index) = u32::try_from(index)
                {
                    let e = self
                        .account
                        .transitions
                        .entry(TransitionId::new(index))
                        .or_default();
                    e.in_flight_ms += ms;
                }
            }
        }
        self.decay(Clock::Wall, seconds);
        match self.settings.stale_half_life_secs {
            Some(half_life) if self.settings.stale_dim() => {
                self.fade *= halving(half_life, seconds);
            }
            _ => self.fade = 1.0,
        }
    }

    // ----- the account -------------------------------------------------------

    /// What the model applied since the account was last taken, handed
    /// over, and a fresh account started: the [`Delta`] a recorder bins.
    /// Each place in it carries its count as it stands now and each
    /// transition its in-flight state.
    pub fn take_delta(&mut self) -> Delta {
        let mut delta = std::mem::take(&mut self.account);
        for (place, e) in &mut delta.places {
            if let Some(p) = self.place(*place) {
                e.count = p.count;
            }
        }
        for (transition, e) in &mut delta.transitions {
            if let Some(t) = self.transition(*transition) {
                e.in_flight = t.in_flight;
            }
        }
        delta
    }

    /// The model's standing state as a [`Delta`] with no hit: each place
    /// holding tokens with its count, and each transition in flight, as
    /// [`standing_state`](Activity::standing_state) reads them. A recorder
    /// that starts on a model already running takes it first, so that a
    /// replay of its record starts from the counts that stood. The account
    /// is left as it is.
    pub fn standing(&self) -> Delta {
        self.standing_state().to_delta()
    }

    /// The model's standing state: each place holding tokens with its
    /// count, and each transition in flight, in the order of their ids. An
    /// [`EventLog`](crate::EventLog) started on a model already running
    /// takes it as its base, so that a replay of the log starts from the
    /// counts that stood.
    pub fn standing_state(&self) -> Standing {
        let ids = |index: usize| u32::try_from(index).ok();
        Standing {
            counts: self
                .places
                .iter()
                .enumerate()
                .filter(|(_, p)| p.count > 0)
                .filter_map(|(index, p)| Some((PlaceId::new(ids(index)?), p.count)))
                .collect(),
            in_flight: self
                .transitions
                .iter()
                .enumerate()
                .filter(|(_, t)| t.in_flight)
                .filter_map(|(index, _)| Some(TransitionId::new(ids(index)?)))
                .collect(),
        }
    }

    /// Applies a [`Delta`] under this model's settings, as a replay of a
    /// record does: the firings clock moves by the delta's firings first,
    /// as the live model decayed everyone before each firing's own hit;
    /// then each transition takes its hits, a full jump per fire and
    /// [`distribute_hit`](ActivitySettings::distribute_hit) of one per
    /// distribute, and its failure level is set to 1 if it failed; each
    /// place takes its hits, its balance by the
    /// [flow gain](ActivitySettings::flow_gain) and its count; each
    /// transition's in-flight state is set, and the staleness dim is lifted
    /// if the delta lifted it.
    ///
    /// The wall clock is not in a delta: a replay
    /// [ticks](Activity::tick) by the bin's width after each. The order
    /// inside the delta is lost, with the consequences on [`Activity`]. The
    /// account takes what was applied, as it does for any other feed.
    pub fn apply_delta(&mut self, delta: &Delta) {
        if delta.firings > 0 {
            self.firings += u64::from(delta.firings);
            self.decay(Clock::Firings, delta.firings as f32);
            self.account.firings = self.account.firings.saturating_add(delta.firings);
        }
        let distribute = self.settings.distribute_hit;
        for (&transition, e) in &delta.transitions {
            let fraction = e.fires as f32 + e.distributes as f32 * distribute;
            if fraction > 0.0 {
                self.hit(transition, fraction);
            }
            if e.failures > 0
                && let Some(t) = self.transition_mut(transition)
            {
                t.failure = 1.0;
            }
            if let Some(a) = self.note_transition(transition) {
                a.fires = a.fires.saturating_add(e.fires);
                a.distributes = a.distributes.saturating_add(e.distributes);
                a.failures = a.failures.saturating_add(e.failures);
            }
        }
        for (&place, e) in &delta.places {
            self.raise_place(place, e.pulse_hits, e.ember_hits);
            self.shift(place, e.balance);
            self.set_count(place, e.count);
        }
        for (&transition, e) in &delta.transitions {
            self.set_in_flight(transition, e.in_flight);
        }
        if delta.lifted {
            self.lift();
        }
    }

    // ----- reading ----------------------------------------------------------

    /// The settings the model runs by.
    pub fn settings(&self) -> &ActivitySettings {
        &self.settings
    }

    /// Replaces the settings. The levels stay where they are and move by the
    /// new values from now on.
    pub fn set_settings(&mut self, settings: ActivitySettings) {
        self.settings = settings;
    }

    /// A place's levels, flow and count, or `None` for a place the model
    /// does not know.
    pub fn place(&self, place: PlaceId) -> Option<PlaceActivity> {
        self.places.get(place.index() as usize).copied()
    }

    /// A transition's levels, failure and in-flight state, or `None` for a
    /// transition the model does not know.
    pub fn transition(&self, transition: TransitionId) -> Option<TransitionActivity> {
        self.transitions.get(transition.index() as usize).copied()
    }

    /// A place's base glow, from its count by the
    /// [count curve](ActivitySettings::count_curve); 0 for a place the model
    /// does not know.
    pub fn base(&self, place: PlaceId) -> f32 {
        self.place(place)
            .map_or(0.0, |p| self.settings.base(p.count))
    }

    /// The glow to draw a node with, in 0 to 1: its two levels by the
    /// [glow mode](ActivitySettings::glow), for a place no less than its
    /// [base](Activity::base), and the whole times the
    /// [freshness](Activity::freshness). 0 for a node the model does not
    /// know.
    pub fn glow(&self, node: LayoutNode) -> f32 {
        let glow = match node {
            LayoutNode::Place(id) => self.place(id).map_or(0.0, |p| {
                self.settings
                    .glow
                    .combine(p.pulse, p.ember)
                    .max(self.settings.base(p.count))
            }),
            LayoutNode::Transition(id) => self
                .transition(id)
                .map_or(0.0, |t| self.settings.glow.combine(t.pulse, t.ember)),
        };
        glow * self.freshness()
    }

    /// How far the staleness dim has left the picture, 1 when it is lifted:
    /// the factor every glow is drawn at. While the dim is in force (some
    /// layer on the firings clock) it fades on its own half-life of wall time
    /// toward the [floor](ActivitySettings::stale_floor), never below it,
    /// and comes back to 1 at the next fire, launch, distribute or failure.
    /// A token added from outside does not restore it.
    pub fn freshness(&self) -> f32 {
        let floor = self.settings.stale_floor.clamp(0.0, 1.0);
        floor + (1.0 - floor) * self.fade
    }

    /// How many fires and launches the model has seen: the firings clock.
    pub fn firings(&self) -> u64 {
        self.firings
    }

    /// How many places the model knows.
    pub fn place_count(&self) -> usize {
        self.places.len()
    }

    /// How many transitions the model knows.
    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    /// A place's name, from the topology the model was built from.
    pub fn place_name(&self, place: PlaceId) -> Option<&str> {
        self.place_names
            .get(place.index() as usize)
            .map(String::as_str)
    }

    /// A transition's name, from the topology the model was built from.
    pub fn transition_name(&self, transition: TransitionId) -> Option<&str> {
        self.transition_names
            .get(transition.index() as usize)
            .map(String::as_str)
    }

    /// The place with this name, for a feed that knows nodes by name.
    pub fn place_named(&self, name: &str) -> Option<PlaceId> {
        let index = self.place_names.iter().position(|n| n == name)?;
        Some(PlaceId::new(u32::try_from(index).ok()?))
    }

    /// The transition with this name, for a feed that knows nodes by name.
    pub fn transition_named(&self, name: &str) -> Option<TransitionId> {
        let index = self.transition_names.iter().position(|n| n == name)?;
        Some(TransitionId::new(u32::try_from(index).ok()?))
    }

    // ----- inside -----------------------------------------------------------

    fn transition_mut(&mut self, transition: TransitionId) -> Option<&mut TransitionActivity> {
        self.transitions.get_mut(transition.index() as usize)
    }

    /// A known place's entry in the account, made if it has none.
    fn note_place(&mut self, place: PlaceId) -> Option<&mut PlaceDelta> {
        if (place.index() as usize) < self.places.len() {
            Some(self.account.places.entry(place).or_default())
        } else {
            None
        }
    }

    /// A known transition's entry in the account, made if it has none.
    fn note_transition(&mut self, transition: TransitionId) -> Option<&mut TransitionDelta> {
        if (transition.index() as usize) < self.transitions.len() {
            Some(self.account.transitions.entry(transition).or_default())
        } else {
            None
        }
    }

    /// The staleness dim lifted by an end of a firing, and the account told.
    fn lift(&mut self) {
        self.fade = 1.0;
        self.account.lifted = true;
    }

    /// A hit of `fraction` times each layer's jump on a transition. A pulse
    /// held in flight stays at 1.
    fn hit(&mut self, transition: TransitionId, fraction: f32) {
        let (pulse, ember) = (self.settings.pulse.jump, self.settings.ember.jump);
        if let Some(t) = self.transition_mut(transition) {
            t.pulse = if t.in_flight {
                1.0
            } else {
                raise(t.pulse, fraction * pulse)
            };
            t.ember = raise(t.ember, fraction * ember);
        }
    }

    /// One fire or launch anywhere in the net: the firings clock advances,
    /// and the staleness dim is lifted.
    fn firing(&mut self) {
        self.firings += 1;
        self.account.firings = self.account.firings.saturating_add(1);
        self.lift();
        self.decay(Clock::Firings, 1.0);
    }

    /// Every level on `clock` decays by `amount` of it: the pulse and the
    /// failure by the pulse's half-life, the ember and the flow by the
    /// ember's. A pulse held in flight does not decay.
    fn decay(&mut self, clock: Clock, amount: f32) {
        let pulse = self.settings.pulse.decay(clock, amount);
        let ember = self.settings.ember.decay(clock, amount);
        if pulse == 1.0 && ember == 1.0 {
            return;
        }
        for p in &mut self.places {
            p.pulse *= pulse;
            p.ember *= ember;
            p.flow *= ember;
        }
        for t in &mut self.transitions {
            if !t.in_flight {
                t.pulse *= pulse;
            }
            t.failure *= pulse;
            t.ember *= ember;
        }
    }

    /// What a transition's launch took, no longer held: empty for one not
    /// in flight, or one whose launch the model did not see.
    fn take_launch(&mut self, transition: TransitionId) -> Vec<(PlaceId, TokenId)> {
        self.launch_taken
            .get_mut(transition.index() as usize)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The tokens of one event leave and land: each place touched once, with
    /// all it lost and gained, so that a place a firing both takes from and
    /// writes to keeps an even balance whatever its flow was.
    fn exchange(&mut self, taken: &[(PlaceId, TokenId)], added: &[(PlaceId, TokenId)]) {
        for (place, out, into) in tally(taken, added) {
            self.touch(place, out, into);
        }
    }

    /// Tokens leave and land with the flow left alone: each place touched
    /// once, its hits and its count moving by all it lost and gained.
    fn move_tokens(&mut self, taken: &[(PlaceId, TokenId)], added: &[(PlaceId, TokenId)]) {
        for (place, out, into) in tally(taken, added) {
            self.hit_place(place, out + into);
            self.recount(place, out, into);
        }
    }

    /// The flow alone: each place touched takes the balance of all it lost
    /// and gained in one tally, so a place that lost a token and got one
    /// back stays where it was.
    fn shift_flow(&mut self, taken: &[(PlaceId, TokenId)], added: &[(PlaceId, TokenId)]) {
        for (place, out, into) in tally(taken, added) {
            self.balance(place, out, into);
        }
    }

    /// `hits` hits on a place's pulse, one per token, and one on its ember
    /// if there are any, since the ember counts events.
    fn hit_place(&mut self, place: PlaceId, hits: usize) {
        let hits = u32::try_from(hits).unwrap_or(u32::MAX);
        self.raise_place(place, hits, hits.min(1));
    }

    /// `pulse_hits` jumps on a place's pulse and `ember_hits` on its ember,
    /// each clamped at 1, and the account told.
    fn raise_place(&mut self, place: PlaceId, pulse_hits: u32, ember_hits: u32) {
        if pulse_hits == 0 && ember_hits == 0 {
            return;
        }
        let (pulse, ember) = (self.settings.pulse.jump, self.settings.ember.jump);
        if let Some(p) = self.places.get_mut(place.index() as usize) {
            p.pulse = raise(p.pulse, pulse_hits as f32 * pulse);
            p.ember = raise(p.ember, ember_hits as f32 * ember);
        }
        if let Some(e) = self.note_place(place) {
            e.pulse_hits = e.pulse_hits.saturating_add(pulse_hits);
            e.ember_hits = e.ember_hits.saturating_add(ember_hits);
        }
    }

    /// A place lost `taken` tokens and gained `added`: a pulse hit for each
    /// and one ember hit, its count and its flow.
    fn touch(&mut self, place: PlaceId, taken: usize, added: usize) {
        self.hit_place(place, taken + added);
        self.recount(place, taken, added);
        self.balance(place, taken, added);
    }

    /// A place's count after it lost `taken` tokens and gained `added`,
    /// never below 0.
    fn recount(&mut self, place: PlaceId, taken: usize, added: usize) {
        if let Some(p) = self.places.get_mut(place.index() as usize) {
            let count = p.count.saturating_sub(taken).saturating_add(added);
            if count != p.count {
                p.count = count;
                self.note_place(place);
            }
        }
    }

    /// A place's flow after it lost `taken` tokens and gained `added`: the
    /// gain times the balance, clamped to -1 to 1.
    fn balance(&mut self, place: PlaceId, taken: usize, added: usize) {
        let signed = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        self.shift(place, signed(added) - signed(taken));
    }

    /// A place's flow moved by `balance` tokens, added less taken, times the
    /// gain and clamped to -1 to 1, and the account told.
    fn shift(&mut self, place: PlaceId, balance: i64) {
        if balance == 0 {
            return;
        }
        let gain = self.settings.flow_gain;
        if let Some(p) = self.places.get_mut(place.index() as usize) {
            p.flow = (p.flow + balance as f32 * gain).clamp(-1.0, 1.0);
        }
        if let Some(e) = self.note_place(place) {
            e.balance = e.balance.saturating_add(balance);
        }
    }
}

/// The tokens of one event by place: for each place touched, in the order
/// first touched, how many left it and how many landed in it.
fn tally(
    taken: &[(PlaceId, TokenId)],
    added: &[(PlaceId, TokenId)],
) -> Vec<(PlaceId, usize, usize)> {
    let mut tally: Vec<(PlaceId, usize, usize)> = Vec::new();
    let moves = taken
        .iter()
        .map(|&(place, _)| (place, 1, 0))
        .chain(added.iter().map(|&(place, _)| (place, 0, 1)));
    for (place, out, into) in moves {
        match tally.iter_mut().find(|(p, ..)| *p == place) {
            Some(entry) => {
                entry.1 += out;
                entry.2 += into;
            }
            None => tally.push((place, out, into)),
        }
    }
    tally
}

/// Names by index: each one at the index it comes with, and an empty name
/// in any gap below the largest.
fn by_index(named: impl Iterator<Item = (u32, String)>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (index, name) in named {
        let index = index as usize;
        if names.len() <= index {
            names.resize(index + 1, String::new());
        }
        names[index] = name;
    }
    names
}

/// A level raised by `by` and clamped to 0 to 1.
fn raise(level: f32, by: f32) -> f32 {
    (level + by).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use petritype::{MutateEdge, NetBuilder, TakeEdge};

    use super::*;
    use crate::settings::{GlowMode, LayerSettings};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// `In -> Go -> Out`, and `Side -> Other -> Out`.
    fn topology() -> Topology {
        let mut b = NetBuilder::new();
        let input = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        let side = b.place::<u32>("Side");
        b.transition("Go", (TakeEdge(input),), (out,), |n: u32| (n,));
        b.transition("Other", (TakeEdge(side),), (out,), |n: u32| (n,));
        b.build().expect("the net builds").topology()
    }

    const IN: PlaceId = PlaceId::new(0);
    const OUT: PlaceId = PlaceId::new(1);
    const SIDE: PlaceId = PlaceId::new(2);
    const GO: TransitionId = TransitionId::new(0);
    const OTHER: TransitionId = TransitionId::new(1);

    fn activity(settings: ActivitySettings) -> Activity {
        Activity::new(&topology(), settings)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_hit_raises_a_level_by_the_jump_and_clamps_at_one() {
        let mut a = activity(ActivitySettings::default());
        a.added(IN, 1);
        let p = a.place(IN).unwrap();
        assert_eq!((p.pulse, p.ember), (1.0, 0.05));
        a.added(IN, 1);
        let p = a.place(IN).unwrap();
        assert_eq!(p.pulse, 1.0, "the pulse clamps");
        assert!(close(p.ember, 0.1), "the ember climbs: {p:?}");
        let mut settings = ActivitySettings::default();
        settings.ember.jump = 0.4;
        let mut a = activity(settings);
        for _ in 0..3 {
            a.added(IN, 1);
        }
        assert_eq!(a.place(IN).unwrap().ember, 1.0, "and clamps too");
        // A smaller jump shows the add before the clamp.
        let mut settings = ActivitySettings::default();
        settings.pulse.jump = 0.4;
        let mut a = activity(settings);
        a.fired(GO);
        assert!(close(a.transition(GO).unwrap().pulse, 0.4));
        a.fired(GO);
        // The second firing first decays nothing (wall clock), then adds.
        assert!(close(a.transition(GO).unwrap().pulse, 0.8));
        a.fired(GO);
        assert_eq!(a.transition(GO).unwrap().pulse, 1.0);
    }

    #[test]
    fn a_place_takes_one_ember_hit_per_event_and_a_pulse_hit_per_token() {
        let mut settings = ActivitySettings::default();
        settings.pulse.jump = 0.1;
        let landed = |n: usize| {
            let mut a = activity(settings.clone());
            a.apply(&Event::Distributed {
                transition: GO,
                produced: vec![(OUT, TokenId::new(0)); n],
            });
            a.place(OUT).unwrap()
        };
        let (ten, one) = (landed(10), landed(1));
        assert_eq!(ten.ember, one.ember, "the ember counts the event");
        assert_eq!(one.ember, 0.05);
        assert!(
            close(ten.pulse, 1.0) && close(one.pulse, 0.1),
            "{ten:?} {one:?}"
        );
        assert_eq!((ten.count, one.count), (10, 1));
    }

    #[test]
    fn one_half_life_halves_a_level() {
        let mut a = activity(ActivitySettings::default());
        a.fired(GO);
        a.tick(ms(300));
        let t = a.transition(GO).unwrap();
        assert!(close(t.pulse, 0.5), "{t:?}");
        assert!(close(t.ember, 0.05 * 0.5f32.powf(0.3 / 20.0)), "{t:?}");
        a.tick(ms(300));
        assert!(close(a.transition(GO).unwrap().pulse, 0.25));
        // Ticking in small steps comes to the same.
        let mut b = activity(ActivitySettings::default());
        b.fired(GO);
        for _ in 0..30 {
            b.tick(ms(20));
        }
        assert!(close(b.transition(GO).unwrap().pulse, 0.25));
    }

    #[test]
    fn in_flight_holds_the_pulse_and_distribute_releases_it() {
        let mut a = activity(ActivitySettings::default());
        a.launched(GO);
        a.tick(Duration::from_secs(10));
        let t = a.transition(GO).unwrap();
        assert!(t.in_flight);
        assert_eq!(t.pulse, 1.0, "held through ten seconds");
        assert!(t.ember < 0.05, "the ember is not held");
        a.distributed(GO);
        let t = a.transition(GO).unwrap();
        assert!(!t.in_flight);
        assert_eq!(t.pulse, 1.0, "released from full");
        a.tick(ms(300));
        assert!(close(a.transition(GO).unwrap().pulse, 0.5), "and decaying");
    }

    #[test]
    fn in_flight_holds_the_pulse_and_a_failure_releases_it() {
        let mut a = activity(ActivitySettings::default());
        a.launched(GO);
        a.tick(Duration::from_secs(1));
        a.failed(GO);
        let t = a.transition(GO).unwrap();
        assert!(!t.in_flight);
        assert_eq!((t.pulse, t.failure), (1.0, 1.0));
        assert_eq!(a.firings(), 1, "the launch counted the firing");
        a.tick(ms(300));
        let t = a.transition(GO).unwrap();
        assert!(close(t.pulse, 0.5) && close(t.failure, 0.5), "{t:?}");
        // A failure with no launch before it is a firing of its own.
        a.failed(OTHER);
        assert_eq!(a.firings(), 2);
        let t = a.transition(OTHER).unwrap();
        assert_eq!((t.pulse, t.failure, t.in_flight), (1.0, 1.0, false));
    }

    #[test]
    fn a_distribute_hit_is_the_fraction_of_the_jump() {
        let mut a = activity(ActivitySettings::default());
        a.launched(GO);
        a.distributed(GO);
        // The launch's ember hit, then half of it again.
        assert!(close(a.transition(GO).unwrap().ember, 0.05 + 0.025));
        // A distribute whose launch the model never saw shows it in the
        // pulse too: a mirror that joined while the body ran.
        a.distributed(OTHER);
        let t = a.transition(OTHER).unwrap();
        assert!(close(t.pulse, 0.5) && close(t.ember, 0.025), "{t:?}");
        assert_eq!(a.firings(), 1, "a distribute is no firing");
    }

    #[test]
    fn flow_rises_with_tokens_added_falls_with_tokens_taken_and_decays_to_zero() {
        let mut a = activity(ActivitySettings::default());
        a.added(OUT, 2);
        assert!(close(a.place(OUT).unwrap().flow, 0.5));
        a.taken(IN, 1);
        assert!(close(a.place(IN).unwrap().flow, -0.25));
        a.added(OUT, 10);
        assert_eq!(a.place(OUT).unwrap().flow, 1.0, "clamped at 1");
        a.taken(IN, 10);
        assert_eq!(a.place(IN).unwrap().flow, -1.0, "and at -1");
        // On the ember's half-life: 20 s halves it, and the sign stays.
        a.tick(Duration::from_secs(20));
        assert!(close(a.place(OUT).unwrap().flow, 0.5));
        assert!(close(a.place(IN).unwrap().flow, -0.5));
        a.tick(Duration::from_secs(400));
        assert!(a.place(OUT).unwrap().flow.abs() < 1e-5);
        // A token in and one out leave the balance where it was.
        a.added(OUT, 1);
        a.taken(OUT, 1);
        assert!(a.place(OUT).unwrap().flow.abs() < 1e-5);
    }

    #[test]
    fn counts_follow_tokens_taken_and_added_and_never_go_below_zero() {
        let mut a = activity(ActivitySettings::default());
        a.set_count(IN, 3);
        assert!(close(a.base(IN), 0.875));
        a.taken(IN, 2);
        assert_eq!(a.place(IN).unwrap().count, 1);
        assert!(close(a.base(IN), 0.5));
        a.taken(IN, 5);
        assert_eq!(a.place(IN).unwrap().count, 0);
        // The glow is no less than the base, whatever the levels.
        a.set_count(OUT, 2);
        assert!(close(a.glow(LayoutNode::Place(OUT)), 0.75));
    }

    #[test]
    fn under_the_firings_clock_a_level_halves_after_its_half_life_in_firings_elsewhere() {
        let mut settings = ActivitySettings::default();
        settings.pulse.clock = Clock::Firings;
        settings.stale_half_life_secs = None;
        let mut a = activity(settings);
        a.fired(GO);
        assert_eq!(a.transition(GO).unwrap().pulse, 1.0, "its own firing first");
        for _ in 0..3 {
            a.fired(OTHER);
        }
        assert!(close(a.transition(GO).unwrap().pulse, 0.5));
        // While nothing fires, it holds, however long.
        a.tick(Duration::from_secs(3600));
        assert!(close(a.transition(GO).unwrap().pulse, 0.5));
        // A distribute or a token added from outside is no firing.
        a.distributed(OTHER);
        a.added(IN, 1);
        assert!(close(a.transition(GO).unwrap().pulse, 0.5));
        // The ember is still on the wall clock: the hour took it.
        assert!(a.transition(GO).unwrap().ember < 1e-5);
        for _ in 0..3 {
            a.launched(OTHER);
            a.distributed(OTHER);
        }
        assert!(
            close(a.transition(GO).unwrap().pulse, 0.25),
            "launches count"
        );
    }

    #[test]
    fn the_staleness_dim_fades_the_whole_and_a_firing_restores_it() {
        let mut settings = ActivitySettings {
            stale_floor: 0.0,
            ..ActivitySettings::default()
        };
        settings.ember.clock = Clock::Firings;
        let mut a = activity(settings);
        a.set_count(OUT, 1);
        a.fired(GO);
        let go = LayoutNode::Transition(GO);
        let out = LayoutNode::Place(OUT);
        a.tick(Duration::from_secs(30));
        assert!(close(a.freshness(), 0.5));
        // The pulse (wall clock) is gone; the ember (firings) holds, drawn at
        // half; the base is dimmed too.
        assert!(close(a.glow(go), 0.05 * 0.5), "{}", a.glow(go));
        assert!(close(a.glow(out), 0.5 * 0.5));
        a.tick(Duration::from_secs(30));
        assert!(close(a.freshness(), 0.25));
        a.fired(OTHER);
        assert_eq!(a.freshness(), 1.0);
        assert!(close(a.glow(out), 0.5), "the pattern is back at full");
        // Each end of a firing restores it too, and an outside add does not.
        for end in [Activity::distributed, Activity::failed] {
            a.tick(Duration::from_secs(30));
            end(&mut a, GO);
            assert_eq!(a.freshness(), 1.0);
        }
        a.tick(Duration::from_secs(30));
        a.added(IN, 1);
        assert!(close(a.freshness(), 0.5));
        // Off, or with both layers on the wall clock, there is no dim.
        let mut off = activity(ActivitySettings::default());
        off.tick(Duration::from_secs(30));
        assert_eq!(off.freshness(), 1.0);
    }

    #[test]
    fn the_staleness_dim_fades_toward_its_floor_and_never_below() {
        let mut settings = ActivitySettings::default();
        settings.pulse.clock = Clock::Firings;
        let mut a = activity(settings.clone());
        a.fired(GO);
        let go = LayoutNode::Transition(GO);
        a.tick(Duration::from_secs(30));
        // Half way from 1 to the default floor of a quarter.
        assert!(close(a.freshness(), 0.25 + 0.75 * 0.5));
        a.tick(Duration::from_secs(3600));
        assert!(close(a.freshness(), 0.25));
        // The stalled pulse still shows, at a quarter of its glow.
        assert!(close(a.glow(go), 0.25), "{}", a.glow(go));
        // A new floor applies at once, to the fade already made.
        settings.stale_floor = 0.5;
        a.set_settings(settings);
        assert!(close(a.freshness(), 0.5));
        a.fired(OTHER);
        assert_eq!(a.freshness(), 1.0);
    }

    #[test]
    fn peak_and_sum_make_the_glow_from_a_pulse_and_an_ember() {
        let mut settings = ActivitySettings {
            pulse: LayerSettings {
                jump: 0.5,
                ..LayerSettings::PULSE
            },
            ember: LayerSettings {
                jump: 0.25,
                ..LayerSettings::EMBER
            },
            ..ActivitySettings::default()
        };
        let mut a = activity(settings.clone());
        a.fired(GO);
        let go = LayoutNode::Transition(GO);
        // Pulse 0.5, ember 0.25.
        assert!(close(a.glow(go), 0.5), "peak");
        settings.glow = GlowMode::Sum;
        a.set_settings(settings);
        assert!(close(a.glow(go), 0.75), "sum");
        a.fired(GO);
        // Pulse 1, ember 0.5: the sum clamps.
        assert_eq!(a.glow(go), 1.0);
    }

    #[test]
    fn unknown_nodes_are_ignored_and_names_find_ids() {
        let mut a = activity(ActivitySettings::default());
        a.fired(TransitionId::new(99));
        a.added(PlaceId::new(99), 1);
        assert_eq!(a.place(PlaceId::new(99)), None);
        assert_eq!(a.glow(LayoutNode::Place(PlaceId::new(99))), 0.0);
        assert_eq!(a.place_named("Out"), Some(OUT));
        assert_eq!(a.transition_named("Other"), Some(OTHER));
        assert_eq!(a.place_named("Nope"), None);
        assert_eq!(a.place_name(IN), Some("In"));
        assert_eq!(a.transition_name(GO), Some("Go"));
        assert_eq!((a.place_count(), a.transition_count()), (3, 2));
    }

    #[test]
    fn a_model_built_from_names_is_the_one_built_from_the_topology() {
        let topology = topology();
        let named = Activity::named(
            topology.places.iter().map(|p| (p.id, p.name.clone())),
            topology.transitions.iter().map(|t| (t.id, t.name.clone())),
            ActivitySettings::default(),
        );
        assert_eq!(named, activity(ActivitySettings::default()));
        // Out of order, with a gap: each name at its id.
        let a = Activity::named(
            [(OUT, "Out".to_owned()), (IN, "In".to_owned())],
            [(OTHER, "Other".to_owned())],
            ActivitySettings::default(),
        );
        assert_eq!((a.place_count(), a.transition_count()), (2, 2));
        assert_eq!(a.place_named("Out"), Some(OUT));
        assert_eq!(a.transition_name(OTHER), Some("Other"));
        assert_eq!(a.transition_name(GO), Some(""));
    }

    #[test]
    fn nested_and_toggled_events_change_no_level() {
        let mut a = activity(ActivitySettings::default());
        let before = a.clone();
        let token = TokenId::new(0);
        let inner = Event::Fired {
            transition: GO,
            consumed: vec![(IN, token)],
            produced: vec![(OUT, token)],
        };
        let mut b = NetBuilder::new();
        let _ = b.place::<u32>("Unused");
        let nested_id = b.build().expect("an empty net").id();
        a.apply(&Event::Nested {
            place: IN,
            token,
            net: String::new(),
            nested_id,
            event: Box::new(inner),
        });
        a.apply(&Event::Toggled {
            transition: GO,
            enabled: false,
        });
        assert_eq!(a, before);
    }

    #[test]
    fn a_token_changed_in_place_hits_its_place_and_leaves_the_flow_alone() {
        let mut a = activity(ActivitySettings::default());
        a.set_count(IN, 2);
        a.added(IN, 1);
        a.tick(Duration::from_secs(20));
        let before = a.place(IN).unwrap();
        a.apply(&Event::Mutated {
            place: IN,
            token: TokenId::new(0),
        });
        let after = a.place(IN).unwrap();
        assert_eq!(after.pulse, 1.0);
        assert!(
            close(after.ember, before.ember + 0.05),
            "{before:?} {after:?}"
        );
        assert_eq!((after.flow, after.count), (before.flow, before.count));
        assert_eq!(a.firings(), 0, "a change in place is no firing");
        assert_eq!(a.transition(GO).unwrap(), TransitionActivity::default());
        // Through the hit interface, `n` at once: one event, one ember hit.
        a.mutated(OUT, 3);
        assert_eq!(a.place(OUT).unwrap().ember, 0.05);
        assert_eq!(a.place(OUT).unwrap().flow, 0.0);
    }

    #[test]
    fn a_mutating_edge_lights_the_place_it_changed() {
        let mut b = NetBuilder::new();
        let counter = b.place::<u32>("Counter");
        b.seed(counter, [0]);
        let bump = b
            .transition("Bump", (MutateEdge(counter),), (), |n: &mut u32| {
                *n += 1;
            })
            .id();
        let mut net = b.build().expect("the net builds");
        let mut a = Activity::from_net(&net, ActivitySettings::default());
        net.fire(bump).expect("Bump fires");
        drain(&mut net, &mut a);
        let p = a.place(counter.id()).unwrap();
        assert_eq!((p.pulse, p.flow, p.count), (1.0, 0.0, 1));
        assert_eq!(a.transition(bump).unwrap().pulse, 1.0);
    }

    #[test]
    fn a_failed_event_takes_tokens_only_when_no_launch_took_them() {
        let token = TokenId::new(0);
        let failed = |restored| Event::Failed {
            transition: GO,
            consumed: vec![(IN, token)],
            restored,
            error: "no".to_owned(),
        };
        let launched = Event::Launched {
            transition: GO,
            consumed: vec![(IN, token)],
        };
        let start = || {
            let mut a = activity(ActivitySettings::default());
            a.set_count(IN, 1);
            a
        };
        // Staged, lost: the launch took it, and the flow counts it at the
        // failure.
        let mut a = start();
        a.apply(&launched);
        assert_eq!(a.place(IN).unwrap().flow, 0.0, "the flow waits");
        a.apply(&failed(false));
        assert_eq!(a.place(IN).unwrap().count, 0);
        assert_eq!(a.place(IN).unwrap().flow, -0.25);
        // Staged by a resync, lost: the launch was never seen, so the
        // failure's own list counts; the count was set after the launch
        // took the token and does not move.
        let mut a = start();
        a.set_in_flight(GO, true);
        a.apply(&failed(false));
        assert_eq!(a.place(IN).unwrap().count, 1);
        assert_eq!(a.place(IN).unwrap().flow, -0.25);
        assert_eq!(
            a.firings(),
            0,
            "the failure ends a firing counted elsewhere"
        );
        // Staged, restored: back again with a hit, the count as it was
        // before the launch and the flow never moved.
        let mut a = start();
        a.apply(&launched);
        a.tick(Duration::from_secs(5));
        a.apply(&failed(true));
        let p = a.place(IN).unwrap();
        assert_eq!((p.count, p.pulse, p.flow), (1, 1.0, 0.0));
        // Sequential, lost: taken now.
        let mut a = start();
        a.apply(&failed(false));
        assert_eq!(a.place(IN).unwrap().count, 0);
        assert_eq!(a.firings(), 1);
        // Sequential, restored: taken and put back, the place touched.
        let mut a = start();
        a.apply(&failed(true));
        let p = a.place(IN).unwrap();
        assert_eq!((p.count, p.pulse), (1, 1.0));
        assert_eq!(a.transition(GO).unwrap().failure, 1.0);
    }

    #[test]
    fn a_launch_and_its_distribute_are_one_exchange_for_the_flow() {
        let token = TokenId::new(0);
        let mut a = activity(ActivitySettings::default());
        a.set_count(IN, 1);
        a.set_count(SIDE, 1);
        // `IN` loops, taken at the launch and written back at the
        // distribute; `SIDE` only loses its token and `OUT` only gains one.
        a.apply(&Event::Launched {
            transition: GO,
            consumed: vec![(IN, token), (SIDE, token)],
        });
        for place in [IN, SIDE] {
            let p = a.place(place).unwrap();
            assert_eq!((p.count, p.pulse, p.flow), (0, 1.0, 0.0), "{p:?}");
        }
        a.tick(Duration::from_secs(5));
        a.apply(&Event::Distributed {
            transition: GO,
            produced: vec![(IN, token), (OUT, token)],
        });
        let (looped, side, out) = (
            a.place(IN).unwrap(),
            a.place(SIDE).unwrap(),
            a.place(OUT).unwrap(),
        );
        assert_eq!((looped.count, looped.pulse, looped.flow), (1, 1.0, 0.0));
        assert_eq!((side.count, side.flow), (0, -0.25), "drained at the end");
        assert!(side.pulse < 1e-3, "and not hit again: {side:?}");
        assert_eq!((out.count, out.pulse, out.flow), (1, 1.0, 0.25));
        // The take counts once: a distribute with no launch before it moves
        // the flow by only what it lands.
        a.apply(&Event::Distributed {
            transition: GO,
            produced: vec![(OUT, token)],
        });
        assert_eq!(a.place(SIDE).unwrap().flow, -0.25);
    }

    #[test]
    fn the_traffic_light_launched_and_distributed_keeps_its_light_even() {
        let mut light = crate::traffic_light::build_traffic_light().expect("it builds");
        let mut a = Activity::from_net(&light.net, ActivitySettings::default());
        let lamp = light.light.id();
        let firing = light.net.launch(light.change).expect("Change launches");
        drain(&mut light.net, &mut a);
        let p = a.place(lamp).unwrap();
        assert_eq!((p.count, p.flow), (0, 0.0), "out, and no drain shown");
        a.tick(Duration::from_secs(1));
        light.net.distribute(firing.run()).expect("it lands");
        drain(&mut light.net, &mut a);
        let p = a.place(lamp).unwrap();
        assert_eq!((p.count, p.pulse, p.flow), (1, 1.0, 0.0), "{p:?}");
    }

    #[test]
    fn the_account_shows_a_launch_balance_only_at_its_distribute() {
        let token = TokenId::new(0);
        let mut a = activity(ActivitySettings::default());
        a.set_count(IN, 1);
        let start = a.take_delta();
        assert_eq!(
            start.places[&IN].count, 1,
            "the count set is in the account"
        );
        assert!(a.take_delta().is_empty(), "and taking it starts afresh");

        a.apply(&Event::Launched {
            transition: GO,
            consumed: vec![(IN, token)],
        });
        let launch = a.take_delta();
        assert_eq!((launch.firings, launch.lifted), (1, true));
        assert_eq!(
            launch.places[&IN],
            PlaceDelta {
                pulse_hits: 1,
                ember_hits: 1,
                balance: 0,
                count: 0,
            },
            "the token left, and the flow waits"
        );
        let go = launch.transitions[&GO];
        assert_eq!((go.fires, go.distributes, go.in_flight), (1, 0, true));

        // A long tick in flight counts its time.
        a.tick(Duration::from_secs(5));
        a.apply(&Event::Distributed {
            transition: GO,
            produced: vec![(OUT, token)],
        });
        let land = a.take_delta();
        assert_eq!((land.firings, land.lifted), (0, true));
        assert_eq!(
            land.places[&IN],
            PlaceDelta {
                balance: -1,
                ..PlaceDelta::default()
            },
            "the launch's take, counted now"
        );
        assert_eq!(
            land.places[&OUT],
            PlaceDelta {
                pulse_hits: 1,
                ember_hits: 1,
                balance: 1,
                count: 1,
            }
        );
        let go = land.transitions[&GO];
        assert_eq!((go.fires, go.distributes, go.in_flight), (0, 1, false));
        assert!((go.in_flight_ms - 5000.0).abs() < 1e-6, "{go:?}");

        // A token changed in place: a pulse hit and an ember hit, and no
        // balance.
        a.apply(&Event::Mutated { place: OUT, token });
        let changed = a.take_delta();
        assert_eq!(
            changed.places[&OUT],
            PlaceDelta {
                pulse_hits: 1,
                ember_hits: 1,
                balance: 0,
                count: 1,
            }
        );
        assert_eq!((changed.firings, changed.lifted), (0, false));
        assert!(changed.transitions.is_empty());

        // A tick with nothing in flight, or a count set to what stands,
        // leaves the account empty.
        a.tick(Duration::from_secs(1));
        a.set_count(OUT, 1);
        assert!(a.take_delta().is_empty());
    }

    #[test]
    fn the_standing_state_is_the_counts_and_the_flights_with_no_hit() {
        let mut a = activity(ActivitySettings::default());
        a.set_count(IN, 2);
        a.set_count(OUT, 1);
        a.launched(GO);
        a.take_delta();
        let standing = a.standing();
        assert_eq!(standing.firings, 0);
        assert_eq!(
            standing
                .places
                .iter()
                .map(|(p, e)| (*p, *e))
                .collect::<Vec<_>>(),
            [
                (
                    IN,
                    PlaceDelta {
                        count: 2,
                        ..PlaceDelta::default()
                    }
                ),
                (
                    OUT,
                    PlaceDelta {
                        count: 1,
                        ..PlaceDelta::default()
                    }
                ),
            ]
        );
        assert_eq!(
            standing
                .transitions
                .iter()
                .map(|(t, e)| (*t, *e))
                .collect::<Vec<_>>(),
            [(
                GO,
                TransitionDelta {
                    in_flight: true,
                    ..TransitionDelta::default()
                }
            )]
        );
        assert!(a.take_delta().is_empty(), "the account is left alone");
        // Applied to a fresh model: the counts and the flight, and no level
        // but the pulse a flight holds.
        let mut fresh = activity(ActivitySettings::default());
        fresh.apply_delta(&standing);
        assert_eq!(
            fresh.place(IN).map(|p| (p.count, p.pulse, p.ember)),
            Some((2, 0.0, 0.0))
        );
        let go = fresh.transition(GO).unwrap();
        assert_eq!((go.in_flight, go.pulse, go.ember), (true, 1.0, 0.0));
        assert_eq!(fresh.firings(), 0);
    }

    #[test]
    fn a_delta_applied_reproduces_what_the_model_applied() {
        let token = TokenId::new(0);
        let events = [
            Event::Added { place: IN, token },
            Event::Launched {
                transition: GO,
                consumed: vec![(IN, token)],
            },
            Event::Distributed {
                transition: GO,
                produced: vec![(OUT, token), (OUT, token)],
            },
            Event::Mutated { place: OUT, token },
            Event::Fired {
                transition: OTHER,
                consumed: vec![(SIDE, token)],
                produced: vec![(OUT, token)],
            },
            Event::Failed {
                transition: GO,
                consumed: vec![(IN, token)],
                restored: true,
                error: "no".to_owned(),
            },
            Event::Removed { place: OUT, token },
        ];
        for clock in [Clock::Wall, Clock::Firings] {
            let mut settings = ActivitySettings::default();
            settings.pulse.clock = clock;
            settings.ember.clock = clock;
            let mut live = activity(settings.clone());
            live.set_count(SIDE, 1);
            let mut replay = activity(settings);
            replay.apply_delta(&live.take_delta());
            // One event a delta: the order lost is no order at all.
            for event in &events {
                live.apply(event);
                replay.apply_delta(&live.take_delta());
                for place in [IN, OUT, SIDE] {
                    assert_eq!(replay.place(place), live.place(place), "{event:?}");
                }
                for transition in [GO, OTHER] {
                    assert_eq!(
                        replay.transition(transition),
                        live.transition(transition),
                        "{event:?}"
                    );
                }
                assert_eq!(replay.firings(), live.firings());
                live.tick(ms(100));
                replay.tick(ms(100));
                live.take_delta();
                assert_eq!(replay.freshness(), live.freshness());
            }
        }
    }

    fn drain(net: &mut Net, activity: &mut Activity) {
        let events: Vec<Event> = net.drain_events().collect();
        for event in &events {
            activity.apply(event);
        }
    }

    #[test]
    fn the_traffic_light_feeds_the_model_through_its_events() {
        let mut light = crate::traffic_light::build_traffic_light().expect("it builds");
        let mut a = Activity::from_net(&light.net, ActivitySettings::default());
        let (lamp, waiting, passed) = (light.light.id(), light.waiting.id(), light.passed.id());
        assert_eq!(a.place(lamp).unwrap().count, 1, "the seeded red");

        // Red to green: `Change` takes the light and puts the next one back.
        light.net.step().expect("Change fires");
        drain(&mut light.net, &mut a);
        let p = a.place(lamp).unwrap();
        assert_eq!((p.count, p.pulse, p.flow), (1, 1.0, 0.0), "{p:?}");
        assert!(
            close(p.ember, 0.05),
            "one ember hit for the token out and in"
        );
        assert_eq!(a.transition(light.change).unwrap().pulse, 1.0);
        assert_eq!(a.firings(), 1);

        // Two cars from outside, then `Go` drains them both.
        light.net.put(light.waiting, "Taxi".to_owned());
        light.net.put(light.waiting, "Bus".to_owned());
        drain(&mut light.net, &mut a);
        assert_eq!(a.place(waiting).unwrap().count, 2);
        assert!(close(a.place(waiting).unwrap().flow, 0.5));
        a.tick(Duration::from_secs(20));
        light.net.step().expect("Go fires");
        drain(&mut light.net, &mut a);
        let w = a.place(waiting).unwrap();
        assert_eq!((w.count, w.pulse), (0, 1.0));
        assert!(close(w.flow, 0.25 - 0.5), "half of 0.5, less two tokens");
        assert_eq!(a.place(passed).unwrap().count, 2);
        assert!(close(a.place(passed).unwrap().flow, 0.5));
        assert_eq!(a.transition(light.go).unwrap().pulse, 1.0);
        // The light was only read: no hit.
        assert!(a.place(lamp).unwrap().pulse < 1e-5);
    }

    #[test]
    fn a_staged_firing_holds_its_transition_until_it_distributes() {
        let mut b = NetBuilder::new();
        let input = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        b.seed(input, [1]);
        let go = b
            .transition("Go", (TakeEdge(input),), (out,), |n: u32| (n,))
            .id();
        let mut net = b.build().expect("the net builds");
        let mut a = Activity::from_net(&net, ActivitySettings::default());

        let firing = net.launch(go).expect("fireable");
        drain(&mut net, &mut a);
        assert!(a.transition(go).unwrap().in_flight);
        assert_eq!(a.place(input.id()).unwrap().count, 0, "taken at launch");
        assert_eq!(a.place(input.id()).unwrap().flow, 0.0, "the flow waits");
        assert_eq!(a.place(out.id()).unwrap().count, 0);
        // A model built mid-flight knows it too.
        assert!(
            Activity::from_net(&net, ActivitySettings::default())
                .transition(go)
                .unwrap()
                .in_flight
        );
        a.tick(Duration::from_secs(5));
        assert_eq!(a.transition(go).unwrap().pulse, 1.0);

        net.distribute(firing.run()).expect("it lands");
        drain(&mut net, &mut a);
        let t = a.transition(go).unwrap();
        assert!(!t.in_flight);
        assert_eq!(a.place(out.id()).unwrap().count, 1, "landed at distribute");
        assert_eq!(a.place(out.id()).unwrap().pulse, 1.0);
        assert_eq!(a.place(out.id()).unwrap().flow, 0.25);
        assert_eq!(
            a.place(input.id()).unwrap().flow,
            -0.25,
            "drained once it lands"
        );
    }

    #[test]
    fn a_failing_body_fails_its_transition_on_either_path() {
        let mut b = NetBuilder::new();
        let input = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        b.seed(input, [1]);
        let jam = b
            .transition(
                "Jam",
                (TakeEdge(input),),
                (out,),
                |_: u32| -> Result<(u32,), std::fmt::Error> { Err(std::fmt::Error) },
            )
            .restore_tokens_on_failure()
            .id();
        let mut net = b.build().expect("the net builds");
        let mut a = Activity::from_net(&net, ActivitySettings::default());

        net.fire(jam).expect_err("the body fails");
        drain(&mut net, &mut a);
        let t = a.transition(jam).unwrap();
        assert_eq!((t.failure, t.pulse, t.in_flight), (1.0, 1.0, false));
        assert_eq!(a.place(input.id()).unwrap().count, 1, "restored");
        assert_eq!(a.place(input.id()).unwrap().pulse, 1.0, "and touched");

        a.tick(Duration::from_secs(600));
        let firing = net.launch(jam).expect("fireable again");
        drain(&mut net, &mut a);
        assert_eq!(a.place(input.id()).unwrap().count, 0);
        net.distribute(firing.run()).expect_err("it fails again");
        drain(&mut net, &mut a);
        let t = a.transition(jam).unwrap();
        assert_eq!((t.failure, t.in_flight), (1.0, false));
        assert_eq!(a.place(input.id()).unwrap().count, 1, "restored again");
        assert!(
            a.place(input.id()).unwrap().flow.abs() < 1e-6,
            "out and back"
        );
        assert_eq!(a.firings(), 2);
    }
}
