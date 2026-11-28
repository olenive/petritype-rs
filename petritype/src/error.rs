//! Error types. All are `#[non_exhaustive]`: match with a `_` arm.

use std::any::Any;

use std::fmt::Write as _;

use crate::closed;
use crate::firing::Completed;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};

/// A net that cannot be built, returned by
/// [`NetBuilder::build`](crate::NetBuilder::build).
///
/// Build errors are wiring mistakes that the type system cannot catch. They
/// are found once, before the net runs.
///
/// `Clone`, `PartialEq` and `Eq` (since 0.4.0), so a test compares one with
/// `assert_eq!` and a caller keeps a copy of one it reports.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
    /// be `Clone` (the place is what gave the ability up), so this cannot
    /// be a compile error. Use a [`ReadEdge`](crate::ReadEdge) (a
    /// [`ReadAllEdge`](crate::ReadAllEdge) for a survey), which lends the
    /// body the token where it lies and clones nothing, a
    /// [`TakeEdge`](crate::TakeEdge) (a [`DrainEdge`](crate::DrainEdge) for a
    /// survey), or declare the place with [`place`](crate::NetBuilder::place).
    #[error("transition {transition:?} snapshots place {place:?}, which cannot clone its tokens")]
    SnapshotOfNoClonePlace {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads.
        place: String,
    },
    /// One transition reads one place through two or more input edges, and
    /// one of them is of a kind that cannot share its place (since 0.4.0;
    /// until then any two edges on one place were refused, as
    /// `DuplicateInputPlace`).
    ///
    /// Several edges of one transition may read one place when each takes a
    /// token of its own: [`TakeEdge`](crate::TakeEdge) and
    /// [`TakeWhereEdge`](crate::TakeWhereEdge), in any mix.
    /// Every other kind is refused beside another edge on its place: a
    /// [`MutateEdge`](crate::MutateEdge) or a read edge lends the body a
    /// token where it lies, a [`SnapshotEdge`](crate::SnapshotEdge) or a
    /// [`SurveyEdge`](crate::SurveyEdge) copies one or all, and a
    /// [`TakeAllEdge`](crate::TakeAllEdge) or a
    /// [`DrainEdge`](crate::DrainEdge) takes every token, each a token that
    /// another edge of the same firing may take. Take two tokens through
    /// two take edges, a batch through one take-all edge, or split the
    /// work between two transitions. Reported for the first place in edge
    /// order that a second edge reads, with the first kind there that
    /// cannot share.
    #[error(
        "transition {transition:?} reads place {place:?} through a {} and another input edge; \
         only TakeEdge and TakeWhereEdge share a place, each taking a token of its own",
        edge_struct(*.kind)
    )]
    EdgeCannotSharePlace {
        /// The transition whose edges they are.
        transition: String,
        /// The place read through more than one edge.
        place: String,
        /// The kind of the first edge on the place that cannot share it.
        kind: crate::edge::EdgeKind,
    },
    /// Two output edges of one transition go to one place (since 0.4.0;
    /// until then the net built, and a positional body filled both slots
    /// into the place).
    ///
    /// An output edge is one slot of what the body returns, and a slot
    /// already carries any number of tokens: return a `Vec<T>` (or an
    /// `Option<T>`) in one slot to put several tokens, or none, into one
    /// place. Reported for the first transition, and the first place in
    /// output order, that two edges name.
    #[error(
        "transition {transition:?} has two output edges to place {place:?}; a transition has at \
         most one output edge to a place. To put two tokens there, return them as a Vec in one \
         slot"
    )]
    DuplicateOutputPlace {
        /// The transition whose output edges they are.
        transition: String,
        /// The place two of them go to.
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
    /// the consuming edges are affected ([`TakeEdge`](crate::TakeEdge),
    /// [`TakeAllEdge`](crate::TakeAllEdge),
    /// [`TakeWhereEdge`](crate::TakeWhereEdge) and
    /// [`DrainEdge`](crate::DrainEdge), however empty its place is when the
    /// net is built), since a [`MutateEdge`](crate::MutateEdge) or a read
    /// edge takes nothing out of its place and so has nothing to put back,
    /// and a [`SnapshotEdge`](crate::SnapshotEdge) or a
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
    /// A keyed input edge (a [`TakeWhereEdge`](crate::TakeWhereEdge), a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) or a
    /// [`ReadWhereEdge`](crate::ReadWhereEdge)) reads a place that declares
    /// no [keys](crate::NetBuilder::keys).
    ///
    /// A keyed edge chooses its token by the keys the place derives, and a
    /// place without keys derives none, so whatever the filter says the
    /// transition could never fire. A dead transition is cheaper to refuse
    /// at build than to discover by watching a net that never moves.
    /// Declare keys on the place, or read it through a plain edge.
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
    /// A keyed edge chooses its token by a key of another input's token
    /// ([`Clause::IsInput`](crate::Clause::IsInput)), and that input, the
    /// edge's *source*, is a place the transition does not read.
    ///
    /// The clause compares with the token the same firing gets from its
    /// source, so the source must be one of the transition's input places.
    /// Add an edge on it, or name the place the transition reads. Reported,
    /// as the next four are, for the first such edge in transition order and
    /// then edge order (since 2026-11-23).
    #[error(
        "transition {transition:?} chooses its token from place {place:?} by place {input:?}, \
         which it does not read"
    )]
    BoundSourceNotAnInput {
        /// The transition whose edge it is.
        transition: String,
        /// The place the bound edge reads.
        place: String,
        /// The place the clause names.
        input: String,
    },
    /// A keyed edge chooses its token by a key of another input's token, and
    /// that input's edge gives every token rather than one: a
    /// [`TakeAllEdge`](crate::TakeAllEdge), a
    /// [`DrainEdge`](crate::DrainEdge), a
    /// [`SurveyEdge`](crate::SurveyEdge) or a
    /// [`ReadAllEdge`](crate::ReadAllEdge).
    ///
    /// A key is read from one token, so the source must give one: a take, a
    /// snapshot, a mutation or a read, keyed or not.
    #[error(
        "transition {transition:?} chooses its token from place {place:?} by place {input:?}, \
         whose edge gives every token rather than one"
    )]
    BoundSourceGivesMany {
        /// The transition whose edge it is.
        transition: String,
        /// The place the bound edge reads.
        place: String,
        /// The source place.
        input: String,
    },
    /// A keyed edge chooses its token by a key of another input's token, and
    /// that input's own token is chosen by an input too: a chain, or a
    /// clause naming the edge's own place, the shortest cycle.
    ///
    /// A source chooses its token by its own filter alone, so that each
    /// firing has one pair to find per source. Bind both edges to the same
    /// source instead, or choose in two transitions.
    #[error(
        "transition {transition:?} chooses its token from place {place:?} by place {input:?}, \
         whose own token is chosen by an input"
    )]
    BoundSourceIsBound {
        /// The transition whose edge it is.
        transition: String,
        /// The place the bound edge reads.
        place: String,
        /// The source place, which is bound itself.
        input: String,
    },
    /// One keyed edge's filter names two different sources.
    ///
    /// A bound edge has one source, whose token it is paired with. Two
    /// clauses may name two keys of the same source.
    #[error(
        "transition {transition:?} chooses its token from place {place:?} by two inputs, \
         {first:?} and {second:?}"
    )]
    BoundToTwoSources {
        /// The transition whose edge it is.
        transition: String,
        /// The place the bound edge reads.
        place: String,
        /// The source the first input clause names.
        first: String,
        /// Another source a later clause names.
        second: String,
    },
    /// A clause that chooses a token by another input's names a key that
    /// one of the two places does not declare: the bound edge's place its
    /// `key`, or the source place the key it reads.
    ///
    /// Both values are keys, so that the clause shows on the wire and in a
    /// drawing as declared, and so that the pair is found through the index.
    /// Declare the key, or name one the place has. Constant and parameter
    /// clauses are not checked this way: a key no token has accepts nothing.
    #[error(
        "transition {transition:?} chooses its token from place {place:?} by key {key:?}, which \
         place {owner:?} does not declare"
    )]
    BoundKeyNotDeclared {
        /// The transition whose edge it is.
        transition: String,
        /// The place the bound edge reads.
        place: String,
        /// The key named.
        key: String,
        /// The place that should declare it: the bound edge's or the
        /// source's.
        owner: String,
    },
    /// A keyed edge chooses its token by another input's token, and its
    /// transition reads the bound edge's place, or the source's, through a
    /// second edge (since 0.4.0, when two take edges came to share a
    /// place).
    ///
    /// A source is one edge, so that a clause reads one token, and the
    /// pairing chooses its tokens on their own: a second edge on the place
    /// could choose the token the pair was found with. Python petritype
    /// refuses the same. Pair through a place no other edge of the
    /// transition reads.
    #[error(
        "transition {transition:?} reads place {place:?} through more than one input edge, and \
         a token there chooses or is chosen by another input"
    )]
    BoundPlaceShared {
        /// The transition whose edges they are.
        transition: String,
        /// The place read through more than one edge: the bound edge's or
        /// the source's.
        place: String,
    },
    /// An edge names ports it reaches ([`Reaching`](crate::Reaching)) on a
    /// place whose tokens carry no nets, so there is nothing for it to
    /// reach (since 2026-11-23). Declare the place with
    /// [`place_nested`](crate::NetBuilder::place_nested), or drop the
    /// names.
    #[error(
        "transition {transition:?} names ports it reaches through place {place:?}, whose \
         tokens carry no nets"
    )]
    ReachIntoNoNets {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads.
        place: String,
    },
    /// An edge names a port it reaches by the empty string, which no place
    /// is called.
    #[error(
        "transition {transition:?} names a port it reaches through place {place:?} by \
             the empty string"
    )]
    EmptyReachName {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads.
        place: String,
    },
    /// A name was given as the empty string, which no name may be (since
    /// 2026-11-28): a name has at least one character, any character, a
    /// space included. Reported, before every other check but a port's, for
    /// the first such name in this order: the places and subnet places, the
    /// keys of each place, the transitions, the groups, the controls, the
    /// parameters' wire names and the templates registered for the net's
    /// tokens. A port's name is its subnet's place's, already refused there.
    ///
    /// Two names keep `""`, each with a meaning: the name
    /// [`Nested::nets`](crate::Nested::nets) gives a bare [`Net`](crate::Net)
    /// carried as a token, which is its own unnamed inner net, and a
    /// reach's port names, which [`EmptyReachName`](BuildError::EmptyReachName)
    /// refuses as before.
    #[error("{what} is named by the empty string: a name has at least one character")]
    EmptyName {
        /// What was named.
        what: Named,
    },
    /// A control no widget can show. Never returned by
    /// [`build`](crate::NetBuilder::build), since a
    /// [`Control`](crate::control::Control) is checked as it is made: it is
    /// here so that a function that builds a net, and returns a
    /// `BuildError`, can make its controls with `?`.
    #[error(transparent)]
    Control(#[from] ControlError),
    /// A [`keys`](crate::NetBuilder::keys) declaration names no key at all,
    /// `keys::<T, 0>` (since 2026-11-28): no filter could ever ask its
    /// place for a token by one. Name at least one, or declare none.
    #[error("place {place:?} declares its keys with no key in them")]
    NoKeys {
        /// The place the keys were declared on.
        place: String,
    },
    /// A place that holds no nets was given a
    /// [`stepping`](crate::NetBuilder::stepping) rule.
    ///
    /// The rule says how the nets in a place take their turns beside the
    /// net around them ([`Stepping`](crate::Stepping)), and an ordinary
    /// place has none, so the rule would say nothing. Nor does a port of a
    /// subnet place ([`port_of`](crate::NetBuilder::port_of)): the nets its
    /// tokens carry are the subnet's to step, under the rule the subnet's
    /// definition gives the port. Declare the place with
    /// [`place_nested`](crate::NetBuilder::place_nested), give the rule to a
    /// [`subnet_place`](crate::NetBuilder::subnet_place), or drop it.
    /// Reported for the first such place given one.
    #[error("place {place:?} is given a stepping rule but holds no nets")]
    NotNested {
        /// The place that was given the rule.
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
    /// A place's keys are one declaration (the names and the one extractor
    /// that gives their values in that order), so a second would either
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
    /// one is refused rather than left to fail at its first firing. The
    /// reverse, a registered parameter no transition declares, is fine: it
    /// is a value the net carries, for a guard or a reader.
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
    /// fires, so having fired changes nothing about whether it can fire
    /// again, and the net never quiesces. Non-consuming edges do not count, since a
    /// [`SnapshotEdge`](crate::SnapshotEdge), a
    /// [`MutateEdge`](crate::MutateEdge), a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge), a
    /// [`SurveyEdge`](crate::SurveyEdge) and the read edges leave their
    /// places as they found them, and a [`Param<T>`](crate::Param) is not a place at all; nor
    /// does a [`DrainEdge`](crate::DrainEdge), which takes every token there
    /// is, possibly none, and so does not hold the transition back on an
    /// empty place. Give it a
    /// [`TakeEdge`](crate::TakeEdge), a
    /// [`TakeAllEdge`](crate::TakeAllEdge) or a
    /// [`TakeWhereEdge`](crate::TakeWhereEdge), or drop the assertion:
    /// generators and transitions fed from outside the net are deliberate
    /// sources, and are bounded while the net runs instead.
    #[error("transition {transition:?} consumes nothing, so it could fire forever")]
    SourceTransition {
        /// The offending transition: the first one, in declaration order.
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
        /// and places and transitions alternate in between, but where the
        /// cycle goes through a subnet place from one of its ports to
        /// another (since 2026-11-28): the two ports are named by their
        /// subnet place and their own names, `S/In` and `S/Out`, with
        /// `(inside subnet place "S")` between them.
        path: Vec<String>,
    },
    /// Two controls were given the same name, which
    /// [`Net::press`](crate::Net::press) would then be unable to tell
    /// apart.
    #[error("duplicate control name: {name:?}")]
    DuplicateControlName {
        /// The name used more than once.
        name: String,
    },
    /// Two [groups](crate::NetBuilder::group) were given the same name. A
    /// group is known to a reader, and to
    /// [`PlaceInfo::group`](crate::PlaceInfo::group), by its name alone, so
    /// names are unique among a net's groups.
    #[error("duplicate group name: {name:?}")]
    DuplicateGroupName {
        /// The name used more than once.
        name: String,
    },
    /// A place was put in a second group with
    /// [`in_group`](crate::NetBuilder::in_group). A node is in one group or
    /// in none, so that a drawing has one frame to draw it in. Reported for
    /// the first such node, places and transitions alike, in the order
    /// they were put in groups.
    #[error("place {place:?} is put in group {second:?}, but it is in group {first:?} already")]
    PlaceInTwoGroups {
        /// The place.
        place: String,
        /// The group it was put in first, which it stays in until then.
        first: String,
        /// The group it was put in after that.
        second: String,
    },
    /// A transition was put in a second group with
    /// [`TransitionBuilder::group`](crate::TransitionBuilder::group), as
    /// [`PlaceInTwoGroups`](BuildError::PlaceInTwoGroups) for a place.
    #[error(
        "transition {transition:?} is put in group {second:?}, but it is in group {first:?} \
         already"
    )]
    TransitionInTwoGroups {
        /// The transition.
        transition: String,
        /// The group it was put in first, which it stays in until then.
        first: String,
        /// The group it was put in after that.
        second: String,
    },
    /// A port of one of the net's subnet places, the handle
    /// [`port_of`](crate::NetBuilder::port_of) gave out, was put in a group
    /// with [`in_group`](crate::NetBuilder::in_group). A port is drawn in
    /// its subnet place's box and goes where the box goes: put the subnet
    /// place in the group instead. Inside the subnet, the port is an
    /// ordinary place of the subnet's, and may be in one of its groups.
    #[error(
        "place {place:?} is a port of subnet place {subnet:?}, so it cannot be put in group \
         {group:?}: it is drawn in its subnet place's box"
    )]
    PortInGroup {
        /// The port.
        place: String,
        /// The subnet place it belongs to.
        subnet: String,
        /// The group it was put in.
        group: String,
    },
    /// [`port_of`](crate::NetBuilder::port_of) named a port the subnet does
    /// not have: no place of that name, or one its definition did not mark
    /// with [`port`](crate::NetBuilder::port). Only ports can be reached
    /// from outside. Reported, as every port problem is, before anything
    /// else the build checks.
    #[error("subnet place {subnet:?} has no port {port:?}")]
    NoSuchPort {
        /// The subnet place asked.
        subnet: String,
        /// The port name that matched no port.
        port: String,
    },
    /// [`port_of`](crate::NetBuilder::port_of) asked for a port as one type
    /// and the port holds another. The handle's type is what the outer
    /// net's edges are checked against, so it has to be the port's.
    #[error("port {port:?} of subnet place {subnet:?} holds {expected}, not {found}")]
    PortTypeMismatch {
        /// The subnet place asked.
        subnet: String,
        /// The port.
        port: String,
        /// The type the port holds.
        expected: &'static str,
        /// The type it was asked for as.
        found: &'static str,
    },
    /// [`port`](crate::NetBuilder::port) was given a port of one of this
    /// net's own subnet places. A port is a place of the net that marks it,
    /// an ordinary place or one whose tokens carry nets, so that no edge
    /// crosses two borders: a net around this one reaches a subnet's port
    /// through a port of this net and a transition of this net between the
    /// two.
    #[error("place {place:?} is a port of subnet place {subnet:?}, so it cannot be a port here")]
    PortIsSubnetPlace {
        /// The port that was marked.
        place: String,
        /// The subnet place it belongs to.
        subnet: String,
    },
    /// A port of one of the net's subnet places, the handle
    /// [`port_of`](crate::NetBuilder::port_of) gave out, was given an option
    /// that says what the place is: its [`keys`](crate::NetBuilder::keys),
    /// a [`style`](crate::NetBuilder::style) or
    /// [`styled`](crate::NetBuilder::styled), a
    /// [`place_style`](crate::NetBuilder::place_style), or `serializable`
    /// (with the `serde` feature) (since 0.4.0; until then each of those
    /// calls panicked). What a port is, beyond its tokens, is said by its
    /// subnet's definition, where the port is an ordinary place: give it the
    /// option there. Reported, as every port problem is, before anything
    /// else the build checks.
    #[error(
        "place {place:?} is a port of subnet place {subnet:?}, so `{method}` is not called on it \
         here: what a port is, beyond its tokens, is declared in its subnet's definition"
    )]
    PortOption {
        /// The method called: `"keys"`, `"style"`, `"styled"`,
        /// `"place_style"` or `"serializable"`.
        method: &'static str,
        /// The port.
        place: String,
        /// The subnet place it belongs to.
        subnet: String,
    },
    /// A transition has an edge on a subnet place, which holds no tokens of
    /// its own. Only the subnet's ports are reached from outside. The typed
    /// API already refuses this when it compiles, since a
    /// [`SubnetRef`](crate::SubnetRef) is no [`PlaceRef`](crate::PlaceRef);
    /// this is the check behind that.
    #[error("transition {transition:?} has an edge on subnet place {place:?}")]
    EdgeOnSubnetPlace {
        /// The transition whose edge it is.
        transition: String,
        /// The subnet place.
        place: String,
    },
}

