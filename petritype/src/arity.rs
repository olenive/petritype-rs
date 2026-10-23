//! One list of arities, used by every macro that implements a trait over
//! tuples.
//!
//! Input arity and output arity are both capped at 16, as in DESIGN §5: where
//! Bevy's `IntoSystem` and axum's handlers stop. The cap exists because each
//! arity costs an impl per trait, twelve impls in all across the edge, body
//! and output macros. It was 12 until 2026-10-15, when a game's busiest
//! transitions were heading for fourteen to sixteen inputs and thirteen to
//! fifteen outputs, and every way of staying under twelve hid something the
//! net was built to show (`dev-docs/TRANSITION_ARITY.md`). The four arities
//! that took it to 16 cost about a tenth of a second on a full recompile of
//! the crate (that document's "As built").

/// Invokes `$m` once per arity from 0 to 16, with one `(type, value, index)`
/// triple per slot.
///
/// The consumer matches `($(($t:ident, $r:ident, $i:tt)),*)` and uses what it
/// needs: the edge and place impls only want `$t`, the positional output impl
/// wants all three (`$t` for the place's token type, `$r` for the type of
/// the value in that slot, and `$i` for the slot's index, which is a literal
/// rather than a counter so that the generated code has nothing to increment).
macro_rules! for_each_arity {
    ($m:ident) => {
        $m!();
        $m!((A0, R0, 0));
        $m!((A0, R0, 0), (A1, R1, 1));
        $m!((A0, R0, 0), (A1, R1, 1), (A2, R2, 2));
        $m!((A0, R0, 0), (A1, R1, 1), (A2, R2, 2), (A3, R3, 3));
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10),
            (A11, R11, 11)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10),
            (A11, R11, 11),
            (A12, R12, 12)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10),
            (A11, R11, 11),
            (A12, R12, 12),
            (A13, R13, 13)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10),
            (A11, R11, 11),
            (A12, R12, 12),
            (A13, R13, 13),
            (A14, R14, 14)
        );
        $m!(
            (A0, R0, 0),
            (A1, R1, 1),
            (A2, R2, 2),
            (A3, R3, 3),
            (A4, R4, 4),
            (A5, R5, 5),
            (A6, R6, 6),
            (A7, R7, 7),
            (A8, R8, 8),
            (A9, R9, 9),
            (A10, R10, 10),
            (A11, R11, 11),
            (A12, R12, 12),
            (A13, R13, 13),
            (A14, R14, 14),
            (A15, R15, 15)
        );
    };
}

pub(crate) use for_each_arity;
