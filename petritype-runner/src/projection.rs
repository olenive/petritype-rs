//! A connection's projection of one net through its filter (PROTOCOL.md
//! §13.4, DESIGN §19.3.12).
//!
//! `set_filter` gives a net (the outer one, or an inner one named by `path`
//! or `nested_id`) a [`Projection`]. Reads ask it which tokens to list
//! ([`Projection::kept`], from the net's key index); the owner asks it, event
//! by event, what the connection is told: a token that arrives or changes
//! is checked against the filter where it stands ([`Net::matches`]), and a
//! token that leaves is told of exactly when the connection was shown it,
//! which only the projection can remember, since the token is gone by the
//! time its event is read.

use std::collections::{BTreeSet, HashSet};

use petritype::{Clause, Filter, Net, PlaceId, TokenId};

/// `(place, token)` pairs, as the core's events carry them.
pub(crate) type Pairs = Vec<(PlaceId, TokenId)>;

/// One net's projection: the filter, and what the connection has been shown.
#[derive(Debug)]
pub(crate) struct Projection {
    /// The connection's filter, places by id.
    pub(crate) filter: Filter,
    /// The live tokens the connection has been shown: what the filter kept
    /// as of the last line written. A token keeps its place for as long as
    /// it keeps its id, so the pair names it.
    shown: HashSet<(PlaceId, TokenId)>,
    /// Shown tokens a firing has taken, until it lands or fails: a failure
    /// that restores them names them again, and the connection must be told
    /// exactly those.
    taken: HashSet<(PlaceId, TokenId)>,
}

/// What a token that changed in place is to the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// Shown before and after: `mutated`.
    Mutated,
    /// Hidden before, shown now: `added`, though it did not move.
    Added,
    /// Shown before, hidden now: `removed`, though it did not move.
    Removed,
    /// Hidden before and after: `hidden {kind: "mutated"}`.
    Hidden,
}

impl Projection {
    /// The projection of `net` through `filter`, the connection shown what
    /// the filter keeps now. `set_filter` announces nothing: the client
    /// changed its view and reads it afresh.
    pub(crate) fn new(filter: Filter, net: &Net) -> Projection {
        let mut projection = Projection {
            filter,
            shown: HashSet::new(),
            taken: HashSet::new(),
        };
        projection.shown = projection.kept(net).into_iter().collect();
        projection
    }

    /// Every token the filter keeps now, in place order then token order,
    /// from the net's key index. The filter's places were checked when it
    /// was set, against this net, whose places never change.
    pub(crate) fn kept(&self, net: &Net) -> Pairs {
        net.query_ids(&self.filter).unwrap_or_default()
    }