/// What a [`BuildError::EmptyName`] says was named by the empty string.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Named {
    /// A place, by [`place`](crate::NetBuilder::place),
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone) or
    /// [`place_nested`](crate::NetBuilder::place_nested).
    Place,
    /// A [subnet place](crate::NetBuilder::subnet_place).
    SubnetPlace,
    /// A key that a place [declares](crate::NetBuilder::keys).
    Key {
        /// The place, by its name.
        place: String,
    },
    /// A transition.
    Transition,
    /// A [group](crate::NetBuilder::group).
    Group,
    /// A [control](crate::NetBuilder::control).
    Control,
    /// A parameter's wire name, given to `NetBuilder::serializable_param`
    /// (with the `serde` feature). Not `Param`: a variant of that name
    /// makes the compiler spell out `petritype::Param<T>` in its messages.
    Parameter {
        /// The parameter's type, `std::any::type_name::<T>()`.
        type_name: &'static str,
    },
    /// A template, registered for the net's tokens with
    /// `NetBuilder::template` or `NetBuilder::templates`, or built with
    /// `Templates::build` (with the `serde` feature).
    Template,
}

impl std::fmt::Display for Named {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Named::Place => f.write_str("a place"),
            Named::SubnetPlace => f.write_str("a subnet place"),
            Named::Key { place } => write!(f, "a key of place {place:?}"),
            Named::Transition => f.write_str("a transition"),
            Named::Group => f.write_str("a group"),
            Named::Control => f.write_str("a control"),
            Named::Parameter { type_name } => write!(f, "the parameter of type {type_name}"),
            Named::Template => f.write_str("a template"),
        }
    }
}

