//! Closed nets: every net takes tokens from outside it only at its ports,
//! unless its definition declares it open, a body is held to what its edge
//! declares, and a closed net shields the nets inside it (DESIGN §6.9;
//! stage one 2026-11-23, stage two 2026-11-28).
//!
//! Three checks live here, and every public writing method of [`Net`] asks
//! one of them first:
//!
//! - **Tokens** ([`Net::admit`]). Putting, taking, removing, replacing or
//!   setting a token at a place of a net, in this order:
//!   1. while [`Net::press`] runs one of the net's own controls, the
//!      handler is inside, and writes anywhere;
//!   2. a body that holds the net by an input edge (a mark *by an edge*)
//!      writes a closed net at the ports the edge names in its
//!      [`reaching`](crate::Reaching) and nowhere else, and an open net
//!      anywhere but at a window onto a closed subnet. Nothing around the
//!      net counts: the body is on its side of it;
//!   3. a body that reaches, through a scope, a net inside the net it holds
//!      (a mark *deeper*) writes no place of a closed one, and is judged on
//!      an open one as in 4;
//!   4. anyone else, the program, its drivers, the runner, and a body at a
//!      net no edge handed it, writes a closed net at its ports only and an
//!      open one anywhere, unless the net is *shielded*: lent by
//!      [`Net::nested_scope`] or [`Net::subnet_scope`] from inside a closed
//!      net, at any depth, when nothing lands.
//!
//!   The net's own moves go round the check by internal paths: its seeds,
//!   a firing's outputs, a failed firing's restore, an import, a fork, a
//!   deep step.
//! - **Imports** ([`Net::admit_import`]). An import writes every place:
//!   refused into a held net, as a knob is, and into a shielded one.
//! - **Knobs and firing by name** ([`Net::admit_knob`]). While a body holds
//!   a net, closed or declared open, setting a parameter, a switch, the
//!   tie-break, a stepping rule or a selector, firing one of its
//!   transitions by name, pressing a control, draining its events, moving
//!   its tie-break cursor with [`Net::choose`], a staged step and driving
//!   it with [`OnFailure::Disable`](crate::drivers::OnFailure::Disable) are
//!   refused. Stepping and driving it otherwise stay allowed, and go round
//!   the check by internal paths, as the engine's own lifting of events
//!   does. The program, its drivers and the runner never hold a marked
//!   net, so they keep all of it, on a shielded net too.
//!
//! **The mark.** A net a firing lends or hands its body carries a
//! [`Mark`], a [`Weak`] to the [`Hold`] the firing owns and the reach: the
//! input edge, whose ports the hold keeps, or *deeper*. It lapses by itself
//! when the firing ends, wherever the token went. A scope opened on a
//! marked net lends the nets inside it marked *deeper* for the closure's
//! time. A refusal is recorded on the hold, and the firing fails with the
//! first one after its body returns, even when the body dropped the `Err`,
//! or caught the panic, that told it.
//!
//! **The shield.** A scope opened on a closed net, or on a shielded one,
//! lends the nets inside it with a [`Weak`] to the name of the nearest
//! closed net around (through a window, the subnet behind it first), kept
//! by the scope: it lapses when the scope ends,
//! also when the closure panics, and no fork, save or copy carries it. A
//! scope opened by a net's own control handler shields nothing by that
//! net, the handler being inside it, and the looking stops at a net a body
//! holds by an edge, whose side the body is on.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::error::{ClosedRefusal, NetError};
use crate::ids::{NetId, PlaceId};
use crate::nested::NestedVtable;
use crate::net::Net;
use crate::place::{ErasedToken, PlaceSlot, Window};

/// What a refused token write was, as [`NetError::ClosedNet`] says it.
pub(crate) const PUT: &str = "put a token into";
/// See [`PUT`].
pub(crate) const ADD: &str = "add tokens to";
/// See [`PUT`].
pub(crate) const SET: &str = "set the tokens of";
/// See [`PUT`].
pub(crate) const REMOVE: &str = "remove a token from";
/// See [`PUT`].
pub(crate) const REPLACE: &str = "replace a token in";