    /// The tokens of one place the filter keeps now, oldest first.
    pub(crate) fn kept_in(&self, net: &Net, place: PlaceId) -> Vec<TokenId> {
        if self
            .filter
            .places
            .as_ref()
            .is_some_and(|places| !places.contains(&place))
        {
            return Vec::new();
        }
        let within = Filter {
            places: Some(vec![place]),
            ..self.filter.clone()
        };
        net.query_ids(&within)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, token)| token)
            .collect()
    }

    /// Whether the filter keeps the token where it stands now; false when
    /// the net cannot be read (an inner net whose token a body holds).
    pub(crate) fn keeps(&self, net: Option<&Net>, place: PlaceId, token: TokenId) -> bool {
        net.is_some_and(|net| net.matches(&self.filter, place, token))
    }

    /// Whether the filter names a net parameter, so that a change of one
    /// may bring tokens into view or out of it without any token event.
    pub(crate) fn reads_params(&self) -> bool {
        self.filter
            .clauses
            .iter()
            .any(|clause| matches!(clause, Clause::IsParam { .. }))
    }

    /// A token arrived (`added`): whether the connection is told of it.
    pub(crate) fn arrived(&mut self, net: Option<&Net>, place: PlaceId, token: TokenId) -> bool {
        let kept = self.keeps(net, place, token);
        if kept {
            self.shown.insert((place, token));
        }
        kept
    }

    /// A token left from outside (`removed`): whether the connection is
    /// told of it, which is whether it had been shown.
    pub(crate) fn left(&mut self, place: PlaceId, token: TokenId) -> bool {
        self.shown.remove(&(place, token))
    }

    /// A token changed in place (`mutated`). With no net to read, its view
    /// is left as it was.
    pub(crate) fn changed(&mut self, net: Option<&Net>, place: PlaceId, token: TokenId) -> Seen {
        let was = self.shown.contains(&(place, token));
        let now = match net {
            Some(_) => self.keeps(net, place, token),
            None => was,
        };
        match (was, now) {
            (true, true) => Seen::Mutated,
            (false, true) => {
                self.shown.insert((place, token));
                Seen::Added
            }
            (true, false) => {
                self.shown.remove(&(place, token));
                Seen::Removed
            }
            (false, false) => Seen::Hidden,
        }
    }

    /// A firing took these tokens (`launched`, or a one-piece `fired`): the
    /// ones the connection was shown, which are what it is told of.
    pub(crate) fn took(&mut self, consumed: &[(PlaceId, TokenId)]) -> Pairs {
        consumed
            .iter()
            .copied()
            .filter(|pair| {
                let shown = self.shown.remove(pair);
                if shown {
                    self.taken.insert(*pair);
                }
                shown
            })
            .collect()
    }

    /// A firing failed (`failed`): of the tokens it took, those the
    /// connection was shown, shown again if they went back.
    pub(crate) fn failed(&mut self, consumed: &[(PlaceId, TokenId)], restored: bool) -> Pairs {
        let told: Pairs = consumed
            .iter()
            .copied()
            .filter(|pair| self.taken.remove(pair))
            .collect();
        if restored {
            self.shown.extend(told.iter().copied());
        }
        told
    }

    /// A firing landed: what it took is gone for good.
    pub(crate) fn landed(&mut self) {
        self.taken.clear();
    }

    /// A firing placed these tokens (`distributed`, `fired`): the ones the
    /// filter keeps, which are what the connection is told of.
    pub(crate) fn placed(&mut self, net: Option<&Net>, produced: &[(PlaceId, TokenId)]) -> Pairs {
        produced
            .iter()
            .copied()
            .filter(|&(place, token)| self.arrived(net, place, token))
            .collect()
    }

    /// The tokens that came into view or went out of it with no token
    /// event of their own (a change of a net parameter a clause names,
    /// or of the inner nets a key reads), found by comparing what the
    /// connection was shown with what the filter keeps now: over every
    /// token, or over `only` those. Out of view first, then into view, each
    /// in id order, as the Python runner tells them; the projection is
    /// brought up to date.
    pub(crate) fn reconcile(
        &mut self,
        net: &Net,
        only: Option<&[(PlaceId, TokenId)]>,
    ) -> (Pairs, Pairs) {
        let (gone, came): (BTreeSet<_>, BTreeSet<_>) = match only {
            None => {
                let now: HashSet<(PlaceId, TokenId)> = self.kept(net).into_iter().collect();
                let gone = self
                    .shown
                    .difference(&now)
                    .map(|&(place, token)| (token, place))
                    .collect();
                let came = now
                    .difference(&self.shown)
                    .map(|&(place, token)| (token, place))
                    .collect();
                (gone, came)
            }
            Some(only) => {
                let mut gone = BTreeSet::new();
                let mut came = BTreeSet::new();
                for &(place, token) in only {
                    let live = net.tokens(place).any(|(t, _)| t == token);
                    if !live {
                        continue;
                    }
                    let was = self.shown.contains(&(place, token));
                    let now = net.matches(&self.filter, place, token);
                    if was && !now {
                        gone.insert((token, place));
                    } else if now && !was {
                        came.insert((token, place));
                    }
                }
                (gone, came)
            }
        };
        let gone: Pairs = gone
            .into_iter()
            .map(|(token, place)| (place, token))
            .collect();
        let came: Pairs = came
            .into_iter()
            .map(|(token, place)| (place, token))
            .collect();
        for pair in &gone {
            self.shown.remove(pair);
        }
        self.shown.extend(came.iter().copied());
        // A token the connection was shown that has left without an event
        // the projection saw cannot be told of: forget it.
        if only.is_none() {
            self.shown
                .retain(|&(place, token)| net.tokens(place).any(|(t, _)| t == token));
        }
        (gone, came)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::NetBuilder;

    /// `Pond` of numbers keyed by parity, holding 1, 2, 3 and 4.
    fn pond() -> (Net, PlaceId, Vec<TokenId>) {
        let mut b = NetBuilder::new();
        let pond = b.place::<i64>("Pond");
        b.keys(pond, ["even"], |n: &i64| [(n % 2 == 0).into()]);
        b.seed(pond, [1, 2, 3, 4]);
        let net = b.build().unwrap();
        let ids = net.tokens(pond.id()).map(|(t, _)| t).collect();
        (net, pond.id(), ids)
    }

    fn evens() -> Filter {
        Filter::new().key_in("even", [true])
    }

    #[test]
    fn it_starts_from_what_the_filter_keeps() {
        let (net, pond, ids) = pond();
        let projection = Projection::new(evens(), &net);
        assert_eq!(projection.kept(&net), [(pond, ids[1]), (pond, ids[3])]);
        assert_eq!(projection.kept_in(&net, pond), [ids[1], ids[3]]);
        let elsewhere = Projection::new(evens().in_places([]), &net);
        assert!(elsewhere.kept_in(&net, pond).is_empty());
    }

    #[test]
    fn a_taken_token_is_told_of_again_only_if_it_was_shown() {
        let (net, pond, ids) = pond();
        let mut projection = Projection::new(evens(), &net);
        let consumed = [(pond, ids[0]), (pond, ids[1])];
        assert_eq!(projection.took(&consumed), [(pond, ids[1])]);
        assert_eq!(projection.failed(&consumed, true), [(pond, ids[1])]);
        assert!(projection.left(pond, ids[1]), "shown again once restored");
        assert!(projection.took(&consumed).is_empty());
    }

    #[test]
    fn a_change_crosses_the_boundary_both_ways() {
        let (mut net, pond, ids) = pond();
        let mut projection = Projection::new(evens(), &net);
        let place = net.place_ref::<i64>("Pond").unwrap();
        net.replace(place, ids[0], 10).unwrap();
        assert_eq!(projection.changed(Some(&net), pond, ids[0]), Seen::Added);
        net.replace(place, ids[1], 11).unwrap();
        assert_eq!(projection.changed(Some(&net), pond, ids[1]), Seen::Removed);
        assert_eq!(projection.changed(Some(&net), pond, ids[2]), Seen::Hidden);
        assert_eq!(projection.changed(Some(&net), pond, ids[3]), Seen::Mutated);
        assert_eq!(
            projection.changed(None, pond, ids[3]),
            Seen::Mutated,
            "unreadable: left as it was"
        );
    }

    #[test]
    fn reconciling_finds_what_moved_without_an_event() {
        let (mut net, pond, ids) = pond();
        let mut projection = Projection::new(evens(), &net);
        let place = net.place_ref::<i64>("Pond").unwrap();
        // Changed behind the projection's back: no `changed` was asked.
        net.replace(place, ids[0], 10).unwrap();
        net.replace(place, ids[3], 7).unwrap();
        assert_eq!(
            projection.reconcile(&net, Some(&[(pond, ids[3])])),
            (vec![(pond, ids[3])], vec![])
        );
        assert_eq!(
            projection.reconcile(&net, None),
            (vec![], vec![(pond, ids[0])])
        );
        assert_eq!(projection.reconcile(&net, None), (vec![], vec![]));
    }
}