/// A [`Control`](crate::control::Control) no widget can show, refused as
/// it is made (since 2026-11-28): a slider whose numbers are not finite,
/// whose `start` is above its `stop` or whose `step` is not above zero; a
/// select with no option; an initial value a press of the kind could not
/// carry. See [Checked as it is made](crate::control::Control#checked-as-it-is-made).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("control {control:?}: {reason}")]
pub struct ControlError {
    /// The control's name.
    pub control: String,
    /// What no widget can show.
    pub reason: String,
}

/// A press that did not run, or whose handler refused, returned by
/// [`Net::press`](crate::Net::press).
///
/// The first two leave the net untouched: the control's handler never ran.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PressError {
    /// The net declares no control by this name.
    #[error("no control named {name:?}")]
    UnknownControl {
        /// The name pressed.
        name: String,
    },
    /// The value cannot be the control's: the wrong type for its kind, an
    /// option a select does not list, or a number outside a slider's
    /// bounds.
    #[error("control {control:?} cannot take that value: {reason}")]
    WrongKind {
        /// The control pressed.
        control: String,
        /// What was wrong with the value.
        reason: String,
    },
    /// The handler ran and returned `Err` with this message: it could not
    /// do what it was asked. What it did before that is in the log.
    #[error("{0}")]
    Refused(String),
}

