//! Closed nets: a net that marks a port takes tokens from outside it only at
//! its ports (DESIGN §6.9, since 2026-11-23).
//!
//! Two checks live here, and every public writing method of [`Net`] asks one
//! of them first:
//!
//! - **Tokens.** On a closed net ([`Net::is_closed`]), putting, taking,
//!   removing, replacing or setting tokens from outside is allowed at a port
//!   and refused anywhere else, whoever calls: the program, a body holding
//!   the net, a closure of [`Net::nested_scope`] or [`Net::subnet_scope`].
//!   The net's own moves go round the check by internal paths: its seeds, a
//!   firing's outputs, a failed firing's restore, an import, a fork, a deep
//!   step, and the handler of one of its own controls while
//!   [`Net::press`] runs it.
//! - **Knobs and firing by name.** A net a firing lends or hands its body
//!   carries a *mark*, a [`Hold`] shared with that firing, for as long as
//!   the body has it. While it is marked, and it is under the rule (it
//!   marks a port, whether it is closed or declared open), setting a
//!   parameter, a switch, the tie-break, a stepping rule or a selector,
//!   firing one of its transitions by name, pressing a control and
//!   importing a marking are refused. A held net that marks no port keeps
//!   all of them in stage one. The program, its drivers and the runner
//!   never hold a marked net, so they keep all of it.
//!
//! A refusal is recorded on the mark when there is one, and the firing that
//! put the mark there fails with the first refusal after its body returns,
//! even when the body dropped the `Err`, or caught the panic, that told it.
//! The mark is a [`Weak`] to the firing's [`Hold`], so it lapses by itself
//! when the firing ends, wherever the token went.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::error::NetError;
use crate::ids::PlaceId;
use crate::nested::NestedVtable;
use crate::net::Net;
use crate::place::ErasedToken;

/// Whether a net that marks no port is under the rule: false in stage one
/// (2026-11-23), where a net comes under it only by marking a port. Stage
/// two puts every net under it, closing each that does not declare itself
/// open and refusing knobs to a body on any net it holds, which is this
/// constant made true.
const UNDER_RULE_WITHOUT_PORTS: bool = false;

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

/// The mark a firing shares with every net it lends or hands its body, and
/// what that firing learns through it: the first refusal one of those nets
/// gave.
///
/// The firing owns the [`Arc`] and each net it reaches holds a [`Weak`]
/// (`Net::held`), so a net is held exactly while the firing that marked it
/// is under way. It is `Send + Sync`, because a detachable body may run on
/// another thread with the token it owns.
#[derive(Debug, Default)]
pub(crate) struct Hold {
    /// The first refusal, kept until the firing takes it.
    refusal: Mutex<Option<NetError>>,
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
}

/// Marks every net of an erased token of a place with `nested`'s vtable as
/// held by the firing that owns `hold`, and gives back what each was
/// marked with before, in [`Nested::nets_mut`](crate::Nested::nets_mut)
/// order, for [`unmark`].
pub(crate) fn mark(
    nested: NestedVtable,
    value: &mut ErasedToken,
    hold: &Weak<Hold>,
) -> Vec<Option<Weak<Hold>>> {
    (nested.nets_mut)(&mut **value)
        .into_iter()
        .map(|(_, inner)| inner.held.replace(Weak::clone(hold)))
        .collect()
}

/// Puts back what [`mark`] found on each net of the token. A net the body
/// put in place of one it was lent is a new net, and is given what the one
/// it replaced had.
pub(crate) fn unmark(
    nested: NestedVtable,
    value: &mut ErasedToken,
    before: Vec<Option<Weak<Hold>>>,
) {
    for ((_, inner), held) in (nested.nets_mut)(&mut **value).into_iter().zip(before) {
        inner.held = held;
    }
}

/// The mark of a firing that may need one, made the first time a firing
/// lends or hands a net: most firings hand none, and allocate nothing.
pub(crate) fn hold_of(hold: &mut Option<Arc<Hold>>) -> Weak<Hold> {
    Arc::downgrade(hold.get_or_insert_with(Arc::default))
}

