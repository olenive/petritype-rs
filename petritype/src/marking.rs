//! A net's marking as JSON, out and back in: [`Marking`],
//! [`Net::export_marking`] and [`Net::import_marking`] (DESIGN §9, `serde`
//! feature).

use std::collections::BTreeMap;

use crate::error::{CommandError, NetError};
use crate::event::Event;
use crate::ids::PlaceId;
use crate::nested::forget_inner_events;
use crate::net::Net;
use crate::place::ErasedToken;

/// Every place's tokens as JSON, by place name, oldest first: what
/// [`Net::export_marking`] writes and [`Net::import_marking`] reads.
///
/// ```json
/// {"Door": ["Ada"], "Seats": ["Bram", "Cato"], "Standing": []}
/// ```
///
/// Places are keyed by name, as on the rest of the JSON route, because a
/// name is what a saved file can know; they are kept in name order, so the
/// same marking always prints the same text. Each token is in the JSON form
/// of its place's type. Token ids are not part of it: a marking says *what*
/// is where, and an import gives every token a fresh id.
///
/// It is plain data — `Clone`, `PartialEq`, `Serialize`, `Deserialize`,
/// transparent over the map — so two markings compare by place name, token
/// order and value, which is what a test of a save wants.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Marking {
    places: BTreeMap<String, Vec<serde_json::Value>>,
}

impl Marking {
    /// A marking with no places in it, which
    /// [`import_marking`](Net::import_marking) reads as "everything empty".
    pub fn new() -> Self {
        Marking::default()
    }

    /// The tokens of the place with this name, oldest first, or `None` if
    /// the marking does not mention the place.
    pub fn get(&self, place: &str) -> Option<&[serde_json::Value]> {
        self.places.get(place).map(Vec::as_slice)
    }

    /// Sets the tokens of one place, oldest first, and returns what the
    /// marking held for it before.
    pub fn insert(
        &mut self,
        place: impl Into<String>,
        tokens: Vec<serde_json::Value>,
    ) -> Option<Vec<serde_json::Value>> {
        self.places.insert(place.into(), tokens)
    }

    /// Every place the marking mentions, in name order, with its tokens.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[serde_json::Value])> {
        self.places
            .iter()
            .map(|(name, tokens)| (name.as_str(), tokens.as_slice()))
    }

    /// How many places the marking mentions.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    /// Whether the marking mentions no place at all.
    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }
}