/// Misuse of the net's API at run time.
///
/// These are recoverable in the sense that the net is left untouched: nothing
/// is added, removed or mutated when one of these is returned, with the one
/// exception of [`FiringFailed`](NetError::FiringFailed), where the net is
/// consistent but the firing's inputs are gone (unless the transition asked
/// to [restore](crate::TransitionBuilder::restore_tokens_on_failure) them).
///
/// # Names and ids
///
/// An error escapes the scope the net lives in: a `?` in `main` prints the
/// message and exits the process, and by then an id alone is useless to the
/// person reading the log. These messages are read by humans far more often
/// than they are matched on by programs. Ids are the programs' side, and
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
        /// on. The place exists (only the token is missing), so there is a
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
    /// transition happens to sit at that index here. The net is untouched,
    /// and so is the net that launched the firing, whose transition stays in
    /// flight until the `Completed` lands there: `distribute` hands it back
    /// in its [`DistributeError`] (since 0.4.0; until then the call consumed
    /// it).
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
    /// it that way: nothing is written, and the rejected firing comes back
    /// in the [`DistributeError`] (since 0.4.0; until then it was dropped,
    /// outputs and all).
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
    /// registered: usually a value meant for another net, or a newtype
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
    /// cloner, so its tokens cannot be copied: a `File` or a socket is a
    /// resource, not a value. Nothing is copied at all: the snapshot is
    /// refused before any place is walked, so a caller never holds half a
    /// state.
    #[error("place {name:?} ({place:?}) cannot clone its tokens")]
    NoClonePlace {
        /// The place whose tokens cannot be copied: the first such place, in
        /// id order.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on. The name is also what a caller has to go and change in the
        /// builder.
        name: String,
    },
    /// [`Net::set_stepping`](crate::Net::set_stepping) was asked to change
    /// the rule of a place that holds no nets, and so has none.
    /// [`BuildError::NotNested`] is the same refusal in the builder.
    #[error("place {name:?} ({place:?}) holds no nets, so it has no stepping rule")]
    NotNested {
        /// The place that was named.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
    },
    /// [`Net::subnet_scope`](crate::Net::subnet_scope) was given a place
    /// that holds no subnet.
    #[error("place {name:?} ({place:?}) holds no subnet")]
    NotSubnet {
        /// The place that was named.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
    },
    /// An [`export_marking`](crate::Net::export_marking), a save or a
    /// [`fork`](crate::Net::fork) of a net whose subnet place holds a subnet
    /// that cannot be saved, and so a net that cannot be either: the
    /// marking writes each subnet in its saved form, under its place.
    ///
    /// `source` says why, as it would for the subnet saved on its own. Most
    /// often the subnet was built without a template
    /// ([`NoTemplate`](NetError::NoTemplate)): such a subnet runs, but
    /// nothing knows how to build it again. A subnet of the subnet that
    /// cannot be saved is this error again in `source`, so the message
    /// names every level.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}) holds a subnet that cannot be saved: {source}")]
    CannotSaveSubnet {
        /// The subnet place, the first such one in id order.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
        /// Why the subnet has no saved form.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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
    /// A transition is in flight where none may be. A fork or a save of a
    /// net with one (`Net::fork` and `Serialize`, with the `serde`
    /// feature): the tokens that firing took are in no place, so a copy
    /// would silently lose them.
    /// A [`subnet_scope`](crate::Net::subnet_scope) while one that has an
    /// edge on the subnet's ports is airborne: the subnet is held still
    /// until it lands. An import (`Net::import_marking`, with the `serde`
    /// feature) while one of the net's own, or one of a subnet's at any
    /// depth, is airborne (since 2026-11-28): it would land into the marking
    /// imported. Distribute the firing first; a driver never returns with
    /// one airborne.
    #[error("transition {name:?} ({transition:?}){} is in flight", inside(.path))]
    InFlight {
        /// The first transition in flight, in declaration order.
        transition: TransitionId,
        /// Its name, for the message.
        name: String,
        /// Where the transition is, when it is not in the net asked: one
        /// [`PathStep::Subnet`] per level, outer to inner, as
        /// [`FiringFailed::path`] names them. Empty for one of the net's own
        /// (since 2026-11-28).
        path: Vec<PathStep>,
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
    /// A write from outside a net that the rule of closed nets refuses: see
    /// [`Net::is_closed`](crate::Net::is_closed).
    /// [`why`](ClosedRefusal) says which refusal it was, and the message
    /// says how to put it right.
    ///
    /// Every net is closed unless its definition declares it open: from
    /// outside it (a body of the net around it, the program and its drivers,
    /// a runner's `inject`), tokens are put, taken, removed, replaced or set
    /// only at its ports, and a net that marks no port takes none. A body
    /// writes only the ports its edge names in its
    /// [`reaching`](crate::Reaching), and no place of a closed net inside
    /// the net it holds. A closed net shields the nets inside it: from
    /// outside it, nothing inside it is written or loaded. The net is
    /// unchanged. A body that meets it fails its firing with it, whether or
    /// not the body passed it on.
    #[error("{}", closed_message(.net, .place, .act, .why))]
    ClosedNet {
        /// The net written to, by the template that built it, or by its id
        /// (`NetId(7)`) when no template did.
        net: String,
        /// The place written to, by its name in that net; `None` for an
        /// import (`Net::import_marking`), which writes every place.
        place: Option<String>,
        /// What the write was: `"put a token into"`, `"add tokens to"`,
        /// `"set the tokens of"`, `"remove a token from"`,
        /// `"replace a token in"` or `"import a marking into"`.
        act: &'static str,
        /// Which refusal it was.
        why: ClosedRefusal,
    },
    /// A knob or a firing by name asked of a net by a body of the net around
    /// it, which holds the net as a token (since 2026-11-23 on a net that
    /// marks a port, on every net since 2026-11-28).
    ///
    /// On every net, closed or declared open, the parameters, switches,
    /// tie-break, stepping rules and selector, firing its transitions by
    /// name, pressing its controls and importing a marking into it are the
    /// program's, its drivers' and the runner's. So are, since 2026-11-28,
    /// draining its events ([`Net::drain_events`](crate::Net::drain_events)),
    /// moving its tie-break cursor without a firing
    /// ([`Net::choose`](crate::Net::choose)), a staged step
    /// ([`Net::step_deep_staged`](crate::Net::step_deep_staged)) and
    /// driving it with
    /// [`OnFailure::Disable`](crate::drivers::OnFailure::Disable). A body is
    /// handed a net to read, to step, to drive and to write at its ports,
    /// and while it holds the net each of these is refused, the net
    /// unchanged, and its firing fails with this error when the body
    /// returns, whether or not the body passed it on.
    #[error("{}", held_message(.net, .act))]
    HeldByBody {
        /// The net, by the template that built it, or by its id
        /// (`NetId(7)`) when no template did.
        net: String,
        /// What was asked: `"set a parameter of"`, `"switch a transition
        /// of"`, `"fire a transition of"`, `"drain the events of"` and the
        /// like.
        act: &'static str,
    },
    /// A firing this call started failed. [`Net::fire`](crate::Net::fire)
    /// reports a body's failure this way; [`Net::step`](crate::Net::step)
    /// returns the [`FiringFailed`] itself.
    #[error(transparent)]
    FiringFailed(#[from] FiringFailed),
    /// A step could not choose what to fire (since 2026-11-28): a
    /// [`StepError`] converted with `?`, so that a body that steps a net it
    /// holds and returns `NetError` passes it on. See [`SelectionFailed`].
    #[error(transparent)]
    SelectionFailed(#[from] SelectionFailed),
}

/// An erased [`Command`](crate::Command) the net would not carry out,
/// returned by [`Net::apply`](crate::Net::apply).
///
/// A rejected command changes nothing and emits nothing, so this error is
/// always a complete report: the net is exactly as it was before the call.
/// Most of it is a [`NetError`]: the erased route is where a wrong
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
/// than they are matched on by programs. Ids are the programs' side, and
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
    /// way to decode its tokens or, for
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
    /// The JSON is well-formed but the wrong shape: `"seven"` for a place of
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
    /// parameter with no name cannot be addressed from JSON at all, so this
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
    /// A marking's entry for a subnet place did not load into its subnet
    /// ([`Net::import_marking`](crate::Net::import_marking), and so a
    /// restore): it is not a list of one subnet in its saved form, the
    /// subnet it holds was saved from another template than the one that
    /// built this net's subnet, this net's subnet was built without a
    /// template, or the saved subnet's marking, parameters or switches do
    /// not fit it.
    ///
    /// `source` says which, in the subnet's own words: a [`CommandError`]
    /// of the subnet's import, this error again for a subnet of the subnet,
    /// so that the message names every level.
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}): the saved subnet does not load: {source}")]
    CannotLoadSubnet {
        /// The subnet place.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
        /// Why the entry did not load.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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
    /// [`Command::to_json`](crate::Command::to_json) was given a command at
    /// a window, the place a net has for a port of one of its subnet places
    /// ([`port_of`](crate::NetBuilder::port_of)), since 2026-11-28.
    ///
    /// A JSON command names its place by name, and a port is named by its
    /// subnet place and its own name, as [`Net::port_ref`](crate::Net::port_ref)
    /// finds it: the port's name alone would name another place, or none,
    /// when the command is read back. Apply the erased
    /// [`Command`](crate::Command) itself, which carries the window's id.
    #[cfg(feature = "serde")]
    #[error(
        "place {place:?} is port {port:?} of subnet place {subnet:?}, which a JSON command \
         cannot name"
    )]
    PortNotNamed {
        /// The window's id in this net.
        place: PlaceId,
        /// The subnet place the port belongs to.
        subnet: String,
        /// The port's name in its subnet.
        port: String,
    },
    /// [`Command::to_json`](crate::Command::to_json) met a token whose own
    /// `Serialize` refused (since 2026-11-28; until then its error's text
    /// was written as the token).
    #[cfg(feature = "serde")]
    #[error("place {name:?} ({place:?}): a token cannot be serialised: {source}")]
    CannotEncodeToken {
        /// The place the command is for.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on.
        name: String,
        /// What its `Serialize` said.
        #[source]
        source: serde_json::Error,
    },
    /// [`Command::to_json`](crate::Command::to_json) met a parameter value
    /// whose own `Serialize` refused (since 2026-11-28; until then its
    /// error's text was written as the value).
    #[cfg(feature = "serde")]
    #[error("parameter {param:?}: the value cannot be serialised: {source}")]
    CannotEncodeParam {
        /// The parameter's wire name.
        param: String,
        /// What its `Serialize` said.
        #[source]
        source: serde_json::Error,
    },
    /// The value of a JSON `set_param` did not parse as the parameter's type.
    ///
    /// The parameter twin of [`InvalidToken`](CommandError::InvalidToken),
    /// and separate from it because a parameter has no place to blame: it is
    /// named on the wire and typed in the net, and neither is a
    /// [`PlaceId`]. Nothing is applied: decoding happens before the net is
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

