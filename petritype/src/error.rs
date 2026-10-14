//! Error types. All are `#[non_exhaustive]`: match with a `_` arm.

use std::any::Any;

use std::fmt::Write as _;

use crate::ids::{NetId, PlaceId, TokenId, TransitionId};

/// A net that cannot be built, returned by
/// [`NetBuilder::build`](crate::NetBuilder::build).
///
/// Build errors are wiring mistakes that the type system cannot catch. They
/// are found once, before the net runs.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// Two places were given the same name. Names identify places to a reader
    /// and to [`Net::place_named`](crate::Net::place_named), so they must be
    /// unique.
    #[error("duplicate place name: {name:?}")]
    DuplicatePlaceName {
        /// The name used more than once.
        name: String,
    },
    /// Two transitions were given the same name, which
    /// [`Net::transition_named`](crate::Net::transition_named) and every
    /// diagram would then be unable to tell apart.
    #[error("duplicate transition name: {name:?}")]
    DuplicateTransitionName {
        /// The name used more than once.
        name: String,
    },
    /// A [`SnapshotEdge`](crate::SnapshotEdge) or a
    /// [`SurveyEdge`](crate::SurveyEdge) reads a place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone), or with
    /// [`place_nested`](crate::NetBuilder::place_nested), which is such a
    /// place.
    ///
    /// A snapshot is a clone of the front token, a survey a clone of every
    /// token, and such a place recorded no cloner. The token type may well
    /// be `Clone` — the place is what gave the ability up — so this cannot
    /// be a compile error. Use a [`TakeEdge`](crate::TakeEdge) (a
    /// [`DrainEdge`](crate::DrainEdge) for a survey), or declare the place
    /// with [`place`](crate::NetBuilder::place).
    #[error("transition {transition:?} snapshots place {place:?}, which cannot clone its tokens")]
    SnapshotOfNoClonePlace {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads.
        place: String,
    },
    /// One transition reads one place through two input edges, whatever their
    /// kinds.
    ///
    /// A transition's input places must be distinct, because both halves of
    /// firing assume it: "[fireable](crate::Net::fireable) iff every input
    /// place is non-empty" would have to count how many tokens each edge
    /// wants otherwise, and the `&mut` borrows a
    /// [`MutateEdge`](crate::MutateEdge) hands the body are disjoint only
    /// because no two edges name the same place. Take a batch with one
    /// [`TakeAllEdge`](crate::TakeAllEdge), or split the work between two
    /// transitions.
    #[error("transition {transition:?} reads place {place:?} through more than one input edge")]
    DuplicateInputPlace {
        /// The transition whose edges they are.
        transition: String,
        /// The place read more than once.
        place: String,
    },
    /// A transition whose body [routes](crate::RoutedOutput) its outputs by
    /// type may emit a payload of a type none of its output places holds.
    ///
    /// Either an output place is missing from the declaration, or the body
    /// returns a payload the net has nowhere to put. A positional body,
    /// where the slot rather than the type picks the place, has no such
    /// question to answer.
    #[error("transition {transition:?} routes {type_name} to no output place")]
    UnroutableOutput {
        /// The transition whose body it is.
        transition: String,
        /// The payload type with nowhere to go.
        type_name: &'static str,
    },
    /// Two of a transition's output places hold the same type, so that type
    /// no longer picks one of them.
    ///
    /// Routing by type is only unambiguous while the types are distinct.
    /// Return a tuple instead, where order picks the place, or give one of
    /// the two places a newtype of its own so that the types differ again.
    #[error("transition {transition:?} has more than one output place of type {type_name}")]
    AmbiguousOutput {
        /// The transition whose output places they are.
        transition: String,
        /// The payload type with more than one destination.
        type_name: &'static str,
    },
    /// A transition declares an output place that no payload type of its
    /// routed body can reach.
    ///
    /// The declaration is what a diagram draws, so an edge no token can
    /// travel is a lie about the net. Drop the output place, or return a
    /// value that can reach it.
    #[error("transition {transition:?} can route nothing to output place {place:?}")]
    UnreachableOutput {
        /// The transition whose output place it is.
        transition: String,
        /// The place nothing can reach.
        place: String,
    },
    /// A transition asked to
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure)
    /// consumes from a place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone).
    ///
    /// Restoring works on clones taken before the body runs, because the body
    /// owns the tokens themselves and may have consumed or wrecked them by
    /// the time it fails; a place with no cloner has nothing to take. Only
    /// the consuming edges are affected — [`TakeEdge`](crate::TakeEdge),
    /// [`TakeAllEdge`](crate::TakeAllEdge),
    /// [`TakeWhereEdge`](crate::TakeWhereEdge) and
    /// [`DrainEdge`](crate::DrainEdge), however empty its place is when the
    /// net is built — since a [`MutateEdge`](crate::MutateEdge) takes nothing
    /// out of its place and so has nothing to put back, and a
    /// [`SnapshotEdge`](crate::SnapshotEdge) or a
    /// [`SurveyEdge`](crate::SurveyEdge) of such a place is already refused
    /// by [`SnapshotOfNoClonePlace`](BuildError::SnapshotOfNoClonePlace).
    /// Drop the flag, or declare the place with
    /// [`place`](crate::NetBuilder::place).
    #[error(
        "transition {transition:?} restores its inputs on failure but consumes place {place:?}, \
         which cannot clone its tokens"
    )]
    RestoreOfNoClonePlace {
        /// The transition whose flag it is.
        transition: String,
        /// The place it consumes.
        place: String,
    },
    /// A keyed input edge — a [`TakeWhereEdge`](crate::TakeWhereEdge) or a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) — reads a place that
    /// declares no [keys](crate::NetBuilder::keys).
    ///
    /// A keyed edge chooses its token by the keys the place derives, and a
    /// place without keys derives none, so whatever the filter says the
    /// transition could never fire. A dead transition is cheaper to refuse
    /// at build than to discover by watching a net that never moves (DESIGN
    /// §8.3). Declare keys on the place, or read it through a plain edge.
    /// Reported for the first such edge, in transition order.
    #[error(
        "transition {transition:?} reads place {place:?} through a keyed edge, but the place \
         declares no keys"
    )]
    KeyedEdgeOnUnkeyedPlace {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads, which declares no keys.
        place: String,
    },
    /// A place was marked [`passive`](crate::NetBuilder::passive) that was
    /// not declared with
    /// [`place_nested`](crate::NetBuilder::place_nested).
    ///
    /// Passive is a choice about inner nets — that
    /// [`step_deep`](crate::Net::step_deep) leaves them to an outer body —
    /// and an ordinary place has none, so the mark would say nothing. Declare
    /// the place with `place_nested`, or drop the mark. Reported for the
    /// first such place marked.
    #[error("place {place:?} is marked passive but was not declared with place_nested")]
    NotNested {
        /// The place that was marked.
        place: String,
    },
    /// One [`keys`](crate::NetBuilder::keys) declaration names the same key
    /// twice.
    ///
    /// A token has one value per key, and a filter asks for a key by name,
    /// so two keys of one name would make a clause ambiguous. Rename one, or
    /// drop it. Reported for the first such declaration, with the first name
    /// repeated in it.
    #[error("place {place:?} declares the key {name:?} more than once")]
    DuplicateKeyName {
        /// The place the keys were declared on.
        place: String,
        /// The name given more than once.
        name: String,
    },
    /// [`keys`](crate::NetBuilder::keys) was called twice for one place.
    ///
    /// A place's keys are one declaration — the names and the one extractor
    /// that gives their values in that order — so a second would either
    /// replace the first or have to be merged with it, and neither is what a
    /// second call plainly says. Declare every key in one call. Reported for
    /// the first such place; the first declaration is the one kept until
    /// then.
    #[error("place {place:?} declares its keys more than once")]
    KeysAlreadyDeclared {
        /// The place whose keys were declared again.
        place: String,
    },
    /// A transition declared a [`Param<T>`](crate::Param) and no value of
    /// that type was registered with
    /// [`NetBuilder::param`](crate::NetBuilder::param).
    ///
    /// A parameter is not a place: there is no empty state for one to be in,
    /// and a body is handed a clone of the value whenever it fires. So the
    /// value has to exist before the net does, and a net that cannot supply
    /// one is refused rather than left to fail at its first firing (DESIGN
    /// §10). The reverse — a registered parameter no transition declares — is
    /// fine: it is a value the net carries, for a guard or a reader.
    ///
    /// Reported for the first transition in declaration order that wants
    /// one, and within it for the first such input in the order the inputs
    /// were written.
    #[error(
        "transition {transition:?} declares a parameter of type {type_name}, which has no value"
    )]
    MissingParam {
        /// The transition that declared it.
        transition: String,
        /// The parameter type with no value,
        /// `std::any::type_name::<T>()`.
        type_name: &'static str,
    },
    /// Two parameter types were marked
    /// [`serializable_param`](crate::NetBuilder::serializable_param) under
    /// the same wire name.
    ///
    /// The name is how a `JsonCommand::SetParam` picks a parameter out, as a
    /// place's name is how it picks a place, so two types answering to one
    /// name would make a wire message ambiguous.
    #[cfg(feature = "serde")]
    #[error("duplicate parameter name: {name:?}")]
    DuplicateParamName {
        /// The name used more than once.
        name: String,
    },
    /// A transition consumes nothing, in a net built with
    /// [`assert_no_source_transitions`](crate::NetBuilder::assert_no_source_transitions).
    ///
    /// Such a *source* transition takes no token out of any place when it
    /// fires — read edges do not count, since a
    /// [`SnapshotEdge`](crate::SnapshotEdge), a
    /// [`MutateEdge`](crate::MutateEdge), a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) and a
    /// [`SurveyEdge`](crate::SurveyEdge) leave their places as they found
    /// them, and a [`Param<T>`](crate::Param) is not a place at all; nor
    /// does a [`DrainEdge`](crate::DrainEdge), which takes every token there
    /// is, possibly none, and so does not hold the transition back on an
    /// empty place — so having fired changes nothing about whether it can
    /// fire again, and the net never quiesces. Give it a
    /// [`TakeEdge`](crate::TakeEdge), a
    /// [`TakeAllEdge`](crate::TakeAllEdge) or a
    /// [`TakeWhereEdge`](crate::TakeWhereEdge), or drop the assertion:
    /// generators and transitions fed from outside the net are deliberate
    /// sources, and are bounded while the net runs instead.
    #[error("transition {transition:?} consumes nothing, so it could fire forever")]
    SourceTransition {
        /// The offending transition — the first one, in declaration order.
        transition: String,
    },
    /// The net's token flow has a cycle, in a net built with
    /// [`assert_acyclic`](crate::NetBuilder::assert_acyclic).
    ///
    /// Tokens can go round it for ever, so the net has no finish line the
    /// marking is working towards. Break the loop, or drop the assertion: a
    /// net that legitimately runs for ever is bounded while it runs instead.
    #[error("the net's token flow has a cycle: {}", path.join(" -> "))]
    Cycle {
        /// The nodes of the cycle by name, from the node the walk closed on
        /// round to itself: the first and the last name are the same one,
        /// and places and transitions alternate in between.
        path: Vec<String>,
    },
}

