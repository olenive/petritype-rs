//! The key index: per key name, per value, the tokens that hold it (DESIGN
//! §8.3).
//!
//! What lets [`Net::query`] cost about what it finds. A filter with clauses
//! asks the index which tokens hold each clause's values, starts from the
//! clause that names the fewest, and reads only those tokens; a scan of the
//! marking would read them all. [`Net::distinct_values`] reads the index
//! alone.
//!
//! # Keeping it right
//!
//! The index holds no copy of any token's keys. Keys are derived from the
//! value by the place's extractor, which is pure, so a token's entries are
//! found again by extracting its keys once more, from the value about to
//! leave, or from the value as it stood before a body changed it. Every point
//! where the net moves a token is a point where the value is in hand:
//!
//! - a token **enters** a place through `insert_erased` (seeds, `put`,
//!   `extend`, `set_tokens`, the outputs of a firing, an import) and through
//!   a failed firing that restores what it took;
//! - it **leaves** through a consuming edge at launch, `remove`, and the
//!   clearing `set_tokens` does;
//! - it **changes in place** through a [`MutateEdge`](crate::MutateEdge)
//!   body, whose token's keys are taken before the body runs and after, and
//!   through `replace`, which has the old value and the new;
//! - its inner nets change it, for a nested place whose extractor reads
//!   them: an inner firing of `step_deep`, and `nested_scope`. There the old
//!   value is gone by the time the change is known, and the token's entries
//!   are found by looking through the values of each of its keys
//!   ([`KeyIndex::forget`]).
//!
//! An import replaces every place and rebuilds the index from nothing. The
//! index is never saved: a marking carries values, and keys follow from
//! them.
//!
//! # Keyed edges
//!
//! A [`TakeWhereEdge`](crate::TakeWhereEdge) or
//! [`MutateWhereEdge`](crate::MutateWhereEdge) asks the index the one
//! question it was built to answer quickly: does this place hold a token the
//! filter accepts, and which is the first ([`first_match`])? The clause
//! whose values the fewest tokens hold gives the candidates, already in id
//! order, and the first of them that the other clauses' postings also hold
//! is the answer, found without extracting a single key. What the edge then
//! does goes through the same points as everything else: a token taken from
//! the middle of a place leaves it like one taken from the front, and a
//! token mutated where it stands is re-extracted like a front one.
//!
//! # Token order
//!
//! Each value's tokens are kept as `(PlaceId, TokenId)` in an ordered set, so
//! they come out in place order and, within a place, in id order. A place's
//! queue is in id order too, almost always: tokens join at the back with a
//! fresh id, the largest yet, and leave from anywhere without disturbing the
//! rest. The one exception is a failed firing that restores its tokens to
//! the front of a place that has since received a token older than them
//! (two restoring firings of one place landing out of order). Such a place is
//! marked *unordered* until it next empties, and a query reads its tokens in
//! queue order instead of id order.

use std::any::Any;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::ids::{PlaceId, TokenId};
use crate::keys::{Filter, KeyValue, Keys, Match, Resolved, keys_or_none, match_of};
use crate::net::Net;
use crate::param::ParamSlot;
use crate::place::{PlaceMeta, PlaceSlot};

/// The tokens holding one value of one key, in place order then id order.
type Postings = BTreeSet<(PlaceId, TokenId)>;

/// Per key name, per value, the tokens that hold it. See the module docs.
#[derive(Default)]
pub(crate) struct KeyIndex {
    /// Key name, then value, then the tokens. A value no token holds any more
    /// is removed, and so is a key with no values left, so every value here
    /// is held by at least one token.
    postings: HashMap<&'static str, HashMap<KeyValue, Postings>>,
    /// Places whose queue is not in id order: see "Token order" in the
    /// module docs.
    unordered: HashSet<PlaceId>,
}

impl KeyIndex {
    /// An empty index, for a net with no tokens yet.
    pub(crate) fn new() -> KeyIndex {
        KeyIndex::default()
    }