/// Why a token of a [serializable](crate::NetBuilder::serializable) place
/// has no JSON form now, for [`Net::json`](crate::Net::json) and
/// [`Match::json`](crate::Match::json) (since 2026-11-28; until then the
/// reason's text stood in for the value, and a `NaN` or an infinity was
/// written as `null`).
///
/// Its own `Serialize` refused, as a token holding a net that no template
/// built does, or it holds a `NaN` or an infinity, which JSON has no number
/// for, as the Python library holds. The same token is refused by a save
/// ([`NetError::CannotSerialize`]) and by
/// [`Command::to_json`](crate::Command::to_json)
/// ([`CommandError::CannotEncodeToken`]). Its label, its `Debug`, is
/// unaffected.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("the token has no JSON form: {message}")]
pub struct NoJsonForm {
    /// Why: what its `Serialize` said, or which number JSON cannot write
    /// and where in the token it is.
    pub message: String,
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
    /// [`Templates::build`](crate::Templates::build) was asked for a
    /// template by the empty string (since 2026-11-28): a template's name
    /// is what a saved net names it by, and has at least one character.
    #[error("a template is named by the empty string: a name has at least one character")]
    EmptyName,
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
/// By the time this is returned the net is consistent again: the body is
/// back in its slot, the transition is not in flight, and nothing was
/// distributed, so the next [`step`](crate::Net::step) works. What the firing
/// consumed is *gone* unless [`restored`](FiringFailed::restored) says
/// otherwise; [`Event::Failed`](crate::Event::Failed) records which tokens
/// those were.
///
/// A panicking body arrives here like any other failure, with
/// [`BodyPanicked`] as the [`error`](FiringFailed::error). Downcast it to
/// tell the two apart.
///
/// The transition's name travels with its id. A failure is the error most
/// likely to leave the net behind (a `?` in `main` prints it and exits), and
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
    /// one [`PathStep`] per level, outer to inner, each naming the net one
    /// level down from the one before it: the inner net of a token
    /// ([`PathStep::Token`], as [`Event::Nested`](crate::Event::Nested)
    /// names it) or the subnet of a subnet place ([`PathStep::Subnet`], as
    /// [`Event::Subnet`](crate::Event::Subnet) does). Empty for a firing of
    /// the net's own.
    ///
    /// Filled in by [`step_deep`](crate::Net::step_deep) as the failure comes
    /// up through each level, and by the concurrent driver for a firing it
    /// launched inside, and printed in the message as
    /// `inside PlaceId(1) / PlaceId(0) #5 "cabin"`: a subnet place's level
    /// by its place, a token's by its place, its token and its net.
    pub path: Vec<PathStep>,
    /// The nested id of the net each step of [`path`](FiringFailed::path)
    /// leads to, in the same order, as
    /// [`InnerBody::nested_ids`](crate::InnerBody::nested_ids) gives them
    /// (since 0.4.0): what tells apart two nets of one name in one token,
    /// which the path names alike. Empty for a firing of the net's own.
    pub nested_ids: Vec<NetId>,
}

