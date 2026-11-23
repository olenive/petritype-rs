//! What the event log knows of tokens: how each one looked when it was put
//! down or changed ([`Look`]), and which tokens stood in each place, in
//! their place's order, at one moment ([`Roster`]).
//!
//! The log keeps a look once for each change of a token: the
//! [`Logged`](crate::Logged) entry that put it down or changed it carries
//! it, and a token that sits unchanged costs nothing more. A roster is the
//! log's base one advanced entry by entry, so that a reader can name the
//! tokens a place held at any entry and say how each looked then.

use std::mem::size_of;

use petritype::style::Rgb;
use petritype::{PlaceId, TokenId, TransitionId};

use crate::log::{Entry, Logged};

/// How a token looked when the log last saw it put down or changed: the
/// label its net rendered for it, cut to a cap, and the colour its runtime
/// gave it.
///
/// The full value of a token is never kept: a label is display text, and a
/// host cuts it to a cap of its own ([`new`](Look::new)), so that one
/// aberrant token cannot use up a log. With the `serde` feature it is
/// `Serialize` and `Deserialize`, the colour written as `#rrggbb` and left
/// out when there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Look {
    /// The label, at most the cap it was made with, cut with [`CUT`](Look::CUT).
    pub label: String,
    /// The colour, or `None` for a token whose runtime gave it none.
    #[cfg_attr(
        feature = "serde",
        serde(default, with = "hex", skip_serializing_if = "Option::is_none")
    )]
    pub colour: Option<Rgb>,
}

impl Look {
    /// The marker a cut label ends with, the one the core and the wire cut
    /// a label with.
    pub const CUT: &'static str = "...";

    /// A look of `label`, kept to its first `cap` characters, on a
    /// character boundary, with [`CUT`](Look::CUT) after them when anything
    /// was cut, by the rule the core cuts a label by; and `colour`.
    ///
    /// ```
    /// use petritype_activity::Look;
    ///
    /// assert_eq!(Look::new("Car 7", None, 200).label, "Car 7");
    /// assert_eq!(Look::new("Hélène", None, 3).label, "Hél...");
    /// ```
    pub fn new(label: &str, colour: Option<Rgb>, cap: usize) -> Look {
        let label = match label.char_indices().nth(cap) {
            Some((byte, _)) => format!("{}{}", &label[..byte], Look::CUT),
            None => label.to_owned(),
        };
        Look { label, colour }
    }

    /// About how many bytes it holds, its label's text among them: what a
    /// host counts against a budget.
    pub fn bytes(&self) -> usize {
        size_of::<Look>() + self.label.len()
    }
}

/// The colour as `#rrggbb`, by the core's own spelling of it.
#[cfg(feature = "serde")]
mod hex {
    use petritype::style::Rgb;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(colour: &Option<Rgb>, s: S) -> Result<S::Ok, S::Error> {
        match colour {
            Some(colour) => s.serialize_some(&colour.to_string()),
            None => s.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Rgb>, D::Error> {
        let text = Option::<String>::deserialize(d)?;
        Ok(text.as_deref().and_then(Rgb::hex))
    }
}

/// The tokens a log can name in each place at one moment, in their place's
/// order, oldest first, and how each one looked then.
///
/// A place's count is the [`Standing`](crate::Standing)'s to say; a roster
/// names as many of them as its log can. A host that reads a net only in
/// part, such as the first page of a place's tokens, names those; a
/// correction of a count names no token, so after one the roster names none
/// in that place until tokens come again. A reader draws the tokens past
/// the ones named as tokens with no name.
///
/// [`advance`](Roster::advance) moves it on by a logged entry, by the
/// core's rules of where tokens go: a firing takes from the front of its
/// places and puts at the back, a failure that restores puts the tokens it
/// took back at the front in the order they were taken, and a token taken
/// by a launch keeps its look until its firing lands or fails. With the
/// `serde` feature it is `Serialize` and `Deserialize`.
///
/// ```
/// use petritype::{PlaceId, TokenId, TransitionId};
/// use petritype_activity::{Entry, Logged, Look, Roster};
///
/// let (queue, done) = (PlaceId::new(0), PlaceId::new(1));
/// let mut roster = Roster::default();
/// roster.set_place(
///     queue,
///     [(TokenId::new(1), Some(Look::new("ann", None, 200))), (TokenId::new(2), None)],
/// );
/// roster.advance(&Logged {
///     seq: 0,
///     at_ms: 0,
///     entry: Entry::Fired {
///         transition: TransitionId::new(0),
///         consumed: vec![(queue, TokenId::new(1))],
///         produced: vec![(done, TokenId::new(3))],
///     },
///     looks: vec![(TokenId::new(3), Look::new("ann, done", None, 200))],
/// });
/// assert_eq!(roster.tokens(queue), [TokenId::new(2)]);
/// assert_eq!(roster.look(TokenId::new(3)).map(|look| &look.label[..]), Some("ann, done"));
/// assert_eq!(roster.look(TokenId::new(1)), None, "gone");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Roster {
    /// Each place with tokens named, in the order of their ids, with the
    /// tokens in the place's order.
    places: Vec<(PlaceId, Vec<TokenId>)>,
    /// The look of each named token, in a place or taken by a launch, in
    /// the order of their ids.
    looks: Vec<(TokenId, Look)>,
    /// What each transition in flight took, in the order of their ids.
    flying: Vec<(TransitionId, Vec<(PlaceId, TokenId)>)>,
}

impl Roster {
    /// The tokens named in `place`, in its order, oldest first: empty for a
    /// place it names none in.
    pub fn tokens(&self, place: PlaceId) -> &[TokenId] {
        self.places
            .binary_search_by_key(&place, |(p, _)| *p)
            .map_or(&[], |at| &self.places[at].1)
    }