impl Net {
    /// Whether this net's definition declared it open, with
    /// [`NetBuilder::open`](crate::NetBuilder::open): anyone outside it may
    /// then write to any of its places, ports or not.
    ///
    /// The declaration, not the rule's answer, which is
    /// [`is_closed`](Net::is_closed). It is part of the definition, so a
    /// fork, a restored net and a rebuild from the same factory keep it, and
    /// a saved marking does not carry it.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether this net is **closed**: from outside it, tokens may be put,
    /// taken, removed, replaced or mutated only at its ports (DESIGN §6.9,
    /// since 2026-11-23).
    ///
    /// A net is closed when it marks at least one place as a
    /// [port](crate::NetBuilder::port) and does not declare itself
    /// [open](crate::NetBuilder::open). A net that marks no port is open, as
    /// every net was before. (Stage two, announced but not built, closes
    /// every net that does not declare itself open.)
    ///
    /// Outside a net, for this rule: a body, guard or key function of a
    /// transition of the net around it, by any edge; the program and its
    /// drivers; a runner's requests. Inside it: its own transitions' bodies,
    /// guards and keys, the handlers of its own controls, and the tokens its
    /// definition starts it with. A write outside the ports of a closed net
    /// is [`NetError::ClosedNet`], which says the three ways to make it
    /// land: mark the place as a port, start the net with the token in its
    /// definition, or declare the net open. Reading, stepping and the
    /// whole-state acts (loading or importing a marking, restoring,
    /// forking, saving) stay allowed for the program.
    ///
    /// A *window*, the place a net has for a port of one of its subnet
    /// places ([`port_of`](crate::NetBuilder::port_of)), is a place of the
    /// net that holds it, and can never be one of that net's ports: a write
    /// at it from outside is judged by the holding net's rule first, so it
    /// is refused when the holding net is closed, and lands, the subnet's
    /// port taking it, when the holding net is open.
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
    /// assert!(post.is_closed(), "it marks a port");
    /// post.put(inbox, "Dear Ada".to_owned());
    /// let refused = post.try_put(ledger, "IOU".to_owned());
    /// assert!(matches!(refused, Err(NetError::ClosedNet { .. })));
    /// assert_eq!(post.count(ledger.id()), 0, "nothing landed");
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn is_closed(&self) -> bool {
        !self.open && self.is_under_rule()
    }

    /// Whether the rule applies to this net at all: in stage one, whether
    /// it marks a port. A net under the rule is closed unless it declares
    /// itself open, and a body that holds it is refused its knobs and
    /// firing by name either way. The one test stage two widens, through
    /// [`UNDER_RULE_WITHOUT_PORTS`].
    pub(crate) fn is_under_rule(&self) -> bool {
        UNDER_RULE_WITHOUT_PORTS || self.marks_ports
    }

    /// Whether a firing has lent or handed this net to its body and has not
    /// ended: the mark that refuses knobs and firing by name.
    pub(crate) fn is_held(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|hold| hold.strong_count() > 0)
    }

    /// The mark this net carries, while the firing that set it lasts: what
    /// [`nested_scope`](Net::nested_scope) and
    /// [`subnet_scope`](Net::subnet_scope) pass to the nets they lend.
    pub(crate) fn live_mark(&self) -> Option<Weak<Hold>> {
        self.held
            .as_ref()
            .filter(|hold| hold.strong_count() > 0)
            .cloned()
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
        if let Some(hold) = self.held.as_ref().and_then(Weak::upgrade) {
            hold.record(refusal());
        }
        refusal()
    }

    /// The token check: [`NetError::ClosedNet`] for a write from outside at
    /// a place of a closed net that is not one of its ports, `act` saying
    /// what the write was ("put a token into").
    ///
    /// A place this net does not have passes, so that the caller's own
    /// check reports it as it always has.
    pub(crate) fn admit(&self, place: PlaceId, act: &'static str) -> Result<(), NetError> {
        if self.inside || !self.is_closed() {
            return Ok(());
        }
        let Some(slot) = self.places.get(place.0 as usize) else {
            return Ok(());
        };
        if slot.meta.port {
            return Ok(());
        }
        Err(self.refuse(|| NetError::ClosedNet {
            net: self.display_name(),
            place: slot.meta.name.clone(),
            act,
        }))
    }

    /// [`admit`](Net::admit) for a method that has no `Result` to return
    /// it in: the refusal's message is the panic's.
    pub(crate) fn admit_or_panic(&self, place: PlaceId, act: &'static str) {
        if let Err(refusal) = self.admit(place, act) {
            panic!("{refusal}");
        }
    }

    /// The knob check: [`NetError::HeldByBody`] while a body holds this net
    /// and the net is [under the rule](Net::is_under_rule), open or closed,
    /// `act` saying what was asked ("set a parameter of").
    pub(crate) fn admit_knob(&self, act: &'static str) -> Result<(), NetError> {
        if !self.is_held() || !self.is_under_rule() {
            return Ok(());
        }
        Err(self.refuse(|| NetError::HeldByBody {
            net: self.display_name(),
            act,
        }))
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