/// A choice of the next transition that could not be made (since
/// 2026-11-28): a fireable transition's priority is `NaN`, or a custom
/// selector named a transition it was not offered; [`why`](SelectionFailed::why)
/// says which. Until then a `NaN` priority counted as `0.0` and such an
/// answer fired nothing. The Python library raises `ValueError` in both
/// cases.
///
/// Nothing fired and nothing in the net changed, the
/// [round robin](crate::TieBreak::RoundRobin)'s cursor included, so the
/// net is as it was and a later step asks again. It is returned by
/// [`Net::select`](crate::Net::select), [`Net::choose`](crate::Net::choose)
/// and [`Net::priority_of`](crate::Net::priority_of), and inside a
/// [`StepError`] by every step. A choice made in an inner net by
/// [`step_deep`](crate::Net::step_deep) says where in
/// [`path`](SelectionFailed::path), as a [`FiringFailed`] does.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("{}", selection_message(.why, .path))]
pub struct SelectionFailed {
    /// Which choice could not be made.
    pub why: SelectionFault,
    /// Where the net that chose is, when it is not the net that was
    /// stepped: one [`PathStep`] per level, outer to inner, as
    /// [`FiringFailed::path`] names them. Empty for the net's own choice.
    pub path: Vec<PathStep>,
}

impl SelectionFailed {
    /// The fault, in the net that was asked.
    pub(crate) fn new(why: SelectionFault) -> SelectionFailed {
        SelectionFailed {
            why,
            path: Vec::new(),
        }
    }
}

/// Which choice a [`SelectionFailed`] could not make.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionFault {
    /// A fireable transition's priority closure returned `NaN`, which ranks
    /// neither above nor below any other, so no transition can be chosen by
    /// it. The default selector asks every fireable transition's priority,
    /// so this names the first `NaN` it met. An infinity is a number and
    /// ranks as itself.
    NanPriority {
        /// The transition whose priority is `NaN`.
        transition: TransitionId,
        /// Its name, for the message; the id is the field to match on.
        name: String,
    },
    /// The net's custom selector ([`Net::set_selector`](crate::Net::set_selector))
    /// answered with a transition that was not among the ones it was
    /// offered, the fireable ones: a transition that cannot fire now, or
    /// an id this net does not have.
    NotOffered {
        /// What the selector answered.
        chosen: TransitionId,
        /// Its name, when the net has such a transition.
        name: Option<String>,
        /// What it was offered, in declaration order, each with its name.
        offered: Vec<(TransitionId, String)>,
    },
}

/// The message of a [`SelectionFailed`].
fn selection_message(why: &SelectionFault, path: &[PathStep]) -> String {
    let inside = inside(path);
    match why {
        SelectionFault::NanPriority { transition, name } => format!(
            "the priority of transition {name:?} ({transition:?}){inside} is NaN, which ranks \
             neither above nor below any other, so no transition can be chosen by it. Give it a \
             number; infinity ranks as itself"
        ),
        SelectionFault::NotOffered {
            chosen,
            name,
            offered,
        } => {
            let chosen = match name {
                Some(name) => {
                    format!("transition {name:?} ({chosen:?}), which is not fireable now")
                }
                None => format!("{chosen:?}, which this net does not have"),
            };
            let offered: Vec<String> = offered
                .iter()
                .map(|(transition, name)| format!("{name:?} ({transition:?})"))
                .collect();
            format!(
                "the selector{inside} chose {chosen}; it was offered the fireable transitions \
                 [{}], and answers with one of them, or None to fire nothing",
                offered.join(", ")
            )
        }
    }
}