/// What a refused knob or firing by name was, as [`NetError::HeldByBody`]
/// says it.
pub(crate) const SET_PARAM: &str = "set a parameter of";
/// See [`SET_PARAM`].
pub(crate) const SWITCH: &str = "switch a transition of";
/// See [`SET_PARAM`].
pub(crate) const SET_TIE_BREAK: &str = "set the tie-break of";
/// See [`SET_PARAM`].
pub(crate) const SET_STEPPING: &str = "set a stepping rule of";
/// See [`SET_PARAM`].
pub(crate) const SET_SELECTOR: &str = "set the selector of";
/// See [`SET_PARAM`].
pub(crate) const CLEAR_SELECTOR: &str = "clear the selector of";
/// See [`SET_PARAM`].
pub(crate) const FIRE: &str = "fire a transition of";
/// See [`SET_PARAM`].
pub(crate) const LAUNCH: &str = "launch a transition of";
/// See [`SET_PARAM`].
pub(crate) const DISTRIBUTE: &str = "distribute a firing of";
/// See [`SET_PARAM`].
pub(crate) const STEP_INSIDE: &str = "step the nets inside";
/// See [`SET_PARAM`].
pub(crate) const PRESS: &str = "press a control of";
/// See [`SET_PARAM`].
#[cfg(feature = "serde")]
pub(crate) const IMPORT: &str = "import a marking into";
/// See [`SET_PARAM`].
pub(crate) const DRAIN_EVENTS: &str = "drain the events of";
/// See [`SET_PARAM`].
pub(crate) const CHOOSE: &str = "choose a transition of";
/// See [`SET_PARAM`].
pub(crate) const STAGE_STEP: &str = "stage a step of";
/// See [`SET_PARAM`].
pub(crate) const DISABLE_ON_FAILURE: &str = "switch off the failing transitions of";

/// What a firing shares with every net it lends or hands its body: the
/// first refusal one of those nets gave, and the ports each input edge of
/// its transition names.
///
/// The firing owns the [`Arc`] and each net it reaches holds a [`Weak`] in
/// its [`Mark`] (`Net::held`), so a net is held exactly while the firing
/// that marked it is under way. It is `Send + Sync`, because a detachable
/// body may run on another thread with the token it owns.
#[derive(Debug)]
pub(crate) struct Hold {
    /// The first refusal, kept until the firing takes it.
    refusal: Mutex<Option<NetError>>,
    /// The transition's `reaches`, one entry per input edge, in edge order.
    reaches: Arc<[Vec<String>]>,
}

impl Hold {
    /// Keeps `refusal` unless an earlier one is kept already.
    fn record(&self, refusal: NetError) {
        let mut kept = self.refusal.lock().unwrap_or_else(PoisonError::into_inner);
        kept.get_or_insert(refusal);
    }

    /// The first refusal, if there was one, taken out.
    pub(crate) fn take_refusal(&self) -> Option<NetError> {
        self.refusal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Whether the input edge at `edge` names `port` in its reaching.
    fn reaches(&self, edge: usize, port: &str) -> bool {
        self.reaches
            .get(edge)
            .is_some_and(|names| names.iter().any(|name| name == port))
    }
}

/// How a body holds a net: the firing's [`Hold`], and what the body may
/// write there.
#[derive(Debug, Clone)]
pub(crate) struct Mark {
    /// The firing's hold, which lapses when the firing ends.
    hold: Weak<Hold>,
    /// How the body came by the net.
    reach: Reach,
}

/// How a body came by a net it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// The input edge at this index, in the transition's edge order, lent
    /// or handed it the net: a closed one takes the body's writes at the
    /// ports the edge names.
    Edge(usize),
    /// A scope lent it the net from inside one the body holds, at any
    /// depth: a closed one takes none of the body's writes.
    Deeper,
}

impl Mark {
    /// The mark of the input edge at `edge` of the firing whose hold is
    /// `hold`, which is made the first time a firing lends or hands a net,
    /// with its transition's `reaches`: most firings hand none, and
    /// allocate nothing.
    pub(crate) fn by_edge(
        hold: &mut Option<Arc<Hold>>,
        reaches: &Arc<[Vec<String>]>,
        edge: usize,
    ) -> Mark {
        let hold = hold.get_or_insert_with(|| {
            Arc::new(Hold {
                refusal: Mutex::default(),
                reaches: Arc::clone(reaches),
            })
        });
        Mark {
            hold: Arc::downgrade(hold),
            reach: Reach::Edge(edge),
        }
    }

    /// Whether the firing that set it is still under way.
    fn is_live(&self) -> bool {
        self.hold.strong_count() > 0
    }
}

