//! A net's marking as JSON, out and back in: [`Marking`],
//! [`Net::export_marking`] and [`Net::import_marking`] (DESIGN §9, `serde`
//! feature).

use std::collections::BTreeMap;

use crate::closed::IMPORT;
use crate::error::{CommandError, NetError};
use crate::event::Event;
use crate::ids::{PlaceId, TransitionId};
use crate::nested::forget_inner_events;
use crate::net::Net;
use crate::place::ErasedToken;
use crate::template::Loaded;

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
/// A [subnet place](crate::NetBuilder::subnet_place)'s entry is a list of
/// one: its subnet in the subnet's saved form (`Net`'s `Serialize`,
/// `{"template", "marking", "params", "enabled"}`), whose `marking` is this
/// form again. A port's tokens are written once, in its subnet's marking,
/// so the net around the subnet has no entry of its own for a port:
///
/// ```json
/// {"Door": ["Ada"],
///  "Level 3": [{"template": "Level",
///               "marking": {"Arena": [], "Exit Hall": [], "Lobby": ["Bram"]},
///               "params": {}, "enabled": {"Walk": true}}]}
/// ```
///
/// It is plain data (`Clone`, `PartialEq`, `Serialize`, `Deserialize`,
/// transparent over the map), so two markings compare by place name, token
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
    /// A [subnet place](crate::NetBuilder::subnet_place) is written as a
    /// list of one, its subnet in its saved form (see [`Marking`]), so the
    /// marking reaches into subnets too, and a port's tokens are written
    /// there, once. A subnet built without a template has no saved form, and
    /// nor then has the net around it.
    ///
    /// # Errors
    ///
    /// [`NetError::NotSerializable`] naming the first place, in id order,
    /// that was not marked, and [`NetError::CannotSaveSubnet`] for the first
    /// subnet place whose subnet no template built (every place is checked
    /// for both before any token is encoded); [`NetError::CannotSerialize`]
    /// for a token whose own `Serialize` refused, such as one holding a net
    /// that no template built; and `CannotSaveSubnet` again for a subnet
    /// whose own save refused, carrying why.
    pub fn export_marking(&self) -> Result<Marking, NetError> {
        let mut encoders = Vec::with_capacity(self.places.len());
        for (index, slot) in self.places.iter().enumerate() {
            // A port's tokens are written once, in its subnet's marking, and
            // a subnet place's entry is its subnet's saved form.
            if slot.meta.window.is_some() {
                encoders.push(None);
                continue;
            }
            if let Some(subnet) = &slot.subnet {
                if subnet.net.template().is_none() {
                    return Err(NetError::CannotSaveSubnet {
                        place: PlaceId(index as u32),
                        name: slot.meta.name.clone(),
                        source: Box::new(NetError::NoTemplate),
                    });
                }
                encoders.push(None);
                continue;
            }
            let to_json = slot.meta.to_json.ok_or_else(|| NetError::NotSerializable {
                place: PlaceId(index as u32),
                name: slot.meta.name.clone(),
            })?;
            encoders.push(Some(to_json));
        }
        let mut marking = Marking::new();
        for (index, (slot, to_json)) in self.places.iter().zip(encoders).enumerate() {
            if let Some(subnet) = &slot.subnet {
                let saved =
                    subnet
                        .net
                        .saved_value()
                        .map_err(|source| NetError::CannotSaveSubnet {
                            place: PlaceId(index as u32),
                            name: slot.meta.name.clone(),
                            source,
                        })?;
                marking.insert(slot.meta.name.clone(), vec![saved]);
                continue;
            }
            let Some(to_json) = to_json else {
                continue;
            };
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
    /// A [subnet place](crate::NetBuilder::subnet_place)'s entry, a list of
    /// one subnet in its saved form, goes into the subnet this net already
    /// holds, which keeps its id: the saved subnet must have been saved from
    /// the template that built this one, and its marking is imported into
    /// it by these same rules, its saved parameters set and its saved
    /// switches thrown. A subnet place the marking does not mention has its
    /// subnet's marking loaded empty, its parameters and switches as they
    /// were. A port's tokens come in with its subnet's marking.
    ///
    /// # Events
    ///
    /// An import is a replacement, not a sequence of changes: it emits one
    /// [`Event::Resynced`] and no [`Removed`](Event::Removed) or
    /// [`Added`](Event::Added), and `state_counter` moves by one. A view
    /// that sees it re-reads the marking. Each subnet logs its own
    /// `Resynced`, for its marking, parameters and switches, which comes
    /// out here first, lifted as an [`Event::Subnet`].
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
    /// rejected import leaves the net untouched and silent, subnets
    /// included: [`CommandError::NoSuchPlaceName`] for a place the marking
    /// names and this net does not have; [`CommandError::NotSerializable`]
    /// for the first place of this net, in id order, that was not marked
    /// serializable, and [`CommandError::CannotLoadSubnet`] for the first
    /// subnet place whose subnet no template built, whether or not the
    /// marking mentions either (it could not be filled, and emptying it
    /// would lose what it holds); then, place by place,
    /// [`CommandError::InvalidToken`] for the first token that does not
    /// decode as its place's type and `CannotLoadSubnet` for a subnet
    /// place's entry that does not load into its subnet, saying why.
    /// Before all of them, [`NetError::HeldByBody`] when a body of the net
    /// around this one holds it and it marks a port (since 2026-11-23): an
    /// import is a whole-state act, the program's, and goes round the rule
    /// of [closed nets](Net::is_closed), which it is not for a body.
    pub fn import_marking(&mut self, marking: &Marking) -> Result<(), CommandError> {
        self.admit_knob(IMPORT)?;
        let decoded = self.decode_marking(marking)?;
        self.load_marking(decoded);
        Ok(())
    }

    // ----- internals ------------------------------------------------------

    /// The read half of [`import_marking`](Net::import_marking): every
    /// check, and every token decoded, this net's subnets' included, with
    /// nothing changed.
    pub(crate) fn decode_marking(&self, marking: &Marking) -> Result<Decoded, CommandError> {
        for (name, _) in marking.iter() {
            if self.place_named(name).is_none() {
                return Err(CommandError::NoSuchPlaceName {
                    name: name.to_owned(),
                });
            }
        }
        let mut decoders = Vec::with_capacity(self.places.len());
        for (index, slot) in self.places.iter().enumerate() {
            // A port's tokens come in with its subnet's marking, and a
            // subnet place's entry goes into its subnet.
            if slot.meta.window.is_some() {
                decoders.push(None);
                continue;
            }
            if let Some(subnet) = &slot.subnet {
                if subnet.net.template().is_none() {
                    return Err(CommandError::CannotLoadSubnet {
                        place: PlaceId(index as u32),
                        name: slot.meta.name.clone(),
                        source: Box::new(NetError::NoTemplate),
                    });
                }
                decoders.push(None);
                continue;
            }
            let from_json = slot
                .meta
                .from_json
                .ok_or_else(|| CommandError::NotSerializable {
                    place: PlaceId(index as u32),
                    name: slot.meta.name.clone(),
                })?;
            decoders.push(Some(from_json));
        }
        self.templates.scope(|| {
            let mut tokens = Vec::with_capacity(self.places.len());
            let mut subnets = Vec::new();
            for (index, (slot, from_json)) in self.places.iter().zip(&decoders).enumerate() {
                let place = PlaceId(index as u32);
                if let Some(subnet) = &slot.subnet {
                    // Inside this net's scope, so that the subnet's tokens
                    // find a template here as well as in its own registry.
                    let decoded = decode_subnet(&subnet.net, marking.get(&slot.meta.name))
                        .map_err(|source| CommandError::CannotLoadSubnet {
                            place,
                            name: slot.meta.name.clone(),
                            source,
                        })?;
                    subnets.push((index, decoded));
                    tokens.push(Vec::new());
                    continue;
                }
                let Some(from_json) = from_json else {
                    tokens.push(Vec::new());
                    continue;
                };
                tokens.push(
                    marking
                        .get(&slot.meta.name)
                        .unwrap_or_default()
                        .iter()
                        .map(|value| {
                            from_json(value.clone()).map_err(|source| CommandError::InvalidToken {
                                place,
                                name: slot.meta.name.clone(),
                                expected: slot.meta.type_name,
                                source,
                            })
                        })
                        .collect::<Result<_, _>>()?,
                );
            }
            Ok(Decoded { tokens, subnets })
        })
    }

    /// The write half of [`import_marking`](Net::import_marking): what
    /// [`decode_marking`](Net::decode_marking) made, put in, subnets first,
    /// each lifting its own [`Event::Resynced`], and then this net's.
    pub(crate) fn load_marking(&mut self, decoded: Decoded) {
        // Everything decoded: only now does anything change. The tokens
        // going out are dropped without a word, nested ones included: the
        // six lifting rules leave nothing in an inner log between calls, so
        // there is nothing of theirs to lose. Every place is refilled, so
        // the key index is rebuilt from nothing as the tokens go in (DESIGN
        // §8.3); a port's tokens are its subnet's to index.
        self.index.clear();
        for (index, tokens) in decoded.tokens.into_iter().enumerate() {
            let meta = &self.places[index].meta;
            if meta.window.is_some() || meta.subnet {
                continue;
            }
            let nested = meta.nested;
            self.places[index].tokens.clear();
            let place = PlaceId(index as u32);
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
        for (index, decoded) in decoded.subnets {
            let place = PlaceId(index as u32);
            let subnet = &mut self.places[index]
                .subnet
                .as_deref_mut()
                .expect("a subnet place holds its subnet")
                .net;
            subnet.load_marking(decoded.marking);
            // Set where they are, as the marking is: the subnet's
            // `Resynced` tells them with it.
            for (param, value) in decoded.params {
                subnet.params[param].value = value;
            }
            for (transition, enabled) in decoded.enabled {
                subnet.transitions[transition.0 as usize].enabled = enabled;
            }
            let nested_id = subnet.id();
            let lifted: Vec<Event> = subnet
                .drain_events()
                .map(|event| Event::Subnet {
                    place,
                    nested_id,
                    event: Box::new(event),
                })
                .collect();
            self.emit_all(lifted);
        }
        self.emit(Event::Resynced);
    }
}

/// A marking decoded for one net, ready to go in: what
/// [`Net::decode_marking`] makes and [`Net::load_marking`] puts in.
pub(crate) struct Decoded {
    /// Every place's incoming tokens, in id order: empty for a port's place
    /// and for a subnet place.
    tokens: Vec<Vec<ErasedToken>>,
    /// What goes into each subnet, by the index of its subnet place.
    subnets: Vec<(usize, DecodedSubnet)>,
}

/// A subnet's saved form decoded for the subnet already there.
pub(crate) struct DecodedSubnet {
    /// Its marking.
    marking: Decoded,
    /// Its saved parameters, by the index of each one's slot, decoded.
    params: Vec<(usize, ErasedToken)>,
    /// Its saved switches.
    enabled: Vec<(TransitionId, bool)>,
}

/// A subnet place's entry in a marking (`None` if the marking does not
/// mention the place), decoded for the subnet this net holds there, with
/// nothing changed: a list of one subnet in its saved form, saved from the
/// template that built `subnet`. Not mentioned, it is the empty marking,
/// with no parameters or switches to set.
fn decode_subnet(
    subnet: &Net,
    entry: Option<&[serde_json::Value]>,
) -> Result<DecodedSubnet, Box<dyn std::error::Error + Send + Sync>> {
    let Some(entry) = entry else {
        return Ok(DecodedSubnet {
            marking: subnet.decode_marking(&Marking::new())?,
            params: Vec::new(),
            enabled: Vec::new(),
        });
    };
    let [saved] = entry else {
        return Err(format!(
            "a subnet place's entry is a list of one subnet in its saved form, and this one has {}",
            entry.len()
        )
        .into());
    };
    let loaded: Loaded = serde_json::from_value(saved.clone())?;
    let built = subnet
        .template()
        .expect("every subnet was checked for a template before any was decoded");
    if loaded.template != built {
        return Err(format!(
            "it was saved from template {:?}, and this net's subnet is built by {built:?}",
            loaded.template
        )
        .into());
    }
    let marking = subnet.decode_marking(&loaded.marking)?;
    let mut params = Vec::with_capacity(loaded.params.len());
    for (param, value) in loaded.params {
        // As a JSON `set_param` decodes one: by wire name, with that
        // parameter's own decoder.
        let index = subnet
            .params
            .iter()
            .position(|slot| slot.name.as_deref() == Some(param.as_str()))
            .ok_or_else(|| CommandError::NoSuchParamName {
                name: param.clone(),
            })?;
        let slot = &subnet.params[index];
        let from_json = slot
            .from_json
            .expect("a named parameter records both codecs");
        let value = from_json(value).map_err(|source| CommandError::InvalidParam {
            param,
            expected: slot.type_name,
            source,
        })?;
        params.push((index, value));
    }
    let mut enabled = Vec::with_capacity(loaded.enabled.len());
    for (name, on) in loaded.enabled {
        let transition = subnet
            .transition_named(&name)
            .ok_or(CommandError::NoSuchTransitionName { name })?;
        enabled.push((transition, on));
    }
    Ok(DecodedSubnet {
        marking,
        params,
        enabled,
    })
}