    /// How a token looked, if the roster names it and knows its look.
    pub fn look(&self, token: TokenId) -> Option<&Look> {
        self.looks
            .binary_search_by_key(&token, |(t, _)| *t)
            .ok()
            .map(|at| &self.looks[at].1)
    }

    /// Every place with tokens named, in the order of their ids.
    pub fn places(&self) -> impl Iterator<Item = (PlaceId, &[TokenId])> {
        self.places
            .iter()
            .map(|(place, tokens)| (*place, tokens.as_slice()))
    }

    /// Whether it names no token.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.flying.is_empty()
    }

    /// Names `tokens` in `place`, in its order, with the look of each that
    /// has one: what a host reads off a net at the moment its log begins.
    /// The place's names before are replaced.
    pub fn set_place(
        &mut self,
        place: PlaceId,
        tokens: impl IntoIterator<Item = (TokenId, Option<Look>)>,
    ) {
        self.forget_place(place);
        let mut named = Vec::new();
        for (token, look) in tokens {
            named.push(token);
            if let Some(look) = look {
                self.set_look(token, look);
            }
        }
        self.put_place(place, named);
    }

    /// About how many bytes it holds, the labels' text among them.
    pub fn bytes(&self) -> usize {
        let names: usize = self
            .places
            .iter()
            .map(|(_, tokens)| size_of::<(PlaceId, Vec<TokenId>)>() + tokens.len() * 8)
            .sum();
        let looks: usize = self
            .looks
            .iter()
            .map(|(_, look)| size_of::<TokenId>() + look.bytes())
            .sum();
        let flying: usize = self
            .flying
            .iter()
            .map(|(_, taken)| {
                size_of::<(TransitionId, Vec<(PlaceId, TokenId)>)>() + taken.len() * 16
            })
            .sum();
        size_of::<Roster>() + names + looks + flying
    }