/// What [`mark`] found on each net of a token, by the net's id, for
/// [`unmark`].
pub(crate) type Marked = Vec<(NetId, Option<Mark>)>;

/// Marks every net of an erased token of a place with `nested`'s vtable
/// with `mark`, and gives back what each was marked with before, by the
/// net's id, for [`unmark`].
pub(crate) fn mark(nested: NestedVtable, value: &mut ErasedToken, mark: &Mark) -> Marked {
    (nested.nets_mut)(&mut **value)
        .into_iter()
        .map(|(_, inner)| (inner.id(), inner.held.replace(mark.clone())))
        .collect()
}

/// Puts back what [`mark`] found on each net of the token, matched by the
/// net's id (since 2026-11-28; until then by position, which a body that
/// reordered the token's nets, or added one, misapplied). A net the token
/// did not carry when it was marked, one the body built and put in place
/// of one it was lent or moved in from elsewhere, is a net of its own and
/// is held by nothing once the firing ends.
pub(crate) fn unmark(nested: NestedVtable, value: &mut ErasedToken, mut before: Marked) {
    for (_, inner) in (nested.nets_mut)(&mut **value) {
        let id = inner.id();
        inner.held = before
            .iter()
            .position(|(was, _)| *was == id)
            .and_then(|at| before.swap_remove(at).1);
    }
}

/// What a [`nested_scope`](Net::nested_scope) or a
/// [`subnet_scope`](Net::subnet_scope) lends its nets with, for the
/// closure's time: the body's mark one net further in when a body holds
/// the net the scope is opened on, and the shield when that net is closed
/// or shielded itself.
///
/// The scope keeps it until it ends, also when the closure panics: the
/// shield's name lives in it, so every net it shielded is shielded no
/// longer once it is dropped, wherever that net went.
pub(crate) struct Lending {
    /// The mark to lend with, *deeper*.
    mark: Option<Mark>,
    /// The name of the nearest closed net around the nets lent.
    shield: Option<Arc<str>>,
}

/// What a [`Lending`] found on one net, for [`Lending::restore`].
pub(crate) struct Found {
    held: Option<Mark>,
    shield: Option<Weak<str>>,
}

impl Lending {
    /// Lends `net` with this lending's mark and shield, and gives back what
    /// it had.
    pub(crate) fn lend(&self, net: &mut Net) -> Found {
        Found {
            held: std::mem::replace(&mut net.held, self.mark.clone()),
            shield: std::mem::replace(&mut net.shield, self.shield.as_ref().map(Arc::downgrade)),
        }
    }

    /// [`lend`](Lending::lend) for every net of an erased token of a place
    /// with `nested`'s vtable, each found by the net's id, for
    /// [`restore_all`](Lending::restore_all).
    pub(crate) fn lend_all(
        &self,
        nested: NestedVtable,
        value: &mut ErasedToken,
    ) -> Vec<(NetId, Found)> {
        (nested.nets_mut)(&mut **value)
            .into_iter()
            .map(|(_, inner)| (inner.id(), self.lend(inner)))
            .collect()
    }

    /// Puts back on `net` what [`lend`](Lending::lend) found there.
    pub(crate) fn restore(net: &mut Net, found: Found) {
        net.held = found.held;
        net.shield = found.shield;
    }

    /// [`restore`](Lending::restore) for every net of the token, matched by
    /// the net's id, as [`unmark`] matches. A net the token did not carry
    /// when it was lent, one the closure built or moved in, is lent nothing
    /// once the scope ends.
    pub(crate) fn restore_all(
        nested: NestedVtable,
        value: &mut ErasedToken,
        mut found: Vec<(NetId, Found)>,
    ) {
        for (_, inner) in (nested.nets_mut)(&mut **value) {
            let id = inner.id();
            let was = found
                .iter()
                .position(|(lent, _)| *lent == id)
                .map(|at| found.swap_remove(at).1);
            Lending::restore(
                inner,
                was.unwrap_or(Found {
                    held: None,
                    shield: None,
                }),
            );
        }
    }
}

/// A refusal of the token check, before the act is known: the net and the
/// place it names, and why.
struct Refused {
    net: String,
    place: String,
    why: ClosedRefusal,
}

