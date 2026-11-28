//! Stage three of a firing, [`distribute`](Net::distribute): the body
//! home first of all, then what it produced landed in its output places,
//! or its failure accounted for.

use crate::closed::DISTRIBUTE;
use crate::distribution::{DistributionError, FOREIGN_PLACE};
use crate::erased::{ErasedOutcome, OutputTarget};
use crate::error::{DistributeError, FiringFailed, NetError};
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
use crate::net::Net;
use crate::place::ErasedToken;

use super::{Completed, Report};

impl Net {
    // ----- the three stages -----------------------------------------------

    /// Stage three: puts the body back and writes what it produced.
    ///
    /// Atomic: every target place is resolved and checked before any token is
    /// written, so a firing that cannot be applied writes nothing at all. The
    /// body goes home first of all, on the failing path too, so that a
    /// failure never strands the transition in flight.
    ///
    /// Emits the [`Mutated`](Event::Mutated) events of this firing's
    /// [`MutateEdge`](crate::MutateEdge) and
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) inputs, then
    /// [`Event::Distributed`] naming the tokens that landed (or
    /// [`Event::Failed`] if the firing failed), and adds one to
    /// [`fired_count`](Net::fired_count) on success. A transition that
    /// [restores its inputs](crate::TransitionBuilder::restore_tokens_on_failure)
    /// has them back in their places before the `Failed` event is appended,
    /// so an observer that re-reads the marking when it sees that event sees
    /// them there.
    ///
    /// # Errors
    ///
    /// A [`DistributeError`], whose [`error`](DistributeError::error) is
    /// [`NetError::HeldByBody`] when a body holds this net, as for
    /// [`fire`](Net::fire) (a body cannot have launched one of its firings,
    /// so this is misuse); [`NetError::ForeignFiring`] if the `Completed`
    /// was launched by another net, [`NetError::NotInFlight`] if this net's
    /// transition is not in flight, and [`NetError::FiringFailed`] if the
    /// body returned `Err` or panicked, or named a place it may not write
    /// to. The net is untouched in the first three cases, and the
    /// `Completed` comes back in the error
    /// ([`into_completed`](DistributeError::into_completed), since 0.4.0;
    /// until then it was dropped with its body), so that it can still land
    /// in the net that launched it. In the fourth the net is consistent,
    /// with the firing's inputs gone or restored, and the body is home:
    /// nothing comes back.
    ///
    /// `?` turns the error into a [`NetError`], as this returned until
    /// 0.4.0, dropping a `Completed` it carries.
    ///
    /// # Panics
    ///
    /// Not for a panicking body, whose panic arrives as a failure. A panic
    /// of the net's own code while the outputs land, a key extractor of a
    /// place a token lands in or the [`Nested::nets_mut`](crate::Nested::nets_mut)
    /// of a token that carries nets, goes on to the caller. The body is home
    /// by then, so the transition is not left in flight; the tokens that
    /// had landed stay where they are, and the rest of the outputs are lost.
    pub fn distribute(&mut self, completed: Completed) -> Result<(), DistributeError> {
        if let Err(refused) = self.admit_knob(DISTRIBUTE) {
            return Err(DistributeError::handed_back(refused, completed));
        }
        self.distribute_for_driver(completed)
    }

    /// [`distribute`](Net::distribute) without the check: the concurrent
    /// driver's, for the firings it launched with
    /// [`launch_for_driver`](Net::launch_for_driver).
    pub(crate) fn distribute_for_driver(
        &mut self,
        completed: Completed,
    ) -> Result<(), DistributeError> {
        // The stamp first, exactly as a `PlaceRef` is checked before it is
        // used as an index: a `TransitionId` from another net would otherwise
        // name whatever transition sits at that index here.
        if completed.net != self.id {
            let foreign = NetError::ForeignFiring {
                transition: completed.transition,
                from: completed.net,
            };
            return Err(DistributeError::handed_back(foreign, completed));
        }
        if !self.is_in_flight(completed.transition) {
            let grounded = NetError::NotInFlight {
                transition: completed.transition,
                name: self.transition_name(completed.transition),
            };
            return Err(DistributeError::handed_back(grounded, completed));
        }
        self.distribute_completed(completed)
            .map_err(DistributeError::failed)
    }

    // ----- the stages, written once ---------------------------------------

    /// Stage three, with the stamp and the in-flight slot already checked.
    pub(super) fn distribute_completed(
        &mut self,
        completed: Completed,
    ) -> Result<(), FiringFailed> {
        let Completed {
            transition,
            consumed,
            mutated,
            restore,
            report,
            body,
            outcome,
            hold,
            ..
        } = completed;
        // Home before anything else can go wrong, on the failing path too:
        // DESIGN §7 promises a consistent net after a failure, and a body
        // left out of its slot would strand the transition in flight. A body
        // that panicked comes home as well, carrying whatever state the panic
        // left it in.
        self.transitions[transition.0 as usize].body = Some(body);
        let landed =
            self.distribute_outcome(transition, outcome, consumed, mutated, restore, report);
        // The firing has ended, and with it the mark on every net it handed
        // its body, wherever the body put them (DESIGN §6.9).
        drop(hold);
        landed
    }

    /// Places what the body produced, or accounts for its failure.
    ///
    /// `consumed` and `mutated` are stage one's records, in edge order, and
    /// `restore` the clones it took if this transition restores its inputs.
    fn distribute_outcome(
        &mut self,
        transition: TransitionId,
        outcome: ErasedOutcome,
        consumed: Vec<(PlaceId, TokenId)>,
        mutated: Vec<(PlaceId, TokenId)>,
        restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
        report: Report,
    ) -> Result<(), FiringFailed> {
        let produced = match outcome {
            ErasedOutcome::Ok(tokens) => tokens,
            ErasedOutcome::Err(error) => {
                // The tokens go back first, so that the marking is already
                // whole when the `Failed` event an observer reads arrives.
                let restored = self.restore_consumed(restore);
                // A body that mutated a token through a `MutateEdge` and then
                // failed has still mutated it: the change went into the
                // place as it was made, and there is nothing to roll back to,
                // so the mutations are reported before the failure.
                self.emit_mutations(&mutated);
                self.emit(Event::Failed {
                    transition,
                    consumed,
                    restored,
                    error: error.to_string(),
                });
                return Err(FiringFailed {
                    transition,
                    name: self.transitions[transition.0 as usize].name.clone(),
                    error,
                    restored,
                    path: Vec::new(),
                    nested_ids: Vec::new(),
                });
            }
        };

        // Atomic, as DESIGN §6.2 asks: every target is resolved and checked
        // in this loop, and nothing is written until all of them have been.
        // Two of the three output forms have nothing left to check. A
        // positional slot's index came from the body's return type, which was
        // matched against the output places at compile time, and a routed
        // payload's from the table resolved when the transition was declared,
        // which `NetBuilder::build` refused the net without, so their target
        // is an index and the bound stays a `debug_assert!`. A
        // [`Distribution`](crate::Distribution) names its places itself, from
        // data, so its targets are checked here and a bad one fails the whole
        // firing before any of its siblings lands.
        let mut resolved = Vec::with_capacity(produced.len());
        for (target, token) in produced {
            let place = match target {
                OutputTarget::Index(index) => {
                    debug_assert!(
                        index < self.transitions[transition.0 as usize].outputs.len(),
                        "output index {index} is past the outputs of transition {transition:?}"
                    );
                    self.transitions[transition.0 as usize].outputs[index]
                }
                OutputTarget::Place(place, net) => {
                    match self.distributed_target(transition, place, net, &token) {
                        Ok(place) => place,
                        Err(error) => {
                            // A firing that cannot be applied fails like any
                            // other (DESIGN §4.5): nothing written, the
                            // consumed tokens lost or restored, the net
                            // consistent.
                            let restored = self.restore_consumed(restore);
                            self.emit_mutations(&mutated);
                            self.emit(Event::Failed {
                                transition,
                                consumed,
                                restored,
                                error: error.to_string(),
                            });
                            return Err(FiringFailed {
                                transition,
                                name: self.transitions[transition.0 as usize].name.clone(),
                                error: Box::new(error),
                                restored,
                                path: Vec::new(),
                                nested_ids: Vec::new(),
                            });
                        }
                    }
                }
            };
            resolved.push((place, token));
        }

        // The firing stood, so the clones a restoring transition took are
        // dropped here, unread: they exist only for the failing path.
        drop(restore);

        let mut landed = Vec::with_capacity(resolved.len());
        for (place, token) in resolved {
            let token = self.push_produced(place, token);
            landed.push((place, token));
        }

        self.emit_mutations(&mutated);
        // Rule 4 of DESIGN §6.6: a nested token that landed speaks under its
        // new id, after the event that announces the id. Only those are kept
        // for the loop below, so a net without nesting allocates nothing.
        let arrivals: Vec<(PlaceId, TokenId)> = landed
            .iter()
            .copied()
            .filter(|(place, _)| self.places[place.0 as usize].meta.nested.is_some())
            .collect();
        self.emit(match report {
            // The sequential path: consumed and produced together, because
            // nothing happened between them.
            Report::Fired => Event::Fired {
                transition,
                consumed,
                produced: landed,
            },
            // The staged path already said what was consumed, at launch.
            Report::Staged => Event::Distributed {
                transition,
                produced: landed,
            },
        });
        for (place, token) in arrivals {
            self.lift(place, token);
        }
        self.fired_count += 1;
        Ok(())
    }

    /// Checks one pair of a [`Distribution`](crate::Distribution) against this
    /// transition, and answers with the place to write to.
    ///
    /// Three things are asked, in the order they can go wrong:
    ///
    /// - the handle was stamped with *this* net, so that a
    ///   [`PlaceRef`](crate::PlaceRef) from another net is caught rather than
    ///   read as an index into this one;
    /// - the place is one this transition declared as an output, because the
    ///   output tuple is what a diagram draws and a firing may not write
    ///   outside it;
    /// - the token really is of that place's type. The typed API has already
    ///   guaranteed this ([`Distribution::to`](crate::Distribution::to) takes
    ///   a `PlaceRef<T>` and a `T`), so it is a cheap defence against a
    ///   foreign handle whose index happens to coincide, and not a check the
    ///   author is meant to rely on.
    fn distributed_target(
        &self,
        transition: TransitionId,
        place: PlaceId,
        net: NetId,
        token: &ErasedToken,
    ) -> Result<PlaceId, DistributionError> {
        let slot = &self.transitions[transition.0 as usize];
        if net != self.id || !slot.outputs.contains(&place) {
            return Err(DistributionError::NotAnOutput {
                transition,
                transition_name: slot.name.clone(),
                place,
                // A handle stamped with another net names nothing here, and
                // this net's place at that index is a different place
                // entirely, so it is not named.
                name: if net == self.id {
                    self.place_name(place)
                } else {
                    String::from(FOREIGN_PLACE)
                },
            });
        }
        let meta = &self.places[place.0 as usize].meta;
        // `(**token)` and not `token`: a `Box<dyn Any + ..>` is itself `Any`,
        // so asking the box would answer with the box's own type.
        if (**token).type_id() != meta.type_id {
            return Err(DistributionError::WrongType {
                transition,
                transition_name: slot.name.clone(),
                place,
                name: meta.name.clone(),
                expected: meta.type_name,
            });
        }
        Ok(place)
    }

    /// Puts a failed firing's consumed tokens back, and says whether there
    /// was a promise to keep.
    ///
    /// `None` is a transition that does not restore, and the answer is false:
    /// its inputs are gone. `Some` is one that does, and the answer is true
    /// however many tokens are in the list: a restoring transition that
    /// consumes nothing (only snapshot, survey, mutating and read edges)
    /// restores an empty list, and says so.
    ///
    /// The clones go in at the *front*, in the order they were taken, which
    /// is what "the firing is undone" means for a FIFO place: the next firing
    /// sees exactly what this one saw. They keep their original
    /// [`TokenId`]s (the ids were never retired, because the firing they
    /// belonged to never completed), and nothing is minted, so no
    /// [`Added`](Event::Added) is emitted either; the
    /// [`Failed`](Event::Failed) event's `restored` flag is the whole report.
    ///
    /// The list is walked backwards so that each place's tokens end up in
    /// front in the order they left it: the last one taken is pushed first
    /// and ends up behind the ones taken before it. Two take edges on one
    /// place (since 0.4.0) put theirs back in edge order, the first edge's
    /// token at the front, as Python petritype does, whatever order the
    /// tokens stood in.
    fn restore_consumed(&mut self, restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>) -> bool {
        let Some(restore) = restore else {
            return false;
        };
        for (place, token, value) in restore.into_iter().rev() {
            // A token taken from a port goes back into the port.
            let (holder, at) = self.holder_mut(place);
            let slot = &mut holder.places[at.0 as usize];
            let front = slot.tokens.front().map(|(front, _)| *front);
            holder.index.pushed_front(at, token, front);
            holder.index.enter(&slot.meta, at, token, &*value);
            slot.tokens.push_front((token, value));
        }
        true
    }

    /// One [`Event::Mutated`] per mutating edge, in edge order, each
    /// preceded by whatever the body did inside the token's inner nets if it
    /// is a nested one (DESIGN §6.6, rule 2).
    ///
    /// Lifted here rather than when the body returned because the token never
    /// left its place, so here is where it can be found, and here is where
    /// the order the rule promises is written.
    fn emit_mutations(&mut self, mutated: &[(PlaceId, TokenId)]) {
        for &(place, token) in mutated {
            self.lift(place, token);
            self.emit(Event::Mutated { place, token });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::TakeEdge;
    use crate::erased::ErasedInputs;
    use crate::{Event, NetBuilder};

    /// [`NetError::NotInFlight`] is a guard the safe API cannot reach: a
    /// [`Completed`] is minted only by [`Net::launch`], which leaves its
    /// transition in flight, and [`Net::distribute`] consumes the one it is
    /// given, so there is never a second one to hand over. It is exercised
    /// from in here, where a `Completed` for a transition whose body is back
    /// home can be built by hand.
    #[test]
    fn distributing_a_transition_that_is_not_in_flight_is_refused() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [1]);
        let copy = b
            .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
            .id();
        let mut net = b.build().expect("a one-transition net");

        let firing = net.launch(copy).expect("fireable");
        net.distribute(firing.run()).expect("in flight");
        assert!(net.in_flight().is_empty(), "the body is back in its slot");

        let ghost = Completed {
            net: net.id(),
            transition: copy,
            consumed: Vec::new(),
            mutated: Vec::new(),
            restore: None,
            report: Report::Staged,
            body: Box::new(|_: ErasedInputs<'_>| ErasedOutcome::Ok(Vec::new())),
            outcome: ErasedOutcome::Ok(Vec::new()),
            hold: None,
        };
        let refused = net
            .distribute(ghost)
            .expect_err("the body is home, so nothing is in flight");
        assert!(refused.completed().is_some(), "the ghost is handed back");
        let error = NetError::from(refused);
        assert!(
            matches!(error, NetError::NotInFlight { transition, .. } if transition == copy),
            "{error:?}",
        );
        assert_eq!(
            error.to_string(),
            format!("transition \"Copy\" ({copy:?}) is not in flight"),
            "the name is what a reader needs once the error has left the net",
        );

        assert_eq!(net.fired_count(), 1, "the rejected firing changed nothing");
        assert_eq!(net.count(out.id()), 1);
        let events: Vec<Event> = net.drain_events().collect();
        assert!(
            matches!(
                events[..],
                [Event::Launched { .. }, Event::Distributed { .. }]
            ),
            "the one firing that did happen, and nothing since: {events:?}"
        );
    }

    /// The pair of [`DistributionError`] messages, taken straight from the
    /// check that produces them.
    ///
    /// [`WrongType`](DistributionError::WrongType) cannot be reached through
    /// the safe API at all ([`Distribution::to`](crate::Distribution::to)
    /// takes a `PlaceRef<T>` and a `T`), and the foreign half of
    /// [`NotAnOutput`](DistributionError::NotAnOutput) needs a second net's
    /// handle, so both are asked of `distributed_target` directly.
    #[test]
    fn a_refused_distribution_names_the_transition_and_the_place() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let uploads = b.place::<u32>("Uploads");
        let elsewhere = b.place::<u32>("Elsewhere");
        b.seed(inbox, [1]);
        let weigh = b
            .transition("Weigh", (TakeEdge(inbox),), (uploads,), |n: u32| (n,))
            .id();
        let net = b.build().expect("a one-transition net");

        // A place of this net that this transition may not write to.
        let error = net
            .distributed_target(weigh, elsewhere.id(), net.id(), &erased(1u32))
            .expect_err("Elsewhere is not an output of Weigh");
        assert_eq!(
            error.to_string(),
            format!(
                "transition \"Weigh\" ({weigh:?}) distributed a token to place \"Elsewhere\" \
                 ({:?}), which is not one of its outputs",
                elsewhere.id(),
            ),
        );

        // A handle from another net, whose places this one cannot name.
        let error = net
            .distributed_target(weigh, uploads.id(), NetId(u64::MAX), &erased(1u32))
            .expect_err("the stamp is checked before the index is used");
        assert!(
            error.to_string().contains(FOREIGN_PLACE),
            "this net's place at that index is a different place: {error}",
        );

        // An output place of this transition, and a token it cannot hold.
        let error = net
            .distributed_target(weigh, uploads.id(), net.id(), &erased(String::from("x")))
            .expect_err("Uploads holds u32");
        assert_eq!(
            error.to_string(),
            format!(
                "transition \"Weigh\" ({weigh:?}) distributed a token to place \"Uploads\" \
                 ({:?}), which holds u32",
                uploads.id(),
            ),
        );
    }

    /// One token with its type forgotten, as a firing's outputs reach
    /// [`Net::distributed_target`].
    fn erased<T: Send + Sync + 'static>(token: T) -> ErasedToken {
        Box::new(token)
    }
}