    /// Moves the roster on by one logged entry: the tokens it took and put,
    /// by the core's rules, then the looks it carries for the tokens it put
    /// down or changed.
    ///
    /// - [`Fired`](Entry::Fired) takes its consumed tokens and puts its
    ///   produced ones at the back of their places.
    /// - [`Launched`](Entry::Launched) takes its consumed tokens and keeps
    ///   them, with their looks, until the firing lands or fails.
    /// - [`Distributed`](Entry::Distributed) puts its produced tokens at the
    ///   back of their places, and the ones its launch took are gone.
    /// - [`Failed`](Entry::Failed) that restored puts its consumed tokens
    ///   back at the front of their places, in the order they were taken,
    ///   whether a launch took them or the firing took them in one go; one
    ///   that lost them leaves them gone.
    /// - [`Added`](Entry::Added) puts a token at the back, and
    ///   [`Removed`](Entry::Removed) takes one.
    /// - [`Count`](Entry::Count) names no token: the place's names are
    ///   forgotten. [`InFlight`](Entry::InFlight) off forgets what the
    ///   transition took.
    ///
    /// A look the entry carries for a token the roster does not name is
    /// not kept.
    pub fn advance(&mut self, logged: &Logged) {
        match &logged.entry {
            Entry::Fired {
                consumed, produced, ..
            } => {
                for &(place, token) in consumed {
                    self.take(place, token);
                    self.forget_look(token);
                }
                for &(place, token) in produced {
                    self.put_back(place, token);
                }
            }
            Entry::Launched {
                transition,
                consumed,
            } => {
                for &(place, token) in consumed {
                    self.take(place, token);
                }
                self.set_flying(*transition, consumed.clone());
            }
            Entry::Distributed {
                transition,
                produced,
            } => {
                for (_, token) in self.take_flying(*transition) {
                    self.forget_look(token);
                }
                for &(place, token) in produced {
                    self.put_back(place, token);
                }
            }
            Entry::Failed {
                transition,
                consumed,
                restored,
                ..
            } => {
                self.take_flying(*transition);
                if *restored {
                    for &(place, token) in consumed {
                        self.take(place, token);
                    }
                    self.put_front(consumed);
                } else {
                    for &(place, token) in consumed {
                        self.take(place, token);
                        self.forget_look(token);
                    }
                }
            }
            Entry::Added { place, token } => self.put_back(*place, *token),
            Entry::Removed { place, token } => {
                self.take(*place, *token);
                self.forget_look(*token);
            }
            Entry::Mutated { .. } => {}
            Entry::Count { place, .. } => self.forget_place(*place),
            Entry::InFlight { transition, on } => {
                if !on {
                    for (_, token) in self.take_flying(*transition) {
                        self.forget_look(token);
                    }
                }
            }
        }
        for (token, look) in &logged.looks {
            if self.names(*token) {
                self.set_look(*token, look.clone());
            }
        }
    }

    /// The roster that stood before `entries`, when this one stands after
    /// them, as far as the entries can tell: each is undone, the last
    /// first. A token they produced or added was not there before; one they
    /// took is named again, at the front of its place and with no look,
    /// since how it looked is not in an entry; one they changed loses its
    /// look, which was another before; and a place a correction of a count
    /// touched names no token. What a transition in flight before the first
    /// entry had taken is not known. A failure is undone as
    /// [`Standing::worked_back`](crate::Standing::worked_back) undoes it.
    pub fn worked_back(&self, entries: &[Entry]) -> Roster {
        let staged = staged_failures(entries);
        let mut roster = self.clone();
        roster.flying.clear();
        for (entry, staged) in entries.iter().zip(staged).rev() {
            match entry {
                Entry::Fired {
                    consumed, produced, ..
                } => {
                    roster.unput(produced);
                    roster.put_front(consumed);
                }
                Entry::Launched { consumed, .. } => roster.put_front(consumed),
                Entry::Distributed { produced, .. } => roster.unput(produced),
                Entry::Failed {
                    consumed, restored, ..
                } => match (staged, restored) {
                    (true, true) => {
                        for &(place, token) in consumed {
                            roster.take(place, token);
                        }
                    }
                    (false, false) => roster.put_front(consumed),
                    _ => {}
                },
                Entry::Added { place, token } => roster.unput(&[(*place, *token)]),
                Entry::Removed { place, token } => roster.put_front(&[(*place, *token)]),
                Entry::Mutated { token, .. } => roster.forget_look(*token),
                Entry::Count { place, .. } => roster.forget_place(*place),
                Entry::InFlight { .. } => {}
            }
        }
        roster
    }

    /// Whether the roster names `token` in a place or as taken by a launch.
    fn names(&self, token: TokenId) -> bool {
        self.places
            .iter()
            .any(|(_, tokens)| tokens.contains(&token))
            || self
                .flying
                .iter()
                .any(|(_, taken)| taken.iter().any(|(_, t)| *t == token))
    }

    fn set_look(&mut self, token: TokenId, look: Look) {
        match self.looks.binary_search_by_key(&token, |(t, _)| *t) {
            Ok(at) => self.looks[at].1 = look,
            Err(at) => self.looks.insert(at, (token, look)),
        }
    }

    fn forget_look(&mut self, token: TokenId) {
        if let Ok(at) = self.looks.binary_search_by_key(&token, |(t, _)| *t) {
            self.looks.remove(at);
        }
    }