    /// Indexes a token that has just entered `place`. Nothing for a place
    /// without keys.
    pub(crate) fn enter(
        &mut self,
        meta: &PlaceMeta,
        place: PlaceId,
        token: TokenId,
        value: &dyn Any,
    ) {
        let Some(keys) = &meta.keys else {
            return;
        };
        for (name, value) in keys.names.iter().zip(keys.values(value)) {
            self.postings
                .entry(name)
                .or_default()
                .entry(value)
                .or_default()
                .insert((place, token));
        }
    }

    /// Unindexes a token about to leave `place`, or about to change in it,
    /// found by its keys as they are now.
    pub(crate) fn leave(
        &mut self,
        meta: &PlaceMeta,
        place: PlaceId,
        token: TokenId,
        value: &dyn Any,
    ) {
        let Some(keys) = &meta.keys else {
            return;
        };
        self.leave_values(keys.names.iter().copied(), keys.values(value), place, token);
    }

    /// The values of a token's keys as they are now, to be handed back to
    /// [`changed`](KeyIndex::changed) once something has written to it:
    /// `None` for a place without keys.
    pub(crate) fn before(meta: &PlaceMeta, value: &dyn Any) -> Option<Vec<KeyValue>> {
        meta.keys.as_ref().map(|keys| keys.values(value))
    }

    /// Reindexes a token that has changed in place, given its key values
    /// from before the change ([`before`](KeyIndex::before)).
    pub(crate) fn changed(
        &mut self,
        meta: &PlaceMeta,
        place: PlaceId,
        token: TokenId,
        before: Option<Vec<KeyValue>>,
        value: &dyn Any,
    ) {
        let (Some(keys), Some(before)) = (&meta.keys, before) else {
            return;
        };
        let after = keys.values(value);
        if after == before {
            return;
        }
        self.leave_values(keys.names.iter().copied(), before, place, token);
        self.enter(meta, place, token, value);
    }

    /// Reindexes a token whose old value is gone: its entries are found by
    /// looking through every value of each of its place's keys, then it is
    /// indexed afresh. For a change the net learns of only afterwards: an
    /// inner net of a nested token firing, or a `nested_scope` closure.
    pub(crate) fn forget(
        &mut self,
        meta: &PlaceMeta,
        place: PlaceId,
        token: TokenId,
        value: &dyn Any,
    ) {
        let Some(keys) = &meta.keys else {
            return;
        };
        for name in keys.names.iter() {
            if let Entry::Occupied(mut values) = self.postings.entry(name) {
                values.get_mut().retain(|_, postings| {
                    postings.remove(&(place, token));
                    !postings.is_empty()
                });
                if values.get().is_empty() {
                    values.remove();
                }
            }
        }
        self.enter(meta, place, token, value);
    }

    /// The place has no tokens left: whatever order it was in, it is in id
    /// order now.
    pub(crate) fn emptied(&mut self, place: PlaceId) {
        self.unordered.remove(&place);
    }

    /// A token went back to the front of its place, ahead of `front`, the
    /// token that was first until then. The place stays in id order only if
    /// the newcomer is the older of the two.
    pub(crate) fn pushed_front(&mut self, place: PlaceId, token: TokenId, front: Option<TokenId>) {
        if front.is_some_and(|front| front < token) {
            self.unordered.insert(place);
        }
    }

    /// Forgets everything: the start of an import, which refills every place.
    #[cfg(feature = "serde")]
    pub(crate) fn clear(&mut self) {
        self.postings.clear();
        self.unordered.clear();
    }