impl Net {
    /// Every place's tokens as JSON, by place name, oldest first (DESIGN
    /// §9).
    ///
    /// What a save writes and [`import_marking`](Net::import_marking) reads
    /// back, into this net or into a fresh build of the same one. Every
    /// place must be marked [`serializable`](crate::NetBuilder::serializable),
    /// a [nested](crate::NetBuilder::place_nested) one included, whose
    /// tokens then carry their inner nets in the form `Net`'s `Serialize`
    /// gives them: so the marking of an outer net reaches all the way down.
    /// Empty places are listed too, as `[]`.
    ///
    /// ```
    /// use petritype::NetBuilder;
    /// use petritype::serde_json::{self, json};
    ///
    /// let mut b = NetBuilder::new();
    /// let seats = b.place::<String>("Seats");
    /// let door = b.place::<String>("Door");
    /// b.serializable(seats);
    /// b.serializable(door);
    /// b.seed(seats, ["Bram".to_owned(), "Cato".to_owned()]);
    /// let net = b.build()?;
    ///
    /// let marking = net.export_marking()?;
    /// assert_eq!(
    ///     serde_json::to_value(&marking)?,
    ///     json!({"Door": [], "Seats": ["Bram", "Cato"]}),
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Reading is not a change: nothing is emitted and no counter moves.
    /// Tokens airborne with a firing [in flight](Net::in_flight) are in no
    /// place and so in no marking; a driver returns with nothing in flight,
    /// which is when to save.
    ///
    /// # Errors
    ///
    /// [`NetError::NotSerializable`] naming the first place, in id order,
    /// that was not marked — every place is checked before any token is
    /// encoded — and [`NetError::CannotSerialize`] for a token whose own
    /// `Serialize` refused, such as one holding a net that no template
    /// built.
    pub fn export_marking(&self) -> Result<Marking, NetError> {
        let mut encoders = Vec::with_capacity(self.places.len());
        for (index, slot) in self.places.iter().enumerate() {
            let to_json = slot.meta.to_json.ok_or_else(|| NetError::NotSerializable {
                place: PlaceId(index as u32),
                name: slot.meta.name.clone(),
            })?;
            encoders.push(to_json);
        }
        let mut marking = Marking::new();
        for (index, (slot, to_json)) in self.places.iter().zip(encoders).enumerate() {
            let tokens = slot
                .tokens
                .iter()
                .map(|(token, value)| {
                    to_json(&**value).map_err(|source| NetError::CannotSerialize {
                        place: PlaceId(index as u32),
                        name: slot.meta.name.clone(),
                        token: *token,
                        source,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            marking.insert(slot.meta.name.clone(), tokens);
        }
        Ok(marking)
    }

    /// Replaces the whole marking with `marking`: every place emptied, then
    /// filled with the marking's tokens for it, oldest first (DESIGN §9).
    ///
    /// The inverse of [`export_marking`](Net::export_marking). A place the
    /// marking does not mention loads empty. Every token gets a **fresh
    /// id**: ids are not part of a marking, and this net's next ids are the
    /// ones it hands out. (A variant that keeps ids could follow.) Switches,
    /// parameters, the in-flight set and the counters other than
    /// [`state_counter`](Net::state_counter) are left as they are.
    ///
    /// A token of a [nested](crate::NetBuilder::place_nested) place is
    /// decoded with this net's [`templates`](Net::templates) in
    /// [scope](crate::Templates::scope), so its inner nets are rebuilt from
    /// the templates that built them; what they logged while being rebuilt
    /// is setup, and dropped, as for a seed.
    ///
    /// # Events
    ///
    /// An import is a replacement, not a sequence of changes: it emits one
    /// [`Event::Resynced`] and no [`Removed`](Event::Removed) or
    /// [`Added`](Event::Added), and `state_counter` moves by one. A view
    /// that sees it re-reads the marking.
    ///
    /// ```
    /// use petritype::{Event, Marking, NetBuilder};
    /// use petritype::serde_json::{self, json};
    ///
    /// let mut b = NetBuilder::new();
    /// let seats = b.place::<String>("Seats");
    /// b.serializable(seats);
    /// b.seed(seats, ["Ada".to_owned()]);
    /// let mut net = b.build()?;
    ///
    /// let marking: Marking = serde_json::from_value(json!({"Seats": ["Bram", "Cato"]}))?;
    /// net.import_marking(&marking)?;
    /// assert_eq!(net.count(seats.id()), 2);
    /// assert_eq!(net.drain_events().collect::<Vec<_>>(), [Event::Resynced]);
    /// assert_eq!(net.state_counter(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Checked in this order, and all before anything changes, so a
    /// rejected import leaves the net untouched and silent:
    /// [`CommandError::NoSuchPlaceName`] for a place the marking names and
    /// this net does not have; [`CommandError::NotSerializable`] for the
    /// first place of this net, in id order, that was not marked
    /// serializable, whether or not the marking mentions it (it could not be
    /// filled, and emptying it would lose what it holds); and
    /// [`CommandError::InvalidToken`] for the first token that does not
    /// decode as its place's type.
    pub fn import_marking(&mut self, marking: &Marking) -> Result<(), CommandError> {
        for (name, _) in marking.iter() {
            if self.place_named(name).is_none() {
                return Err(CommandError::NoSuchPlaceName {
                    name: name.to_owned(),
                });
            }
        }
        let mut decoders = Vec::with_capacity(self.places.len());
        for (index, slot) in self.places.iter().enumerate() {
            let from_json = slot
                .meta
                .from_json
                .ok_or_else(|| CommandError::NotSerializable {
                    place: PlaceId(index as u32),
                    name: slot.meta.name.clone(),
                })?;
            decoders.push(from_json);
        }
        let decoded: Vec<Vec<ErasedToken>> = self.templates.scope(|| {
            self.places
                .iter()
                .zip(&decoders)
                .enumerate()
                .map(|(index, (slot, from_json))| {
                    marking
                        .get(&slot.meta.name)
                        .unwrap_or_default()
                        .iter()
                        .map(|value| {
                            from_json(value.clone()).map_err(|source| CommandError::InvalidToken {
                                place: PlaceId(index as u32),
                                name: slot.meta.name.clone(),
                                expected: slot.meta.type_name,
                                source,
                            })
                        })
                        .collect()
                })
                .collect::<Result<_, _>>()
        })?;

        // Everything decoded: only now does anything change. The tokens
        // going out are dropped without a word, nested ones included: the
        // six lifting rules leave nothing in an inner log between calls, so
        // there is nothing of theirs to lose.
        for (index, tokens) in decoded.into_iter().enumerate() {
            self.places[index].tokens.clear();
            let place = PlaceId(index as u32);
            let nested = self.places[index].meta.nested;
            for mut token in tokens {
                // What an incoming token's inner nets logged while they were
                // being rebuilt is setup, as for a seed (DESIGN §6.6).
                if let Some(nested) = nested {
                    forget_inner_events(nested, &mut token);
                }
                self.insert_erased(place, token, "<imported>")
                    .expect("a token decoded by its own place's decoder has the place's type");
            }
        }
        self.emit(Event::Resynced);
        Ok(())
    }
}