impl Net {
    /// Whether this net's definition declared it open, with
    /// [`NetBuilder::open`](crate::NetBuilder::open): anyone outside it may
    /// then write to any of its places, ports or not, unless a closed net
    /// around it shields it.
    ///
    /// The declaration, not the rule's answer, which is
    /// [`is_closed`](Net::is_closed). It is part of the definition, so a
    /// fork, a restored net and a rebuild from the same factory keep it, and
    /// a saved marking does not carry it.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether this net is **closed**: from outside it, tokens may be put,
    /// taken, removed, replaced or mutated only at its
    /// [ports](crate::NetBuilder::port).
    ///
    /// Every net is closed unless its definition declares it
    /// [open](crate::NetBuilder::open) (since 2026-11-28; from 2026-11-23
    /// only a net that marked a port was). A closed net that marks no port
    /// takes no token from outside at all.
    ///
    /// Outside a net, for this rule: a body, guard or key function of a
    /// transition of the net around it, by any edge; the program and its
    /// drivers; a runner's requests. Inside it: its own transitions' bodies,
    /// guards and keys, the handlers of its own controls, and the tokens its
    /// definition starts it with. A write outside the ports of a closed net
    /// is [`NetError::ClosedNet`], which says the three ways to make it
    /// land: mark the place as a port, start the net with the token in its
    /// definition, or declare the net open. Reading, stepping and the
    /// whole-state acts (importing a marking, restoring, forking, saving)
    /// stay allowed for the program.
    ///
    /// A question changes no net: a guard, a priority and a selector are
    /// given `&Net` and a key function `&T`, so the compiler refuses them
    /// every change through this library. It cannot see a change through
    /// interior mutability in a token (a `Mutex` reached through `&T`) or
    /// through state the closure captured. An output distribution function
    /// owns the body's result, so a net that result carries is under the
    /// body's rule there.
    ///
    /// Three more rules (since 2026-11-28), each a [`ClosedRefusal`]:
    ///
    /// - **A body is held to its edge.** A body writes a closed net it holds
    ///   only at the ports that the input edge which handed it the net names
    ///   in its [`reaching`](crate::Reaching): an edge that names none lets
    ///   it write no port ([`ClosedRefusal::Undeclared`]). Naming a port the
    ///   body never writes, or one the net lacks, is no error.
    /// - **A body writes no closed net inside the one it holds**, a net one
    ///   of its tokens carries or a subnet, at any depth, at a port or not
    ///   ([`ClosedRefusal::Deeper`]): an edge names ports of the net it
    ///   hands over only.
    /// - **A closed net shields the nets inside it.** Lent by
    ///   [`nested_scope`](Net::nested_scope) or
    ///   [`subnet_scope`](Net::subnet_scope) from inside a closed net, at
    ///   any depth, a net takes no token from the program, its drivers or
    ///   the runner, at a port or not, and no import
    ///   ([`ClosedRefusal::Around`]). It lands only when every net around is
    ///   declared open, the net's own rule allowing it; or the closed net is
    ///   given a port and a transition that carries the token in. The knobs
    ///   of a shielded net stay the program's.
    ///
    /// A *window*, the place a net has for a port of one of its subnet
    /// places ([`port_of`](crate::NetBuilder::port_of)), is a place of the
    /// net that holds it, and can never be one of that net's ports: a write
    /// at it from outside is judged by the holding net's rule first, so it
    /// is refused when the holding net is closed, and lands, the subnet's
    /// port taking it, when the holding net is open (and not shielded). A
    /// body that holds the open net is refused a window onto a closed
    /// subnet, a net inside the one it holds.
    ///
    /// ```
    /// use petritype::{NetBuilder, NetError};
    ///
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<String>("Inbox");
    /// let ledger = b.place::<String>("Ledger");
    /// b.port(inbox);
    /// let mut post = b.build()?;
    ///
    /// assert!(post.is_closed(), "it does not declare itself open");
    /// post.put(inbox, "Dear Ada".to_owned());
    /// let refused = post.try_put(ledger, "IOU".to_owned());
    /// assert!(matches!(refused, Err(NetError::ClosedNet { .. })));
    /// assert_eq!(post.count(ledger.id()), 0, "nothing landed");
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn is_closed(&self) -> bool {
        !self.open
    }

    /// Whether a firing has lent or handed this net to its body and has not
    /// ended: the mark that refuses knobs and firing by name.
    pub(crate) fn is_held(&self) -> bool {
        self.held.as_ref().is_some_and(Mark::is_live)
    }