    /// A place's names, to change, made when there are none.
    fn place_mut(&mut self, place: PlaceId) -> &mut Vec<TokenId> {
        let at = match self.places.binary_search_by_key(&place, |(p, _)| *p) {
            Ok(at) => at,
            Err(at) => {
                self.places.insert(at, (place, Vec::new()));
                at
            }
        };
        &mut self.places[at].1
    }

    fn put_place(&mut self, place: PlaceId, tokens: Vec<TokenId>) {
        if !tokens.is_empty() {
            *self.place_mut(place) = tokens;
        }
    }

    /// A place's names forgotten, with their looks.
    fn forget_place(&mut self, place: PlaceId) {
        if let Ok(at) = self.places.binary_search_by_key(&place, |(p, _)| *p) {
            let (_, tokens) = self.places.remove(at);
            for token in tokens {
                self.forget_look(token);
            }
        }
    }

    fn put_back(&mut self, place: PlaceId, token: TokenId) {
        self.place_mut(place).push(token);
    }

    /// The tokens put at the front of their places, in the order given.
    fn put_front(&mut self, tokens: &[(PlaceId, TokenId)]) {
        for &(place, token) in tokens.iter().rev() {
            let named = self.place_mut(place);
            if !named.contains(&token) {
                named.insert(0, token);
            }
        }
    }

    /// The tokens taken out of their places, their looks forgotten.
    fn unput(&mut self, tokens: &[(PlaceId, TokenId)]) {
        for &(place, token) in tokens {
            self.take(place, token);
            self.forget_look(token);
        }
    }

    /// A token taken out of a place, keeping its look; an empty place is
    /// dropped.
    fn take(&mut self, place: PlaceId, token: TokenId) {
        if let Ok(at) = self.places.binary_search_by_key(&place, |(p, _)| *p) {
            let named = &mut self.places[at].1;
            named.retain(|t| *t != token);
            if named.is_empty() {
                self.places.remove(at);
            }
        }
    }

    fn set_flying(&mut self, transition: TransitionId, taken: Vec<(PlaceId, TokenId)>) {
        match self.flying.binary_search_by_key(&transition, |(t, _)| *t) {
            Ok(at) => self.flying[at].1 = taken,
            Err(at) => self.flying.insert(at, (transition, taken)),
        }
    }

    fn take_flying(&mut self, transition: TransitionId) -> Vec<(PlaceId, TokenId)> {
        match self.flying.binary_search_by_key(&transition, |(t, _)| *t) {
            Ok(at) => self.flying.remove(at).1,
            Err(_) => Vec::new(),
        }
    }
}