/// Misuse of the net's API at run time.
///
/// These are recoverable in the sense that the net is left untouched: nothing
/// is added, removed or mutated when one of these is returned — with the one
/// exception of [`FiringFailed`](NetError::FiringFailed), where the net is
/// consistent but the firing's inputs are gone (unless the transition asked
/// to [restore](crate::TransitionBuilder::restore_tokens_on_failure) them).
///
/// # Names and ids
///
/// An error escapes the scope the net lives in: a `?` in `main` prints the
/// message and exits the process, and by then an id alone is useless to the
/// person reading the log. These messages are read by humans far more often
/// than they are matched on by programs — ids are the programs' side, and
/// [`Event`](crate::Event)s already serve it while the net is alive. So a
/// variant about a thing that *exists* carries both its id and its `name`:
/// the name goes in the `Display` message, and the id stays as the field a
/// program matches on. A not-found variant carries the id alone, because
/// there is no name to carry.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NetError {
    /// No place with this id exists in this net. Usually a handle from a
    /// different net.
    ///
    /// The id alone: a place that does not exist has no name to report, and
    /// the id is the only thing the caller handed over.
    #[error("no such place: {place:?}")]
    NoSuchPlace {
        /// The offending id.
        place: PlaceId,
    },
    /// No transition with this id exists in this net. Usually an id from a
    /// different net, since ids are never reused within one.
    ///
    /// The id alone: a transition that does not exist has no name to report.
    #[error("no such transition: {transition:?}")]
    NoSuchTransition {
        /// The offending id.
        transition: TransitionId,
    },
    /// [`Net::fire`](crate::Net::fire) was told to fire a transition that
    /// cannot fire: it is switched off, one of its input places is empty, or
    /// its guard says no. [`Net::fireable`](crate::Net::fireable) lists the
    /// ones that can.
    #[error("transition {name:?} ({transition:?}) is not fireable")]
    NotFireable {
        /// The transition that was asked to fire.
        transition: TransitionId,
        /// The transition's name, for the message; the id is the field to
        /// match on.
        name: String,
    },
    /// The place exists but holds no token with this id. The token may have
    /// been consumed, removed, or never have belonged to this place.
    #[error("place {name:?} ({place:?}) holds no token {token:?}")]
    NoSuchToken {
        /// The place that was searched.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on. The place exists — only the token is missing — so there is a
        /// name to give.
        name: String,
        /// The token that was not found.
        token: TokenId,
    },
    /// A value of the wrong type was offered to a place. Only reachable
    /// through a handle from another net or, later, through an erased
    /// command: the typed API makes this a compile error.
    #[error("place {name:?} ({place:?}) holds {expected}, not {found}")]
    TypeMismatch {
        /// The place that was written to.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
        /// The type the place holds.
        expected: &'static str,
        /// The type that was offered.
        found: &'static str,
    },
    /// [`Net::distribute`](crate::Net::distribute) was handed a
    /// [`Completed`](crate::Completed) that another net launched.
    ///
    /// Checked before anything else, because a
    /// [`TransitionId`] means nothing outside its own net: every net numbers
    /// its transitions from zero, so the id would otherwise name whichever
    /// transition happens to sit at that index here. The net is untouched —
    /// and so is the net that launched the firing, whose transition stays in
    /// flight, since the `Completed` was consumed by the call.
    ///
    /// The ids alone, unlike the other runtime errors: the transition belongs
    /// to the *other* net, whose names this one cannot reach.
    #[error("transition {transition:?} was launched by net {from:?}, not this one")]
    ForeignFiring {
        /// The transition the firing belongs to, in the net that launched it.
        transition: TransitionId,
        /// That net.
        from: NetId,
    },
    /// [`Net::distribute`](crate::Net::distribute) was handed a
    /// [`Completed`](crate::Completed) for a transition whose body is already
    /// in its slot.
    ///
    /// A transition is in flight from
    /// [`launch`](crate::Net::launch) until the firing it produced is
    /// distributed ([`Net::in_flight`](crate::Net::in_flight)), and a
    /// `Completed` is consumed by the call that distributes it, so the safe
    /// API gives no way to distribute one twice. This is the guard that keeps
    /// it that way: nothing is written, and the outputs of the rejected
    /// firing are dropped with it.
    #[error("transition {name:?} ({transition:?}) is not in flight")]
    NotInFlight {
        /// The transition that was not airborne.
        transition: TransitionId,
        /// The transition's name, for the message; the id is the field to
        /// match on.
        name: String,
    },
    /// [`Net::set_param`](crate::Net::set_param), or an erased
    /// [`Command::SetParam`](crate::Command::SetParam), named a parameter
    /// type this net has no value for.
    ///
    /// Parameters are fixed when the net is built: every type some
    /// transition declares has a value ([`BuildError::MissingParam`]), and
    /// nothing adds a parameter afterwards, so this means the type was never
    /// registered — usually a value meant for another net, or a newtype
    /// mixed up with the type inside it.
    ///
    /// On the erased route the type cannot be named: a `dyn Any` knows its
    /// [`TypeId`](std::any::TypeId), which is what the lookup compares, but a
    /// `TypeId` cannot be turned back into a name. `type_name` is
    /// `"<erased>"` there, exactly as [`TypeMismatch`](NetError::TypeMismatch)
    /// blames `"<erased>"` for a token that arrived the same way.
    ///
    /// No name is added: a parameter is addressed by its type, and
    /// `type_name` already is the human-readable half.
    #[error("no parameter of type {type_name} in this net")]
    NoSuchParam {
        /// The parameter type that has no value, or `"<erased>"` when the
        /// value came in erased.
        type_name: &'static str,
    },
    /// [`Net::snapshot`](crate::Net::snapshot) was asked for a net holding a
    /// place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone).
    ///
    /// A snapshot is a copy of the marking, and such a place recorded no
    /// cloner, so its tokens cannot be copied — a `File` or a socket is a
    /// resource, not a value. Nothing is copied at all: the snapshot is
    /// refused before any place is walked, so a caller never holds half a
    /// state.
    #[error("place {name:?} ({place:?}) cannot clone its tokens")]
    NoClonePlace {
        /// The place whose tokens cannot be copied — the first such place, in
        /// id order.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on. The name is also what a caller has to go and change in the
        /// builder.
        name: String,
    },
    /// [`Net::export_marking`](crate::Net::export_marking), a save or a
    /// [`fork`](crate::Net::fork) met a place that was not marked
    /// [`serializable`](crate::NetBuilder::serializable).
    ///
    /// A marking is every place's tokens as JSON, and such a place has no
    /// encoder, so half a marking is all there could be. Every place is
    /// checked before anything is written; this names the first offender in
    /// id order. [`CommandError::NotSerializable`] is the same refusal on the
    /// way in.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}) is not marked serializable")]
    NotSerializable {
        /// The place with no encoder.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
    },
    /// A token's own `Serialize` refused, on the way to a marking: most
    /// often a token holding a [`Net`](crate::Net) that no template built,
    /// which cannot be saved because it could not be restored.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}): token {token:?} cannot be serialised: {source}")]
    CannotSerialize {
        /// The place holding the token.
        place: PlaceId,
        /// The place's name, for the message.
        name: String,
        /// The token that would not serialise.
        token: TokenId,
        /// What its `Serialize` said.
        #[source]
        source: serde_json::Error,
    },
    /// [`Net::fork`](crate::Net::fork) was asked of a net that no template
    /// built: there is no factory to build the copy with. Build the net
    /// through [`Templates::build`](crate::Templates::build) to give it one.
    #[cfg(feature = "serde")]
    #[error("the net was not built from a template, so it cannot be rebuilt")]
    NoTemplate,
    /// A [`fork`](crate::Net::fork) or a save of a net with a transition in
    /// flight: the tokens that firing took are in no place, so a copy would
    /// silently lose them. Distribute the firing first; a driver never
    /// returns with one airborne.
    #[cfg(feature = "serde")]
    #[error("transition {name:?} ({transition:?}) is in flight")]
    InFlight {
        /// The first transition in flight, in declaration order.
        transition: TransitionId,
        /// Its name, for the message.
        name: String,
    },
    /// A nested token whose type keeps the default
    /// [`Nested::fork`](crate::Nested::fork), which refuses: the type has
    /// not said how to copy itself. Implement `fork` (the plain fields
    /// cloned, each inner net through [`Net::fork`](crate::Net::fork)).
    #[cfg(feature = "serde")]
    #[error("{type_name} does not implement Nested::fork, so it cannot be copied")]
    NotForkable {
        /// The token type, `std::any::type_name::<T>()`.
        type_name: &'static str,
    },
    /// [`Net::snapshot`](crate::Net::snapshot) could not fork one token of a
    /// nested place; `source` says why. The first such token, in place
    /// order and then token order; nothing is returned, as with
    /// [`NoClonePlace`](NetError::NoClonePlace).
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}): token {token:?} cannot be forked: {source}")]
    CannotFork {
        /// The nested place holding the token.
        place: PlaceId,
        /// The place's name, for the message.
        name: String,
        /// The token whose fork failed.
        token: TokenId,
        /// Why: [`NotForkable`](NetError::NotForkable),
        /// [`NoTemplate`](NetError::NoTemplate) or whatever else the token's
        /// fork returned.
        #[source]
        source: Box<NetError>,
    },
    /// [`Net::fork`](crate::Net::fork) built a fresh net from the template
    /// and could not make it a copy: the factory failed this time, or the
    /// marking it exported did not load back into what the factory built.
    #[cfg(feature = "serde")]
    #[error("the net could not be rebuilt from template {template:?}: {source}")]
    RebuildFailed {
        /// The template the net was built from.
        template: String,
        /// The factory's error, or the [`CommandError`] of the import.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A firing this call started failed. [`Net::fire`](crate::Net::fire)
    /// reports a body's failure this way; [`Net::step`](crate::Net::step)
    /// returns the [`FiringFailed`] itself.
    #[error(transparent)]
    FiringFailed(#[from] FiringFailed),
}

/// An erased [`Command`](crate::Command) the net would not carry out,
/// returned by [`Net::apply`](crate::Net::apply).
///
/// A rejected command changes nothing and emits nothing (DESIGN §9), so this
/// error is always a complete report: the net is exactly as it was before the
/// call. Most of it is a [`NetError`] — the erased route is where a wrong
/// type or a stale id can really arrive, since the typed API makes both a
/// compile error or a panic. The rest is the JSON route's own vocabulary,
/// where places and transitions travel by name and tokens as
/// `serde_json::Value`, so there are two more ways to be wrong before the net
/// is ever asked.
///
/// # Names and ids
///
/// An error escapes the scope the net lives in: a `?` in `main` prints the
/// message and exits the process, and by then an id alone is useless to the
/// person reading the log. These messages are read by humans far more often
/// than they are matched on by programs — ids are the programs' side, and
/// [`Event`](crate::Event)s already serve it while the net is alive. So a
/// variant about a thing that *exists* carries both its id and its `name`:
/// the name goes in the `Display` message, and the id stays as the field a
/// program matches on. A not-found variant carries the id alone, because
/// there is no name to carry.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandError {
    /// The net rejected the command: an unknown place, transition or token,
    /// or a token of the wrong type for its place.
    #[error(transparent)]
    Net(#[from] NetError),
    /// A JSON command names a place that was not marked
    /// [`serializable`](crate::NetBuilder::serializable), so the net has no
    /// way to decode its tokens — or, for
    /// [`Command::to_json`](crate::Command::to_json), to encode them.
    ///
    /// Serialisation is opt-in per place; mark the place in the builder.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}) is not marked serializable")]
    NotSerializable {
        /// The place whose tokens cannot cross a wire.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
    },
    /// A token in a JSON command did not parse as the place's type.
    ///
    /// The JSON is well-formed but the wrong shape — `"seven"` for a place of
    /// `u32`. Nothing is applied: the command is rejected at the first token
    /// that fails, before the net is touched.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}): token does not parse as {expected}: {source}")]
    InvalidToken {
        /// The place the token was meant for.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
        /// The type that place holds, `std::any::type_name::<T>()`.
        expected: &'static str,
        /// What `serde_json` made of it.
        #[source]
        source: serde_json::Error,
    },
    /// A JSON command named a place that does not exist.
    ///
    /// The JSON form addresses places by name rather than by
    /// [`PlaceId`], because a name is what an author or a saved file knows
    /// and it survives a change in declaration order.
    ///
    /// The name alone, and no id: the place does not exist, so there is no id
    /// to report, and the name is what the sender said.
    #[cfg(feature = "serde")]
    #[error("no place named {name:?}")]
    NoSuchPlaceName {
        /// The name that matched no place.
        name: String,
    },
    /// A JSON command named a transition that does not exist.
    ///
    /// The name alone, as for
    /// [`NoSuchPlaceName`](CommandError::NoSuchPlaceName): nothing exists to
    /// have an id.
    #[cfg(feature = "serde")]
    #[error("no transition named {name:?}")]
    NoSuchTransitionName {
        /// The name that matched no transition.
        name: String,
    },
    /// A JSON command named a parameter that no parameter type of this net
    /// is marked with.
    ///
    /// The wire name comes from
    /// [`serializable_param`](crate::NetBuilder::serializable_param), and a
    /// parameter with no name cannot be addressed from JSON at all — so this
    /// is both "no such parameter" and "that parameter was never marked" on
    /// the way in.
    ///
    /// The name alone: a parameter has no id at all, and this one has no
    /// value either.
    #[cfg(feature = "serde")]
    #[error("no parameter named {name:?}")]
    NoSuchParamName {
        /// The name that matched no parameter.
        name: String,
    },
    /// [`Command::to_json`](crate::Command::to_json) was given a
    /// [`SetParam`](crate::Command::SetParam) whose type has no wire name.
    ///
    /// Serialisation is opt-in per parameter type, as it is per place: mark
    /// the type with
    /// [`serializable_param`](crate::NetBuilder::serializable_param), which
    /// records the codecs and the name together. A value whose type this net
    /// has no parameter of at all is reported the same way, as `"<erased>"`:
    /// an erased value knows its [`TypeId`](std::any::TypeId) but cannot be
    /// asked for its name.
    #[cfg(feature = "serde")]
    #[error("parameter {type_name} is not marked serializable")]
    ParamNotSerializable {
        /// The parameter type that cannot cross a wire, or `"<erased>"`.
        type_name: &'static str,
    },
    /// The value of a JSON `set_param` did not parse as the parameter's type.
    ///
    /// The parameter twin of [`InvalidToken`](CommandError::InvalidToken),
    /// and separate from it because a parameter has no place to blame: it is
    /// named on the wire and typed in the net, and neither is a
    /// [`PlaceId`]. Nothing is applied — decoding happens before the net is
    /// touched.
    #[cfg(feature = "serde")]
    #[error("parameter {param:?}: value does not parse as {expected}: {source}")]
    InvalidParam {
        /// The wire name the value arrived under.
        param: String,
        /// The type that parameter holds, `std::any::type_name::<T>()`.
        expected: &'static str,
        /// What `serde_json` made of it.
        #[source]
        source: serde_json::Error,
    },
}

/// A [`Templates::build`](crate::Templates::build) that did not produce a
/// net.
#[cfg(feature = "serde")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TemplateError {
    /// No template is registered under this name.
    #[error("no template named {name:?}")]
    NoSuchTemplate {
        /// The name asked for.
        name: String,
    },
    /// The template's factory returned an error.
    #[error("template {name:?} failed to build its net: {source}")]
    Factory {
        /// The template's name.
        name: String,
        /// What the factory returned.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// A firing whose body returned `Err` or panicked.
///
/// By the time this is returned the net is consistent again (DESIGN §7): the
/// body is back in its slot, the transition is not in flight, and nothing was
/// distributed, so the next [`step`](crate::Net::step) works. What the firing
/// consumed is *gone* unless [`restored`](FiringFailed::restored) says
/// otherwise; [`Event::Failed`](crate::Event::Failed) records which tokens
/// those were.
///
/// A panicking body arrives here like any other failure, with
/// [`BodyPanicked`] as the [`error`](FiringFailed::error) — downcast it to
/// tell the two apart.
///
/// The transition's name travels with its id. A failure is the error most
/// likely to leave the net behind — a `?` in `main` prints it and exits — and
/// by then `TransitionId(2)` says nothing to the person reading the log. The
/// id is still the field to match on; the name is for the message.
///
/// A firing inside an inner net, made by
/// [`step_deep`](crate::Net::step_deep), fails the same way and says where
/// in [`path`](FiringFailed::path); its transition and name are the inner
/// net's.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("transition {name:?} ({transition:?}){} failed: {error}", inside(.path))]
pub struct FiringFailed {
    /// The transition whose body failed.
    pub transition: TransitionId,
    /// The transition's name, for the message; the id is the field to match
    /// on.
    pub name: String,
    /// What the body returned, erased at the edge of the typed API so that
    /// the engine speaks one error type whatever bodies return. It is also
    /// this error's [`source`](std::error::Error::source).
    #[source]
    pub error: Box<dyn std::error::Error + Send + Sync>,
    /// Whether the consumed tokens were put back in their places.
    ///
    /// True exactly for a transition declared with
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
    /// whose consumed tokens are back at the front of their places with the
    /// ids they had. False otherwise: a failed firing loses its inputs by
    /// default, because the body owned them and may have changed or dropped
    /// them before it failed.
    pub restored: bool,
    /// Where the transition is, when it is not in the net that was stepped:
    /// one `(place, token, net name)` per level, outer to inner, each naming
    /// the nested place, the token in it and the inner net of that token, as
    /// [`Event::Nested`](crate::Event::Nested) does. Empty for a firing of
    /// the net's own.
    ///
    /// Filled in by [`step_deep`](crate::Net::step_deep) as the failure comes
    /// up through each level, and printed in the message as
    /// `inside PlaceId(0) #5 "cabin"`.
    pub path: Vec<(PlaceId, TokenId, String)>,
}

/// The part of a [`FiringFailed`] message that says which inner net the
/// transition is in, or nothing for a transition of the net itself.
fn inside(path: &[(PlaceId, TokenId, String)]) -> String {
    let mut text = String::new();
    for (level, (place, token, net)) in path.iter().enumerate() {
        text.push_str(if level == 0 { " inside " } else { " / " });
        let _ = write!(text, "{place:?} #{token} {net:?}");
    }
    text
}

/// A body that panicked, as the failure its firing reports.
///
/// Panics are caught (DESIGN §7): [`Firing::run`](crate::Firing::run) wraps
/// the call to a detachable body, and [`launch`](crate::Net::launch) wraps
/// the inline call it makes for a transition with a
/// [`MutateEdge`](crate::MutateEdge) input, both in
/// [`catch_unwind`](std::panic::catch_unwind). Either way the panic becomes
/// this error, the firing fails like one whose body returned `Err`, and the
/// body goes home to its slot — so one panicking token does not strand a
/// transition in flight for good.
///
/// What comes home is whatever the panic left behind: a body is `FnMut` and
/// may keep state of its own, and a panic can stop it half way through
/// changing it. The net's own state is not in question — nothing was
/// distributed — and a token a [`MutateEdge`](crate::MutateEdge) body had
/// already changed stays changed, since the change was made where the token
/// lies.
///
/// Nothing can be caught under `panic = "abort"`, where the process dies at
/// the panic, as the profile asked.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("panicked: {message}")]
pub struct BodyPanicked {
    /// The panic payload as text: the message a `panic!` carried, or
    /// `"panic payload of unknown type"` for a payload that is neither a
    /// `&str` nor a `String` — which only
    /// [`panic_any`](std::panic::panic_any) can produce.
    pub message: String,
}

impl BodyPanicked {
    /// Reads the message out of a caught payload.
    ///
    /// The two types `panic!` itself produces, in that order: a literal
    /// message is a `&'static str` and a formatted one a `String`. Anything
    /// else came from [`panic_any`](std::panic::panic_any) and has no text to
    /// read, so the type is not named either — `Any` cannot say what it is.
    pub(crate) fn from_payload(payload: Box<dyn Any + Send>) -> BodyPanicked {
        let message = if let Some(message) = payload.downcast_ref::<&'static str>() {
            (*message).to_owned()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            String::from("panic payload of unknown type")
        };
        BodyPanicked { message }
    }
}