    /// The mark this net carries, while the firing that set it lasts.
    fn live_mark(&self) -> Option<&Mark> {
        self.held.as_ref().filter(|mark| mark.is_live())
    }

    /// The name of the nearest closed net around this one, while a scope
    /// that lent it from inside one lasts.
    pub(crate) fn live_shield(&self) -> Option<Arc<str>> {
        self.shield.as_ref().and_then(Weak::upgrade)
    }

    /// What a scope opened on this net lends with: through `window` for
    /// [`nested_scope`](Net::nested_scope) on a window, whose tokens rest in
    /// a port of the subnet behind it, and `None` otherwise.
    pub(crate) fn lending(&self, window: Option<Window>) -> Lending {
        let mark = self.live_mark().map(|mark| Mark {
            hold: Weak::clone(&mark.hold),
            reach: Reach::Deeper,
        });
        Lending {
            mark,
            shield: self.shield_for(window).map(Arc::from),
        }
    }

    /// The nearest closed net around the nets a scope opened on this net
    /// lends, by name, if there is one the opener is outside of.
    fn shield_for(&self, window: Option<Window>) -> Option<String> {
        if let Some(window) = window {
            let subnet = self.subnet_at(window);
            if subnet.is_closed() {
                return Some(subnet.display_name());
            }
        }
        if self.inside {
            // A handler of this net's own control is inside it.
            return None;
        }
        if self.is_closed() {
            return Some(self.display_name());
        }
        match self.live_mark() {
            // A body is on its side of the net its edge handed it.
            Some(Mark {
                reach: Reach::Edge(_),
                ..
            }) => None,
            _ => self.live_shield().map(|around| around.to_string()),
        }
    }

    /// The subnet behind a window, to read.
    fn subnet_at(&self, window: Window) -> &Net {
        &self.places[window.subnet.0 as usize]
            .subnet
            .as_deref()
            .expect("a window's subnet place holds its subnet")
            .net
    }

    /// How a refusal names this net: by the template that built it, or by
    /// its id when no template did.
    pub(crate) fn display_name(&self) -> String {
        #[cfg(feature = "serde")]
        if let Some(template) = self.template() {
            return template.to_owned();
        }
        format!("{:?}", self.id)
    }

    /// The refusal `refusal()` makes, recorded on the mark first when a
    /// body holds this net.
    fn refuse(&self, refusal: impl Fn() -> NetError) -> NetError {
        if let Some(hold) = self.live_mark().and_then(|mark| mark.hold.upgrade()) {
            hold.record(refusal());
        }
        refusal()
    }

    /// The token check, as the module comment orders it:
    /// [`NetError::ClosedNet`] for a write from outside at a place of this
    /// net that the rule refuses, `act` saying what the write was ("put a
    /// token into").
    ///
    /// A place this net does not have passes, so that the caller's own
    /// check reports it as it always has.
    pub(crate) fn admit(&self, place: PlaceId, act: &'static str) -> Result<(), NetError> {
        if self.inside {
            return Ok(());
        }
        let Some(slot) = self.places.get(place.0 as usize) else {
            return Ok(());
        };
        let here = |why| Refused {
            net: self.display_name(),
            place: slot.meta.name.clone(),
            why,
        };
        let refused = match (self.live_mark(), self.is_closed()) {
            (Some(mark), true) => match mark.reach {
                Reach::Deeper => Some(here(ClosedRefusal::Deeper)),
                _ if !slot.meta.port => Some(here(ClosedRefusal::NotAPort)),
                Reach::Edge(edge) => {
                    let named = mark
                        .hold
                        .upgrade()
                        .is_some_and(|hold| hold.reaches(edge, &slot.meta.name));
                    (!named).then(|| here(ClosedRefusal::Undeclared))
                }
            },
            (Some(mark), false) => self
                .behind_closed_window(slot)
                .or_else(|| match mark.reach {
                    Reach::Edge(_) => None,
                    Reach::Deeper => self.around(slot),
                }),
            (None, true) if !slot.meta.port => Some(here(ClosedRefusal::NotAPort)),
            (None, _) => self.around(slot),
        };
        match refused {
            None => Ok(()),
            Some(Refused { net, place, why }) => Err(self.refuse(|| NetError::ClosedNet {
                net: net.clone(),
                place: Some(place.clone()),
                act,
                why: why.clone(),
            })),
        }
    }