/// Why a step fired nothing of what it chose (since 2026-11-28): a firing
/// failed, or the choice of what to fire could not be made, or (since
/// 0.4.0) a body asked a step of a net it holds that is the program's.
/// Returned by
/// [`Net::step`](crate::Net::step), [`Net::step_deep`](crate::Net::step_deep),
/// [`Net::step_deep_staged`](crate::Net::step_deep_staged),
/// [`Net::step_deep_staged_with`](crate::Net::step_deep_staged_with) and
/// [`Net::step_inside`](crate::Net::step_inside), which returned
/// [`FiringFailed`] alone until then, and by
/// [`Net::choose`](crate::Net::choose), which returned
/// [`SelectionFailed`] until 0.4.0.
///
/// Code that read the failure changes little: [`firing`](StepError::firing)
/// and [`into_firing`](StepError::into_firing) give it back, a
/// [`FiringFailed`] converts into a `StepError` with `?`, and a `StepError`
/// converts into a [`NetError`] with `?`, so a body that steps a net it
/// holds and returns `NetError` is unchanged.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StepError {
    /// A firing the step made failed: its body returned `Err` or panicked,
    /// or what it returned could not be distributed. See [`FiringFailed`].
    #[error(transparent)]
    FiringFailed(#[from] FiringFailed),
    /// The step could not choose what to fire, and fired nothing. See
    /// [`SelectionFailed`].
    #[error(transparent)]
    SelectionFailed(#[from] SelectionFailed),
    /// A step a body may not ask of a net it holds (since 0.4.0; until then
    /// the step panicked with this message): a staged step
    /// ([`Net::step_deep_staged`](crate::Net::step_deep_staged),
    /// [`Net::step_deep_staged_with`](crate::Net::step_deep_staged_with)),
    /// one without the net's own turn
    /// ([`Net::step_inside`](crate::Net::step_inside)), or a choice that
    /// moves the tie-break's cursor with no firing
    /// ([`Net::choose`](crate::Net::choose)). Nothing fired, the
    /// net is unchanged, and the body's firing fails with
    /// [`NetError::HeldByBody`] when it returns, whether or not the body
    /// passed this on; `?` into a [`NetError`] gives that error. A body
    /// steps a net it holds with [`Net::step`](crate::Net::step) or
    /// [`Net::step_deep`](crate::Net::step_deep), which never return this.
    #[error("{}", held_message(.net, .act))]
    HeldByBody {
        /// The net, by the template that built it, or by its id
        /// (`NetId(7)`) when no template did.
        net: String,
        /// What was asked: `"stage a step of"`, `"step the nets
        /// inside"` or `"choose a transition of"`.
        act: &'static str,
    },
}

impl StepError {
    /// The failed firing, when a firing failed.
    pub fn firing(&self) -> Option<&FiringFailed> {
        match self {
            StepError::FiringFailed(failed) => Some(failed),
            StepError::SelectionFailed(_) | StepError::HeldByBody { .. } => None,
        }
    }

    /// The failed firing, when a firing failed, and the error back
    /// otherwise.
    ///
    /// # Errors
    ///
    /// The `StepError` itself, unchanged, when no firing failed.
    pub fn into_firing(self) -> Result<FiringFailed, StepError> {
        match self {
            StepError::FiringFailed(failed) => Ok(failed),
            other => Err(other),
        }
    }

    /// The selection that could not be made, when that was the trouble.
    pub fn selection(&self) -> Option<&SelectionFailed> {
        match self {
            StepError::SelectionFailed(failed) => Some(failed),
            StepError::FiringFailed(_) | StepError::HeldByBody { .. } => None,
        }
    }

    /// Where it happened, outer to inner: the
    /// [`path`](FiringFailed::path) of the failed firing or of the
    /// selection. Empty for the net that was stepped, which is always where
    /// a [`HeldByBody`](StepError::HeldByBody) happens.
    pub fn path(&self) -> &[PathStep] {
        match self {
            StepError::FiringFailed(failed) => &failed.path,
            StepError::SelectionFailed(failed) => &failed.path,
            StepError::HeldByBody { .. } => &[],
        }
    }

    /// The same error one level further out: `step` goes in front of its
    /// path, and for a failed firing `nested_id`, the id of the net it
    /// leads to, in front of its nested ids. A refusal is only ever the
    /// net's that was stepped, and has no path to grow.
    pub(crate) fn one_level_out(mut self, step: PathStep, nested_id: NetId) -> StepError {
        match &mut self {
            StepError::FiringFailed(failed) => {
                failed.path.insert(0, step);
                failed.nested_ids.insert(0, nested_id);
            }
            StepError::SelectionFailed(failed) => failed.path.insert(0, step),
            StepError::HeldByBody { .. } => {}
        }
        self
    }
}

impl FiringFailed {
    /// The same failure, of a firing in the net `way` leads down to: its
    /// path and nested ids are the way's, which the concurrent driver keeps
    /// for each firing it launches inside.
    pub(crate) fn along(mut self, way: &[crate::deep::Level]) -> FiringFailed {
        (self.path, self.nested_ids) = crate::deep::split(way);
        self
    }
}

impl SelectionFailed {
    /// The same choice, of the net `way` leads down to, as
    /// [`FiringFailed::along`].
    pub(crate) fn along(mut self, way: &[crate::deep::Level]) -> SelectionFailed {
        self.path = way.iter().map(|level| level.step.clone()).collect();
        self
    }
}

impl From<StepError> for NetError {
    /// A failed firing as [`NetError::FiringFailed`], a selection as
    /// [`NetError::SelectionFailed`], and a refusal as
    /// [`NetError::HeldByBody`], the error the body's firing fails with.
    fn from(error: StepError) -> NetError {
        match error {
            StepError::FiringFailed(failed) => NetError::FiringFailed(failed),
            StepError::SelectionFailed(failed) => NetError::SelectionFailed(failed),
            StepError::HeldByBody { net, act } => NetError::HeldByBody { net, act },
        }
    }
}

/// A [`Completed`] that [`Net::distribute`](crate::Net::distribute) did
/// not land, and why (since 0.4.0; until then `distribute` returned the
/// [`NetError`] alone and dropped the `Completed` with its body).
///
/// [`error`](DistributeError::error) says why: the reasons are
/// [`NetError`]'s. For the three that leave the firing as it was,
/// [`NetError::HeldByBody`], [`NetError::ForeignFiring`] and
/// [`NetError::NotInFlight`], the `Completed` comes back too,
/// [`completed`](DistributeError::completed), body and outputs and all, so
/// the caller can hand it to the net that launched it and that transition
/// does not stay in flight for good. For [`NetError::FiringFailed`] there
/// is nothing to give back: the firing was distributed as a failure, its
/// body is home and the net is consistent.
///
/// Its message is the error's, and so is its
/// [`source`](std::error::Error::source). `?` turns it into a
/// [`NetError`], dropping a `Completed` it carries, as `distribute` did
/// until 0.4.0, or into a `Box<dyn Error + Send + Sync>`, which keeps it.
///
/// ```
/// use petritype::{NetBuilder, NetError, TakeEdge};
///
/// fn build() -> petritype::Net {
///     let mut b = NetBuilder::new();
///     let inbox = b.place::<u32>("Inbox");
///     let out = b.place::<u32>("Out");
///     b.seed(inbox, [21]);
///     b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
///     b.build().expect("a net")
/// }
/// let (mut here, mut there) = (build(), build());
/// let double = here.transition_named("Double").unwrap();
/// let done = here.launch(double)?.run();
///
/// // The wrong net refuses it and hands it back, so it can still land.
/// let refused = there.distribute(done).unwrap_err();
/// assert!(matches!(refused.error(), NetError::ForeignFiring { .. }));
/// let done = refused.into_completed().expect("handed back");
/// here.distribute(done)?;
/// assert!(here.in_flight().is_empty());
/// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
/// ```
#[derive(Debug)]
pub struct DistributeError {
    /// Why the `Completed` did not land.
    error: NetError,
    /// The `Completed`, when the net left it as it was: boxed, so that the
    /// `Result` that `distribute` returns stays small, a firing that lands
    /// included.
    completed: Option<Box<Completed>>,
}

impl DistributeError {
    /// A refusal that leaves the firing as it was: the `Completed` goes
    /// back to the caller.
    pub(crate) fn handed_back(error: NetError, completed: Completed) -> DistributeError {
        DistributeError {
            error,
            completed: Some(Box::new(completed)),
        }
    }

    /// A firing distributed as a failure: nothing to give back.
    pub(crate) fn failed(failed: FiringFailed) -> DistributeError {
        DistributeError {
            error: NetError::FiringFailed(failed),
            completed: None,
        }
    }

    /// Why the `Completed` did not land: [`NetError::HeldByBody`],
    /// [`NetError::ForeignFiring`], [`NetError::NotInFlight`] or
    /// [`NetError::FiringFailed`].
    pub fn error(&self) -> &NetError {
        &self.error
    }

    /// The `Completed`, when the net left it as it was, for every reason
    /// but [`NetError::FiringFailed`].
    pub fn completed(&self) -> Option<&Completed> {
        self.completed.as_deref()
    }

    /// The error and the `Completed`, when there is one, taken apart.
    pub fn into_parts(self) -> (NetError, Option<Completed>) {
        (self.error, self.completed.map(|completed| *completed))
    }

    /// The `Completed`, when the net left it as it was, the error
    /// dropped.
    pub fn into_completed(self) -> Option<Completed> {
        self.completed.map(|completed| *completed)
    }
}

impl std::fmt::Display for DistributeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for DistributeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<DistributeError> for NetError {
    /// The error alone. A `Completed` the error carried is dropped, and its
    /// transition stays in flight in the net that launched it; take it
    /// with [`into_parts`](DistributeError::into_parts) to keep it.
    fn from(error: DistributeError) -> NetError {
        error.error
    }
}

/// One level of a [`FiringFailed::path`]: which net, one level down, the
/// failure came up from.
///
/// A path names a net by the places that lead to it, from the net that was
/// stepped down. Each step names a place of the net the steps before it
/// reach, and no step is text joined with a separator, so a place may be
/// called anything.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathStep {
    /// One of the inner nets of a token in a nested place
    /// ([`place_nested`](crate::NetBuilder::place_nested)).
    Token {
        /// The nested place.
        place: PlaceId,
        /// The token, by the id it has in that place.
        token: TokenId,
        /// The inner net's name, as [`Nested::nets`](crate::Nested::nets)
        /// gave it.
        net: String,
    },
    /// The subnet of a subnet place
    /// ([`subnet_place`](crate::NetBuilder::subnet_place)), which has no
    /// token to name.
    Subnet {
        /// The subnet place.
        place: PlaceId,
    },
}

impl PathStep {
    /// The place this step goes through: the nested place or the subnet
    /// place.
    pub fn place(&self) -> PlaceId {
        match self {
            PathStep::Token { place, .. } | PathStep::Subnet { place } => *place,
        }
    }
}

/// Which refusal a [`NetError::ClosedNet`] was. Each says, in the error's
/// message, how to make the write land.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClosedRefusal {
    /// The net is closed and the place is not one of its ports; a window,
    /// the place a net has for a port of one of its subnet places, never
    /// is one. Mark the place as a port, start the net with the token in its
    /// definition, or declare the net open.
    NotAPort,
    /// A body wrote a port of a closed net it holds that the input edge
    /// which handed it the net does not name in its
    /// [`reaching`](crate::Reaching). Name the port there, beside any the
    /// edge names already.
    Undeclared,
    /// A body wrote a closed net inside the net it holds, a net one of its
    /// tokens carries or a subnet, at any depth, at a port or not: an edge
    /// names ports of the net it hands over, not of the nets inside it. Put
    /// the token at a port of the net the body holds, and let a transition
    /// of that net carry it further in.
    Deeper,
    /// The net is inside `net`, which is closed, and the writer is outside
    /// it: the program, its drivers, the runner, or a body that does not
    /// hold `net`. Put the token at a port of `net` and let one of its
    /// transitions carry it in, or declare `net` open.
    Around {
        /// The nearest closed net around, named as
        /// [`ClosedNet::net`](NetError::ClosedNet) is.
        net: String,
    },
}