    /// Removes one token from the postings of each of these values.
    fn leave_values(
        &mut self,
        names: impl Iterator<Item = &'static str>,
        values: Vec<KeyValue>,
        place: PlaceId,
        token: TokenId,
    ) {
        for (name, value) in names.zip(values) {
            let Entry::Occupied(mut values) = self.postings.entry(name) else {
                continue;
            };
            if let Entry::Occupied(mut entry) = values.get_mut().entry(value) {
                entry.get_mut().remove(&(place, token));
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
            if values.get().is_empty() {
                values.remove();
            }
        }
    }

    /// The first token of `place` in queue order whose keys pass every
    /// resolved clause, with where it stands: [`first_match`] once the
    /// place is known to declare keys and the filter to have clauses.
    ///
    /// Asked of the postings alone. Per clause, the sets of the values it
    /// accepts; a key or a clause no token holds ends the question at once.
    /// In a place in id order the candidates are the tokens of this place
    /// holding the clause with the fewest tokens, walked in id order per
    /// value until one is held by every other clause too; the earliest of
    /// those is the answer, and its position is found by binary search. An
    /// unordered place is walked in queue order instead.
    fn first_in(
        &self,
        slot: &PlaceSlot,
        place: PlaceId,
        resolved: &Resolved<'_>,
    ) -> Option<(usize, TokenId)> {
        let mut clauses: Vec<Vec<&Postings>> = Vec::with_capacity(resolved.clauses.len());
        for clause in &resolved.clauses {
            let values = self.postings.get(clause.key)?;
            let sets: Vec<&Postings> = clause
                .values()
                .iter()
                .filter_map(|value| values.get(value))
                .collect();
            if sets.is_empty() {
                return None;
            }
            clauses.push(sets);
        }
        let passes = |token: TokenId| {
            clauses.iter().all(|sets| {
                sets.iter()
                    .any(|postings| postings.contains(&(place, token)))
            })
        };

        if self.unordered.contains(&place) {
            return slot
                .tokens
                .iter()
                .position(|(token, _)| passes(*token))
                .map(|at| (at, slot.tokens[at].0));
        }

        let fewest = clauses
            .iter()
            .min_by_key(|sets| sets.iter().map(|postings| postings.len()).sum::<usize>())?;
        let here = (place, TokenId(0))..=(place, TokenId(u64::MAX));
        let token = fewest
            .iter()
            .filter_map(|postings| {
                postings
                    .range(here.clone())
                    .map(|&(_, token)| token)
                    .find(|&token| passes(token))
            })
            .min()?;
        let at = slot.tokens.binary_search_by_key(&token, |(id, _)| *id);
        debug_assert!(
            at.is_ok(),
            "the index names {token:?} in {place:?}, which does not hold it"
        );
        Some((at.ok()?, token))
    }

    /// The index a net with these places would have if it were built from
    /// nothing: what the incremental one is checked against.
    #[cfg(test)]
    pub(crate) fn rebuild(places: &[PlaceSlot]) -> KeyIndex {
        let mut index = KeyIndex::new();
        for (position, slot) in places.iter().enumerate() {
            let place = PlaceId(position as u32);
            for (token, value) in &slot.tokens {
                index.enter(&slot.meta, place, *token, &**value);
            }
            if !slot.tokens.iter().is_sorted_by_key(|(token, _)| *token) {
                index.unordered.insert(place);
            }
        }
        index
    }

    /// Whether this index says what a fresh one would: the same postings,
    /// and every place that is out of id order marked so (a mark on a place
    /// that has come back into order is allowed, and only costs a query a
    /// pass over that place).
    #[cfg(test)]
    pub(crate) fn agrees_with(&self, places: &[PlaceSlot]) -> Result<(), String> {
        let fresh = KeyIndex::rebuild(places);
        if self.postings != fresh.postings {
            return Err(format!(
                "postings differ:\n  kept:  {:?}\n  fresh: {:?}",
                self.postings, fresh.postings
            ));
        }
        if let Some(place) = fresh.unordered.difference(&self.unordered).next() {
            return Err(format!("{place:?} is out of id order but not marked"));
        }
        Ok(())
    }
}

/// The first token of `place`, in queue order, whose keys pass every clause
/// of `filter`, its parameters read from `params`, with its position in the
/// place: the token a keyed input edge takes or hands over (DESIGN §8.3).
/// `None` when there is none, or no such place.
///
/// The filter's `places` and `prefix` are not consulted: an edge reads one
/// place, and says which. A filter without clauses accepts the front token;
/// one with clauses accepts nothing in a place without keys.
pub(crate) fn first_match(
    places: &[PlaceSlot],
    params: &[ParamSlot],
    index: &KeyIndex,
    place: PlaceId,
    filter: &Filter,
) -> Option<(usize, TokenId)> {
    let slot = places.get(place.0 as usize)?;
    if filter.clauses.is_empty() {
        return slot.tokens.front().map(|(token, _)| (0, *token));
    }
    slot.meta.keys.as_ref()?;
    index.first_in(slot, place, &filter.resolve_in(params))
}

/// [`first_match`] by reading the place front to back and extracting each
/// token's keys: what the indexed answer is checked against.
#[cfg(test)]
pub(crate) fn first_match_scan(
    places: &[PlaceSlot],
    params: &[ParamSlot],
    place: PlaceId,
    filter: &Filter,
) -> Option<(usize, TokenId)> {
    let slot = places.get(place.0 as usize)?;
    if !filter.clauses.is_empty() && slot.meta.keys.is_none() {
        return None;
    }
    let resolved = filter.resolve_in(params);
    slot.tokens
        .iter()
        .position(|(_, value)| resolved.accepts(&keys_or_none(slot, &**value)))
        .map(|at| (at, slot.tokens[at].0))
}

/// How many values each key has, never the values or the tokens: a net's
/// `Debug` does not print its marking in bulk.
impl fmt::Debug for KeyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let values: BTreeMap<&str, usize> = self
            .postings
            .iter()
            .map(|(name, values)| (*name, values.len()))
            .collect();
        f.debug_struct("KeyIndex")
            .field("values", &values)
            .field("unordered", &self.unordered.len())
            .finish()
    }
}