    /// For a body, a window of this open net onto a closed subnet: a net
    /// inside the one it holds, named with its port.
    fn behind_closed_window(&self, slot: &PlaceSlot) -> Option<Refused> {
        let window = slot.meta.window?;
        let subnet = self.subnet_at(window);
        subnet.is_closed().then(|| Refused {
            net: subnet.display_name(),
            place: subnet.places[window.port.0 as usize].meta.name.clone(),
            why: ClosedRefusal::Deeper,
        })
    }

    /// The shield's refusal at `slot`, while a scope lends this net from
    /// inside a closed one.
    fn around(&self, slot: &PlaceSlot) -> Option<Refused> {
        self.live_shield().map(|around| Refused {
            net: self.display_name(),
            place: slot.meta.name.clone(),
            why: ClosedRefusal::Around {
                net: around.to_string(),
            },
        })
    }

    /// [`admit`](Net::admit) for a method that has no `Result` to return
    /// it in: the refusal's message is the panic's.
    pub(crate) fn admit_or_panic(&self, place: PlaceId, act: &'static str) {
        if let Err(refusal) = self.admit(place, act) {
            panic!("{refusal}");
        }
    }

    /// The import check: [`NetError::HeldByBody`] while a body holds this
    /// net, as for a knob; [`NetError::InFlight`] while a firing of this
    /// net, or of one of its subnets at any depth, is in flight, which
    /// would land into the marking imported; and [`NetError::ClosedNet`]
    /// with no place while a scope lends it from inside a closed net, since
    /// an import writes every place.
    #[cfg(feature = "serde")]
    pub(crate) fn admit_import(&self) -> Result<(), NetError> {
        self.admit_knob(IMPORT)?;
        self.check_landed_deep()?;
        if self.inside {
            return Ok(());
        }
        match self.live_shield() {
            None => Ok(()),
            Some(around) => Err(NetError::ClosedNet {
                net: self.display_name(),
                place: None,
                act: IMPORT,
                why: ClosedRefusal::Around {
                    net: around.to_string(),
                },
            }),
        }
    }

    /// The knob check: [`NetError::HeldByBody`] while a body holds this net,
    /// open or closed, `act` saying what was asked ("set a parameter of").
    pub(crate) fn admit_knob(&self, act: &'static str) -> Result<(), NetError> {
        if !self.is_held() {
            return Ok(());
        }
        Err(self.refuse(|| NetError::HeldByBody {
            net: self.display_name(),
            act,
        }))
    }

    /// [`admit_knob`](Net::admit_knob) for a step, whose error is a
    /// [`StepError`](crate::StepError): its
    /// [`HeldByBody`](crate::StepError::HeldByBody), the refusal recorded on
    /// the body's firing as a knob's is.
    pub(crate) fn admit_step(&self, act: &'static str) -> Result<(), crate::StepError> {
        if !self.is_held() {
            return Ok(());
        }
        let net = self.display_name();
        self.refuse(|| NetError::HeldByBody {
            net: net.clone(),
            act,
        });
        Err(crate::StepError::HeldByBody { net, act })
    }