/// For each of `entries`, whether it is the failure of a transition a
/// launch among the entries before it put in flight, with no landing in
/// between: a staged firing's failure. A failure with no such launch is
/// read as a firing in one go.
pub(crate) fn staged_failures(entries: &[Entry]) -> Vec<bool> {
    let mut flying: Vec<TransitionId> = Vec::new();
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Launched { transition, .. }
            | Entry::InFlight {
                transition,
                on: true,
            } => {
                if !flying.contains(transition) {
                    flying.push(*transition);
                }
                false
            }
            Entry::Distributed { transition, .. }
            | Entry::InFlight {
                transition,
                on: false,
            } => {
                flying.retain(|t| t != transition);
                false
            }
            Entry::Failed { transition, .. } => {
                let staged = flying.contains(transition);
                flying.retain(|t| t != transition);
                staged
            }
            _ => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: PlaceId = PlaceId::new(0);
    const DONE: PlaceId = PlaceId::new(1);
    const GO: TransitionId = TransitionId::new(0);

    fn t(n: u64) -> TokenId {
        TokenId::new(n)
    }

    fn look(label: &str) -> Look {
        Look::new(label, None, 200)
    }

    fn logged(entry: Entry, looks: Vec<(TokenId, Look)>) -> Logged {
        Logged {
            seq: 0,
            at_ms: 0,
            entry,
            looks,
        }
    }

    fn label(roster: &Roster, token: TokenId) -> Option<&str> {
        roster.look(token).map(|look| look.label.as_str())
    }

    #[test]
    fn a_label_is_cut_to_its_cap_on_a_character_boundary_with_the_cores_marker() {
        let long = "x".repeat(250);
        let cut = Look::new(&long, None, 200);
        assert_eq!(cut.label.chars().count(), 203);
        assert!(cut.label.ends_with("..."));
        assert_eq!(&cut.label[..200], &long[..200]);
        assert_eq!(Look::new("abc", None, 3).label, "abc", "nothing cut");
        assert_eq!(Look::new("ééé", None, 2).label, "éé...");
        assert!(Look::new("abc", None, 3).bytes() > 3);
    }

    #[test]
    fn a_firing_moves_names_and_a_launch_keeps_looks_until_it_lands_or_fails() {
        let mut roster = Roster::default();
        roster.set_place(
            QUEUE,
            [(t(1), Some(look("one"))), (t(2), Some(look("two")))],
        );
        // Launched, then put back by a failure: at the front, looks kept.
        roster.advance(&logged(
            Entry::Launched {
                transition: GO,
                consumed: vec![(QUEUE, t(1))],
            },
            vec![],
        ));
        assert_eq!(roster.tokens(QUEUE), [t(2)]);
        assert_eq!(label(&roster, t(1)), Some("one"), "kept while in flight");
        roster.advance(&logged(
            Entry::Failed {
                transition: GO,
                consumed: vec![(QUEUE, t(1))],
                restored: true,
                error: String::new(),
            },
            vec![],
        ));
        assert_eq!(roster.tokens(QUEUE), [t(1), t(2)], "back at the front");
        assert_eq!(label(&roster, t(1)), Some("one"));
        // Launched and landed: the taken one gone, the new one at the back
        // with the look the entry carries.
        roster.advance(&logged(
            Entry::Launched {
                transition: GO,
                consumed: vec![(QUEUE, t(1))],
            },
            vec![],
        ));
        roster.advance(&logged(
            Entry::Distributed {
                transition: GO,
                produced: vec![(DONE, t(3))],
            },
            vec![(t(3), look("one, done")), (t(99), look("unnamed"))],
        ));
        assert_eq!(roster.tokens(DONE), [t(3)]);
        assert_eq!(label(&roster, t(1)), None);
        assert_eq!(label(&roster, t(3)), Some("one, done"));
        assert_eq!(label(&roster, t(99)), None, "a look for no named token");
        // A mutation stores the new look once.
        roster.advance(&logged(
            Entry::Mutated {
                place: DONE,
                token: t(3),
            },
            vec![(t(3), look("changed"))],
        ));
        assert_eq!(label(&roster, t(3)), Some("changed"));
        // A correction names no token.
        roster.advance(&logged(
            Entry::Count {
                place: QUEUE,
                count: 1,
            },
            vec![],
        ));
        assert_eq!(roster.tokens(QUEUE), [] as [TokenId; 0]);
        assert_eq!(label(&roster, t(2)), None);
    }

    #[test]
    fn a_roster_worked_back_undoes_each_entry_and_names_what_was_taken() {
        let entries = vec![
            Entry::Fired {
                transition: GO,
                consumed: vec![(QUEUE, t(1))],
                produced: vec![(DONE, t(5))],
            },
            Entry::Mutated {
                place: QUEUE,
                token: t(2),
            },
            Entry::Added {
                place: QUEUE,
                token: t(6),
            },
        ];
        let mut before = Roster::default();
        before.set_place(
            QUEUE,
            [(t(1), Some(look("one"))), (t(2), Some(look("two")))],
        );
        let mut after = before.clone();
        for entry in &entries {
            after.advance(&logged(entry.clone(), vec![]));
        }
        assert_eq!(after.tokens(QUEUE), [t(2), t(6)]);
        let back = after.worked_back(&entries);
        assert_eq!(back.tokens(QUEUE), before.tokens(QUEUE));
        assert_eq!(back.tokens(DONE), [] as [TokenId; 0]);
        assert_eq!(label(&back, t(1)), None, "taken: no look to tell");
        assert_eq!(label(&back, t(2)), None, "changed: its look before unknown");
    }

    #[test]
    fn a_failure_after_a_launch_among_the_entries_is_staged_and_one_without_is_in_one_go() {
        let fail = Entry::Failed {
            transition: GO,
            consumed: vec![(QUEUE, t(1))],
            restored: true,
            error: String::new(),
        };
        let launch = Entry::Launched {
            transition: GO,
            consumed: vec![(QUEUE, t(1))],
        };
        assert_eq!(
            staged_failures(&[launch.clone(), fail.clone()]),
            [false, true]
        );
        assert_eq!(staged_failures(std::slice::from_ref(&fail)), [false]);
        let land = Entry::Distributed {
            transition: GO,
            produced: vec![],
        };
        assert_eq!(
            staged_failures(&[launch, land, fail]),
            [false, false, false]
        );
    }
}