impl Net {
    /// [`Net::query`] once the filter's places are known to exist: from the
    /// index when there are clauses, by reading the covered places when
    /// there are none.
    pub(crate) fn query_indexed(&self, filter: &Filter) -> Vec<Match> {
        let mut found = Vec::new();
        self.visit_matches(filter, |slot, place, token, value, keys| {
            let keys = keys.unwrap_or_else(|| keys_or_none(slot, value));
            found.push(match_of(slot, place, token, value, keys, self.label_limit));
        });
        found
    }

    /// [`Net::query_ids`] once the filter's places are known to exist: the
    /// same tokens as [`query_indexed`](Net::query_indexed), with nothing
    /// rendered.
    pub(crate) fn query_ids_indexed(&self, filter: &Filter) -> Vec<(PlaceId, TokenId)> {
        let mut found = Vec::new();
        self.visit_matches(filter, |_, place, token, _, _| found.push((place, token)));
        found
    }

    /// Calls `found` with each token `filter` lets through, in place order
    /// then token order: its place's slot, the place, the token, its value,
    /// and its keys when they were extracted to decide (`None` for a filter
    /// without clauses, which keeps every token of the places it covers and
    /// extracts nothing).
    fn visit_matches(
        &self,
        filter: &Filter,
        mut found: impl FnMut(&PlaceSlot, PlaceId, TokenId, &dyn Any, Option<Keys>),
    ) {
        let resolved = filter.resolve_in(&self.params);

        if resolved.clauses.is_empty() {
            for (position, slot) in self.places.iter().enumerate() {
                let place = PlaceId(position as u32);
                if !filter.covers(place, &slot.meta.name) {
                    continue;
                }
                for (token, value) in &slot.tokens {
                    found(slot, place, *token, &**value, None);
                }
            }
            return;
        }

        // The candidates: the tokens holding one of the values of the clause
        // whose values the fewest tokens hold. A value a clause names twice
        // is counted once, and a key no token holds ends the query at once.
        let mut best: Option<(usize, Vec<&Postings>)> = None;
        for clause in &resolved.clauses {
            let Some(values) = self.index.postings.get(clause.key) else {
                return;
            };
            let mut seen: Vec<&KeyValue> = Vec::new();
            let mut sets = Vec::new();
            for value in clause.values() {
                if seen.contains(&value) {
                    continue;
                }
                seen.push(value);
                if let Some(postings) = values.get(value) {
                    sets.push(postings);
                }
            }
            let held: usize = sets.iter().map(|postings| postings.len()).sum();
            if held == 0 {
                return;
            }
            if best.as_ref().is_none_or(|(fewest, _)| held < *fewest) {
                best = Some((held, sets));
            }
        }
        let Some((held, sets)) = best else {
            return;
        };
        let mut candidates: Vec<(PlaceId, TokenId)> = Vec::with_capacity(held);
        for postings in sets {
            candidates.extend(postings.iter().copied());
        }
        // One token has one value per key, so the sets are disjoint, and
        // sorting puts them in place order then id order.
        candidates.sort_unstable();

        for group in candidates.chunk_by(|a, b| a.0 == b.0) {
            let place = group[0].0;
            let slot = &self.places[place.0 as usize];
            if !filter.covers(place, &slot.meta.name) {
                continue;
            }
            let mut keep = |token: TokenId, value: &dyn Any| {
                let keys = keys_or_none(slot, value);
                if resolved.accepts(&keys) {
                    found(slot, place, token, value, Some(keys));
                }
            };
            if self.index.unordered.contains(&place) {
                // Queue order is not id order here: one pass, in queue order.
                for (token, value) in &slot.tokens {
                    if group
                        .binary_search_by_key(token, |(_, token)| *token)
                        .is_ok()
                    {
                        keep(*token, &**value);
                    }
                }
            } else {
                for &(_, token) in group {
                    let found_at = slot.tokens.binary_search_by_key(&token, |(id, _)| *id);
                    debug_assert!(
                        found_at.is_ok(),
                        "the index names {token:?} in {place:?}, which does not hold it"
                    );
                    if let Ok(position) = found_at {
                        keep(token, &*slot.tokens[position].1);
                    }
                }
            }
        }
    }