    /// [`admit_knob`](Net::admit_knob) for a method that has no `Result` to
    /// return it in. Only a body can meet it, and its firing fails with the
    /// refusal the mark kept.
    pub(crate) fn admit_knob_or_panic(&self, act: &'static str) {
        if let Err(refusal) = self.admit_knob(act) {
            panic!("{refusal}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Weak};

    use super::{Hold, Mark, mark, unmark};
    use crate::place::ErasedToken;
    use crate::{MutateEdge, Net, NetBuilder, TakeEdge};

    /// A closed post: `Inbox`, its port.
    fn post() -> Net {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        b.port(inbox);
        b.build().expect("a post")
    }

    /// A closed net with a post in a token of `Sacks` and one in the subnet
    /// place `Branch`.
    fn office() -> Net {
        let mut b = NetBuilder::new();
        let sacks = b.place_nested::<Net>("Sacks");
        b.seed(sacks, [post()]);
        b.subnet_place("Branch", post());
        b.build().expect("an office")
    }

    fn lent_nothing(net: &Net) -> bool {
        net.held.is_none() && net.shield.is_none()
    }

    /// Two nets a token carries, as a field of each.
    #[derive(Debug)]
    struct Pair {
        left: Net,
        right: Net,
    }

    impl crate::Nested for Pair {
        fn nets(&self) -> Vec<(&str, &Net)> {
            vec![("left", &self.left), ("right", &self.right)]
        }

        fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
            vec![("left", &mut self.left), ("right", &mut self.right)]
        }
    }

    /// What `unmark` puts back follows each net by its id, so a body that
    /// swaps a token's nets, or puts a net of its own in, does not hand one
    /// net the other's mark; by position, the two were swapped.
    #[test]
    fn unmark_puts_back_each_net_s_own_mark_by_its_id() {
        let nested = crate::nested::NestedVtable::of::<Pair>();
        // Three firings' holds, kept alive here so that each mark is live.
        let mut holds: [Option<Arc<Hold>>; 3] = Default::default();
        let reaches: Arc<[Vec<String>]> = Arc::from(vec![Vec::new()]);
        let [left_was, right_was, firing] = holds
            .each_mut()
            .map(|hold| Mark::by_edge(hold, &reaches, 0));
        let same =
            |a: &Option<Mark>, b: &Mark| a.as_ref().is_some_and(|a| Weak::ptr_eq(&a.hold, &b.hold));

        let mut pair = Pair {
            left: post(),
            right: post(),
        };
        pair.left.held = Some(left_was.clone());
        pair.right.held = Some(right_was.clone());
        let mut value: ErasedToken = Box::new(pair);
        let before = mark(nested, &mut value, &firing);
        // The body swaps the two nets.
        let pair = value.downcast_mut::<Pair>().unwrap();
        std::mem::swap(&mut pair.left, &mut pair.right);
        unmark(nested, &mut value, before);
        let pair = value.downcast_ref::<Pair>().unwrap();
        assert!(
            same(&pair.left.held, &right_was),
            "the right net, now on the left"
        );
        assert!(same(&pair.right.held, &left_was));

        // A net of the body's own is held by nothing afterwards.
        let before = mark(nested, &mut value, &firing);
        value.downcast_mut::<Pair>().unwrap().left = post();
        unmark(nested, &mut value, before);
        let pair = value.downcast_ref::<Pair>().unwrap();
        assert!(pair.left.held.is_none(), "a new net");
        assert!(same(&pair.right.held, &left_was));
    }

    #[test]
    fn a_scope_takes_its_shield_away_also_when_its_closure_panics() {
        let mut office = office();
        let sacks = office.place_named("Sacks").unwrap();
        let branch = office.place_named("Branch").unwrap();
        let (token, _) = office.tokens(sacks).next().unwrap();
        let broke = catch_unwind(AssertUnwindSafe(|| {
            office.nested_scope(sacks, token, |nets| {
                assert!(nets[0].1.live_shield().is_some(), "shielded in the scope");
                panic!("the closure broke");
            })
        }));
        assert!(broke.is_err());
        assert!(lent_nothing(office.nested(sacks, token).unwrap()[0].1));
        let broke = catch_unwind(AssertUnwindSafe(|| {
            office.subnet_scope(branch, |subnet| {
                assert!(subnet.live_shield().is_some(), "shielded in the scope");
                panic!("the closure broke");
            })
        }));
        assert!(broke.is_err());
        assert!(lent_nothing(office.subnet(branch).unwrap()));
    }

    #[test]
    fn a_body_s_mark_one_net_further_in_goes_also_when_the_scope_s_closure_panics() {
        let mut unit = NetBuilder::new();
        let sacks = unit.place_nested::<Net>("Sacks");
        unit.seed(sacks, [post()]);
        unit.open();
        let unit = unit.build().expect("a unit");
        let mut b = NetBuilder::new();
        let orders = b.place::<u32>("Orders");
        let field = b.place_nested::<Net>("Field");
        b.seed(orders, [1]);
        b.seed(field, [unit]);
        b.transition(
            "Use",
            (TakeEdge(orders), MutateEdge(field)),
            (),
            move |_: u32, unit: &mut Net| {
                let (token, _) = unit.tokens(sacks.id()).next().unwrap();
                let broke = catch_unwind(AssertUnwindSafe(|| {
                    unit.nested_scope(sacks.id(), token, |nets| {
                        assert!(nets[0].1.is_held(), "held one net further in");
                        panic!("the closure broke");
                    })
                }));
                assert!(broke.is_err());
                let sack = unit.nested(sacks.id(), token).unwrap()[0].1;
                assert!(lent_nothing(sack), "{sack:?}");
            },
        );
        let mut net = b.build().expect("a field");
        net.step().expect("the body caught its panic").expect("Use");
    }
}