/// The message of a [`NetError::ClosedNet`], one for each
/// [`ClosedRefusal`].
fn closed_message(net: &str, place: &Option<String>, act: &str, why: &ClosedRefusal) -> String {
    let Some(place) = place else {
        // An import, which writes every place: only a closed net around
        // refuses it.
        let around = match why {
            ClosedRefusal::Around { net } => net.as_str(),
            _ => net,
        };
        return format!(
            "cannot {act} net {net:?}: the net is inside net {around:?}, which is closed, and an \
             import replaces the tokens of every place, so from outside {around:?} it is not \
             done. Import a marking into {around:?} as a whole, which loads the nets inside it, \
             or declare {around:?} open"
        );
    };
    match why {
        ClosedRefusal::Around { net: around } => format!(
            "cannot {act} {place:?} of net {net:?}: the net is inside net {around:?}, which is \
             closed, so from outside {around:?} nothing inside it is written. Put the token at a \
             port of {around:?} and let one of its transitions carry it in, or declare \
             {around:?} open"
        ),
        ClosedRefusal::Deeper => format!(
            "cannot {act} {place:?} of net {net:?} from a body: the net is inside the net the \
             body holds, and an edge's reaching names ports of the net it hands over, not of the \
             nets inside it. Put the token at a port of the net the body holds, and let a \
             transition of that net carry it further in"
        ),
        ClosedRefusal::Undeclared => format!(
            "cannot {act} {place:?} of net {net:?} from a body: the place is a port, but the edge \
             that hands the body the net does not name it in its reaching. Name it there, \
             .reaching([{place:?}]), beside any port the edge names already"
        ),
        ClosedRefusal::NotAPort => format!(
            "cannot {act} {place:?} from outside net {net:?}, which is closed: from outside it, \
             tokens are written only at its ports, and {place:?} is not one. Mark the place as a \
             port, start the net with the token in its definition, or declare the net open"
        ),
    }
}

/// An edge kind as the struct an author writes for it, for a message:
/// `SnapshotEdge` for [`EdgeKind::Snapshot`](crate::EdgeKind::Snapshot).
fn edge_struct(kind: crate::edge::EdgeKind) -> &'static str {
    use crate::edge::EdgeKind;
    match kind {
        EdgeKind::Take => "TakeEdge",
        EdgeKind::TakeAll => "TakeAllEdge",
        EdgeKind::Snapshot => "SnapshotEdge",
        EdgeKind::Mutate => "MutateEdge",
        EdgeKind::TakeWhere => "TakeWhereEdge",
        EdgeKind::MutateWhere => "MutateWhereEdge",
        EdgeKind::Drain => "DrainEdge",
        EdgeKind::Survey => "SurveyEdge",
        EdgeKind::Read => "ReadEdge",
        EdgeKind::ReadWhere => "ReadWhereEdge",
        EdgeKind::ReadAll => "ReadAllEdge",
    }
}

/// The message of a [`NetError::HeldByBody`], and of the drivers'
/// [`DriverError::HeldByBody`](crate::drivers::DriverError::HeldByBody):
/// what was refused, and what to do instead, which depends on the act.
pub(crate) fn held_message(net: &str, act: &str) -> String {
    let instead = match act {
        closed::DRAIN_EVENTS => {
            "the events of a net a body holds go to the log of the net around it, which lifts \
             them as the firing ends. Read the net's marking instead, and leave its events to \
             the program or the runner"
        }
        closed::CHOOSE => {
            "the tie-break cursor of a net a body holds moves only as the net fires, and the \
             tie-break is for the program, its drivers and the runner. Step the net to have it \
             choose and fire, or ask `select`, which moves nothing"
        }
        closed::STAGE_STEP => {
            "a staged step leaves the net's choice to be launched and distributed by name, \
             which on a net a body holds is for the program, its drivers and the runner. Step \
             the net with `step` or `step_deep`, or drive it with `drivers::run`"
        }
        closed::STEP_INSIDE => {
            "a step that leaves out the net's own turn is for the program, its drivers and the \
             runner, which have asked its selector already. Step the net with `step` or \
             `step_deep`, or drive it with `drivers::run`"
        }
        closed::DISABLE_ON_FAILURE => {
            "the switches of a net a body holds are for the program, its drivers and the \
             runner. Drive it with `OnFailure::Stop` or `OnFailure::Continue`, or leave the \
             switches to the program"
        }
        _ => {
            "the parameters, switches, tie-break, stepping rules, selector and controls of a \
             net a body holds, and firing its transitions by name, are for the program, its \
             drivers and the runner. Do it from the program, or give the net a transition of \
             its own that does it"
        }
    };
    format!("cannot {act} net {net:?} from a body of the net around it: {instead}")
}

/// The part of a [`FiringFailed`] message that says which inner net the
/// transition is in, or nothing for a transition of the net itself.
fn inside(path: &[PathStep]) -> String {
    let mut text = String::new();
    for (level, step) in path.iter().enumerate() {
        text.push_str(if level == 0 { " inside " } else { " / " });
        let _ = match step {
            PathStep::Token { place, token, net } => write!(text, "{place:?} #{token} {net:?}"),
            PathStep::Subnet { place } => write!(text, "{place:?}"),
        };
    }
    text
}

/// A body that panicked, as the failure its firing reports.
///
/// Panics are caught: [`Firing::run`](crate::Firing::run) wraps
/// the call to a detachable body, and [`launch`](crate::Net::launch) wraps
/// the inline call it makes for a transition with a mutating or a read
/// edge (one that is not
/// [detachable](crate::TransitionInfo::detachable)), both in
/// [`catch_unwind`](std::panic::catch_unwind). Either way the panic becomes
/// this error, the firing fails like one whose body returned `Err`, and the
/// body goes home to its slot, so one panicking token does not strand a
/// transition in flight for good.
///
/// What comes home is whatever the panic left behind: a body is `FnMut` and
/// may keep state of its own, and a panic can stop it half way through
/// changing it. The net's own state is not in question (nothing was
/// distributed), and a token a mutating edge's body had already changed
/// stays changed, since the change was made where the token lies.
///
/// Nothing can be caught under `panic = "abort"`, where the process dies at
/// the panic, as the profile asked.
///
/// `Clone`, `PartialEq` and `Eq` (since 0.4.0): it is its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("panicked: {message}")]
pub struct BodyPanicked {
    /// The panic payload as text: the message a `panic!` carried, or
    /// `"panic payload of unknown type"` for a payload that is neither a
    /// `&str` nor a `String`, which only
    /// [`panic_any`](std::panic::panic_any) can produce.
    pub message: String,
}

impl BodyPanicked {
    /// Reads the message out of a caught payload.
    ///
    /// The two types `panic!` itself produces, in that order: a literal
    /// message is a `&'static str` and a formatted one a `String`. Anything
    /// else came from [`panic_any`](std::panic::panic_any) and has no text to
    /// read, so the type is not named either: `Any` cannot say what it is.
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