    /// [`Net::distinct_values`], from the index: each value's first token in
    /// the places asked about, ordered by place and then by where it stands
    /// in its place.
    pub(crate) fn distinct_indexed(&self, key: &str, only: Option<&[PlaceId]>) -> Vec<KeyValue> {
        let Some(values) = self.index.postings.get(key) else {
            return Vec::new();
        };
        // The places to look in, in place order, or every place.
        let only: Option<Vec<PlaceId>> = only.map(|only| {
            let mut only = only.to_vec();
            only.sort_unstable();
            only.dedup();
            only
        });
        let mut firsts: Vec<((PlaceId, u64), &KeyValue)> = Vec::with_capacity(values.len());
        for (value, postings) in values {
            let first = match &only {
                None => postings
                    .first()
                    .map(|&(place, _)| (place, self.earliest(postings, place))),
                Some(only) => only.iter().find_map(|&place| {
                    let rank = self.earliest(postings, place);
                    (rank != u64::MAX).then_some((place, rank))
                }),
            };
            if let Some(first) = first {
                firsts.push((first, value));
            }
        }
        firsts.sort_unstable_by_key(|(first, _)| *first);
        firsts.into_iter().map(|(_, value)| value.clone()).collect()
    }

    /// Where the first of these tokens stands in `place`, as a number that
    /// orders the tokens of that one place: its id in a place in id order,
    /// its position in an unordered one; `u64::MAX` if none is there.
    fn earliest(&self, postings: &Postings, place: PlaceId) -> u64 {
        let mut here = postings.range((place, TokenId(0))..=(place, TokenId(u64::MAX)));
        if !self.index.unordered.contains(&place) {
            return here.next().map_or(u64::MAX, |(_, token)| token.0);
        }
        let held: HashSet<TokenId> = here.map(|&(_, token)| token).collect();
        self.places[place.0 as usize]
            .tokens
            .iter()
            .position(|(token, _)| held.contains(token))
            .map_or(u64::MAX, |position| position as u64)
    }

    /// Whether the index agrees with one built afresh from the marking.
    #[cfg(test)]
    pub(crate) fn index_agrees(&self) -> Result<(), String> {
        self.index.agrees_with(&self.places)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{distinct_scan, query_scan};
    use crate::{
        Command, KeyValue, MutateEdge, MutateWhereEdge, NetBuilder, PlaceRef, TakeAllEdge,
        TakeEdge, TakeWhereEdge,
    };

    #[derive(Debug, Clone, PartialEq)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    struct Animal {
        name: String,
        class: String,
        legs: i64,
    }

    fn animal(name: &str, class: &str, legs: i64) -> Animal {
        Animal {
            name: name.to_owned(),
            class: class.to_owned(),
            legs,
        }
    }

    #[derive(Debug)]
    struct Broken;

    impl std::fmt::Display for Broken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("broken")
        }
    }

    impl std::error::Error for Broken {}

    fn declare_keys(b: &mut NetBuilder, place: PlaceRef<Animal>) {
        b.keys(place, ["name", "class", "legs"], |a: &Animal| {
            [
                a.name.as_str().into(),
                a.class.as_str().into(),
                a.legs.into(),
            ]
        });
    }

    /// `Field` and `Barn` declare keys, `Pen` does not; transitions that
    /// take one, take all, mutate (renaming, so a key changes) and fail, and
    /// keyed ones that take a bird, shear a mammal (changing a key) and fail
    /// on a bird, restoring it.
    struct Farm {
        net: Net,
        field: PlaceRef<Animal>,
        barn: PlaceRef<Animal>,
        pen: PlaceRef<Animal>,
    }

    fn farm() -> Farm {
        let mut b = NetBuilder::new();
        let field = b.place::<Animal>("Field");
        let barn = b.place::<Animal>("Barn");
        let pen = b.place::<Animal>("Pen");
        declare_keys(&mut b, field);
        declare_keys(&mut b, barn);
        #[cfg(feature = "serde")]
        for place in [field, barn, pen] {
            b.serializable(place);
        }
        b.seed(
            field,
            [
                animal("Hen", "Bird", 2),
                animal("Cow", "Mammal", 4),
                animal("Duck", "Bird", 2),
            ],
        );
        b.seed(barn, [animal("Cat", "Mammal", 4)]);
        b.seed(pen, [animal("Pig", "Mammal", 4)]);
        b.transition("Move", (TakeEdge(field),), (barn,), |a: Animal| (a,));
        b.transition("Herd", (TakeAllEdge(field),), (pen,), |all: Vec<Animal>| {
            (all,)
        });
        b.transition("Rename", (MutateEdge(barn),), (), |a: &mut Animal| {
            a.name.push('!');
            a.legs += 1;
        });
        b.transition(
            "Fail",
            (TakeEdge(barn),),
            (field,),
            |_: Animal| -> Result<(Animal,), Broken> { Err(Broken) },
        )
        .restore_tokens_on_failure();
        b.transition(
            "Pick",
            (TakeWhereEdge(
                field,
                Filter::new().key_in("class", ["Bird"]),
            ),),
            (barn,),
            |a: Animal| (a,),
        );
        b.transition(
            "Shear",
            (MutateWhereEdge(
                barn,
                Filter::new().key_in("class", ["Mammal"]),
            ),),
            (),
            |a: &mut Animal| {
                a.name.push('~');
                a.legs -= 1;
            },
        );
        b.transition(
            "Drop a bird",
            (TakeWhereEdge(barn, Filter::new().key_in("class", ["Bird"])),),
            (field,),
            |_: Animal| -> Result<(Animal,), Broken> { Err(Broken) },
        )
        .restore_tokens_on_failure();
        let mut net = b.build().expect("the farm builds");
        // Every transition fires only when a test fires it by name.
        let all: Vec<_> = net.transitions().map(|(id, _)| id).collect();
        for transition in all {
            net.disable(transition).unwrap();
        }
        Farm {
            net,
            field,
            barn,
            pen,
        }
    }

    fn fire(net: &mut Net, name: &str) -> Result<(), crate::NetError> {
        let transition = net.transition_named(name).expect("a transition");
        net.enable(transition)?;
        let fired = net.fire(transition);
        net.disable(transition)?;
        fired
    }

    /// The index agrees with one built afresh, and the indexed answers with
    /// the scanning ones, for a spread of filters and every key.
    fn check(net: &Net) {
        net.index_agrees().unwrap();
        let places: Vec<PlaceId> = net.places().map(|(id, _)| id).collect();
        let mut filters = vec![
            Filter::new(),
            Filter::new().key_in("class", ["Nothing"]),
            Filter::new().key_in("nope", ["Bird"]),
            Filter::new().key_in("legs", [KeyValue::Int(2), KeyValue::Str("4".into())]),
            Filter::new().key_in("class", ["Bird", "Bird"]),
            Filter::new().key_in("class", ["Mammal"]).with_prefix("B"),
            Filter::new().with_prefix("P"),
        ];
        for key in ["name", "class", "legs"] {
            for value in distinct_scan(&net.places, key, None) {
                filters.push(Filter::new().key_in(key, [value.clone()]));
                filters.push(
                    Filter::new()
                        .key_in(key, [value.clone()])
                        .key_in("class", ["Bird", "Mammal"])
                        .in_places(places[..2].iter().copied()),
                );
            }
        }
        for filter in &filters {
            let scanned = query_scan(&net.places, &net.params, net.label_limit, filter).unwrap();
            assert_eq!(net.query(filter).unwrap(), scanned, "{filter:?}");
            // What a keyed edge would choose, from the index and by reading
            // the place front to back.
            for &place in &places {
                assert_eq!(
                    first_match(&net.places, &net.params, &net.index, place, filter),
                    first_match_scan(&net.places, &net.params, place, filter),
                    "{filter:?} in {place:?}"
                );
            }
        }
        for key in ["name", "class", "legs", "nope"] {
            assert_eq!(
                net.distinct_values(key, None),
                distinct_scan(&net.places, key, None),
                "{key}"
            );
            for place in &places {
                let only = [*place];
                assert_eq!(
                    net.distinct_values(key, Some(&only)),
                    distinct_scan(&net.places, key, Some(&only)),
                    "{key} in {place:?}"
                );
            }
        }
    }

    #[test]
    fn the_index_follows_every_kind_of_change() {
        let Farm {
            mut net,
            field,
            barn,
            pen,
        } = farm();
        check(&net);

        let fox = net.put(field, animal("Fox", "Mammal", 4));
        check(&net);
        net.extend(barn, [animal("Owl", "Bird", 2), animal("Bee", "Insect", 6)]);
        check(&net);
        net.replace(field, fox, animal("Vixen", "Mammal", 3))
            .unwrap();
        check(&net);
        net.remove(field.id(), fox).unwrap();
        check(&net);
        net.set_tokens(barn, [animal("Rat", "Mammal", 4)]);
        check(&net);
        net.set_tokens(pen, [animal("Goat", "Mammal", 4)]);
        check(&net);

        fire(&mut net, "Move").unwrap();
        check(&net);
        fire(&mut net, "Rename").unwrap();
        check(&net);
        fire(&mut net, "Herd").unwrap();
        check(&net);
        assert_eq!(net.count(field.id()), 0);
        fire(&mut net, "Fail").expect_err("the body fails, the token goes back");
        check(&net);

        net.apply(Command::put(field, animal("Elk", "Mammal", 4)))
            .unwrap();
        check(&net);

        // The staged path: launch, distribute.
        let rename = net.transition_named("Rename").unwrap();
        net.enable(rename).unwrap();
        let firing = net.launch(rename).unwrap();
        check(&net);
        net.distribute(firing.run()).unwrap();
        check(&net);
    }

    /// The keyed edges move tokens through the same points as the plain
    /// ones: a token taken from the middle leaves the index, one sheared
    /// where it stands is indexed again, and one put back at the front marks
    /// its place out of order.
    #[test]
    fn the_index_follows_the_keyed_edges() {
        let Farm {
            mut net,
            field,
            barn,
            ..
        } = farm();
        net.set_tokens(
            field,
            [
                animal("Cow", "Mammal", 4),
                animal("Hen", "Bird", 2),
                animal("Ram", "Mammal", 4),
                animal("Duck", "Bird", 2),
            ],
        );
        check(&net);

        fire(&mut net, "Pick").unwrap();
        check(&net);
        let names: Vec<String> = net
            .query(&Filter::new().in_places([field.id()]))
            .unwrap()
            .into_iter()
            .map(|found| found.keys.get("name").unwrap().to_string())
            .collect();
        assert_eq!(names, ["Cow", "Ram", "Duck"], "the hen, from between them");

        fire(&mut net, "Shear").unwrap();
        check(&net);
        fire(&mut net, "Pick").unwrap();
        check(&net);

        // The barn holds Cat~ (sheared), Hen, Duck; a failing pick of a bird
        // puts the hen back in front of the cat, out of id order.
        assert!(!net.index.unordered.contains(&barn.id()));
        fire(&mut net, "Drop a bird").expect_err("the body fails, the hen goes back");
        check(&net);
        assert!(net.index.unordered.contains(&barn.id()));
        let birds = Filter::new().key_in("class", ["Bird"]);
        let (front, _) = net.tokens(barn.id()).next().unwrap();
        assert_eq!(
            first_match(&net.places, &net.params, &net.index, barn.id(), &birds),
            Some((0, front)),
            "the restored hen, first in queue order"
        );
        fire(&mut net, "Shear").unwrap();
        check(&net);
    }

    /// Two restoring firings of one place land in the opposite order to the
    /// one they launched in, behind a token put in meanwhile: the place is
    /// out of id order, and a query and a distinct still follow the queue.
    #[test]
    fn a_place_out_of_id_order_is_read_in_queue_order() {
        let mut b = NetBuilder::new();
        let field = b.place::<Animal>("Field");
        let out = b.place::<Animal>("Out");
        declare_keys(&mut b, field);
        b.seed(
            field,
            [animal("Hen", "Bird", 2), animal("Cow", "Mammal", 4)],
        );
        let failing = |_: Animal| -> Result<(Animal,), Broken> { Err(Broken) };
        let first = b
            .transition("First", (TakeEdge(field),), (out,), failing)
            .restore_tokens_on_failure()
            .id();
        let second = b
            .transition("Second", (TakeEdge(field),), (out,), failing)
            .restore_tokens_on_failure()
            .id();
        let mut net = b.build().unwrap();
        let hen_first = net.launch(first).unwrap();
        let cow_second = net.launch(second).unwrap();
        check(&net);
        net.put(field, animal("Owl", "Bird", 2));
        check(&net);
        net.distribute(hen_first.run()).unwrap_err();
        check(&net);
        net.distribute(cow_second.run()).unwrap_err();
        check(&net);

        let order: Vec<String> = net
            .query(&Filter::new().key_in("class", ["Bird", "Mammal"]))
            .unwrap()
            .into_iter()
            .map(|found| found.keys.get("name").unwrap().to_string())
            .collect();
        assert_eq!(order, ["Cow", "Hen", "Owl"], "queue order, not id order");
        assert_eq!(
            net.distinct_values("class", None),
            [KeyValue::from("Mammal"), KeyValue::from("Bird")]
        );

        // Emptied, the place is in order again.
        net.set_tokens(field, []);
        check(&net);
        assert!(!net.index.unordered.contains(&field.id()));
    }

    /// A key that reads a nested token's inner net changes when the inner
    /// net fires, or when `nested_scope` writes to it.
    #[test]
    fn a_key_that_reads_an_inner_net_follows_it() {
        let mut inner = NetBuilder::new();
        let todo = inner.place::<u32>("Todo");
        inner.seed(todo, [1, 2]);
        inner.transition("Done", (TakeEdge(todo),), (), |_: u32| {});
        let inner = inner.build().unwrap();

        let mut b = NetBuilder::new();
        let desk = b.place_nested::<Net>("Desk");
        b.keys(desk, ["todo"], move |net: &Net| {
            [KeyValue::Int(net.count(todo.id()) as i64)]
        });
        b.seed(desk, [inner]);
        let mut net = b.build().unwrap();
        let todo_is = |net: &Net, n: i64| {
            net.query(&Filter::new().key_in("todo", [KeyValue::Int(n)]))
                .unwrap()
                .len()
        };
        net.index_agrees().unwrap();
        assert_eq!(todo_is(&net, 2), 1);

        net.step_deep().unwrap().expect("the inner net fires");
        net.index_agrees().unwrap();
        assert_eq!((todo_is(&net, 2), todo_is(&net, 1)), (0, 1));

        let (token, _) = net.tokens(desk.id()).next().unwrap();
        net.nested_scope(desk.id(), token, |nets| nets[0].1.put(todo, 9));
        net.index_agrees().unwrap();
        assert_eq!(todo_is(&net, 2), 1);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn an_import_rebuilds_the_index() {
        let Farm { mut net, .. } = farm();
        let marking: crate::Marking = serde_json::from_value(serde_json::json!({
            "Field": [{"name": "Yak", "class": "Mammal", "legs": 4}],
            "Barn": [{"name": "Emu", "class": "Bird", "legs": 2},
                     {"name": "Yak", "class": "Mammal", "legs": 4}],
        }))
        .unwrap();
        net.import_marking(&marking).unwrap();
        check(&net);
        assert_eq!(
            net.distinct_values("name", None),
            [KeyValue::from("Yak"), KeyValue::from("Emu")]
        );
    }
}
