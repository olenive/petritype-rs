//! Pair strings with the numbers that are their lengths.
//!
//! A port of the Python `examples/toy/match_up_tokens/01_match_lengths.py`.
//!
//! What it demonstrates: a transition that reads *every* token of two places
//! at once ([`TakeAllEdge`]), decides one match, and returns the rest of both
//! lists to where they came from — positional outputs, one slot per place,
//! with a `Vec` slot meaning "as many tokens as it holds" and an `Option`
//! slot meaning "maybe nothing came of it".
//!
//! Without a [guard](petritype::TransitionBuilder::guard) that would be a
//! futile cycle: the transition is always fireable while either place holds
//! anything, so it would keep taking the leftovers out and putting them back.
//! The guard asks whether a match is still possible, so the run ends by
//! itself with the unmatched tokens still visible in their places.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Net, NetBuilder, PlaceRef, TakeAllEdge};

/// The net and the handles the example reads back.
struct Matching {
    net: Net,
    strings: PlaceRef<String>,
    lengths: PlaceRef<usize>,
    matched: PlaceRef<(String, usize)>,
}

/// Pulls one matching pair out of the two lists, or neither if there is none.
///
/// The domain function, and it knows nothing about places: it takes what it
/// was given and hands back what is left.
fn match_one_string_to_one_length(
    mut strings: Vec<String>,
    mut lengths: Vec<usize>,
) -> (Vec<String>, Vec<usize>, Option<(String, usize)>) {
    let found = strings.iter().enumerate().find_map(|(i, s)| {
        lengths
            .iter()
            .position(|length| s.chars().count() == *length)
            .map(|j| (i, j))
    });
    match found {
        Some((i, j)) => {
            let s = strings.remove(i);
            let length = lengths.remove(j);
            (strings, lengths, Some((s, length)))
        }
        None => (strings, lengths, None),
    }
}

/// `Some Strings` and `Some Lengths` feeding one transition that writes back
/// to both of them and to `Matched Pair`.
fn build() -> Result<Matching, BuildError> {
    let mut b = NetBuilder::new();
    let strings = b.place::<String>("Some Strings");
    let lengths = b.place::<usize>("Some Lengths");
    let matched = b.place::<(String, usize)>("Matched Pair");
    b.seed(strings, ["a", "ab", "abc", "abcd"].map(str::to_owned));
    b.seed(lengths, [2, 3, 4, 5, 99]);

    b.transition(
        "Match Lengths",
        (TakeAllEdge(strings), TakeAllEdge(lengths)),
        (strings, lengths, matched),
        match_one_string_to_one_length,
    )
    .guard(move |net: &Net| {
        let sizes: Vec<usize> = net
            .tokens(strings.id())
            .filter_map(|(_, value)| value.downcast_ref::<String>())
            .map(|s| s.chars().count())
            .collect();
        net.tokens(lengths.id())
            .filter_map(|(_, value)| value.downcast_ref::<usize>())
            .any(|length| sizes.contains(length))
    });

    Ok(Matching {
        net: b.build()?,
        strings,
        lengths,
        matched,
    })
}

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .filter_map(|(_, value)| value.downcast_ref::<T>())
        .cloned()
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut matching = build()?;
    let summary = drivers::run(
        &mut matching.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    println!("matched:   {:?}", values(&matching.net, matching.matched));
    println!(
        "leftover strings: {:?}",
        values(&matching.net, matching.strings)
    );
    println!(
        "leftover lengths: {:?}",
        values(&matching.net, matching.lengths)
    );
    println!("{} matches made", summary.fired_count);
    println!("{}", petritype_dot::dot(&matching.net));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build, match_one_string_to_one_length, values};
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    /// Builds the net and runs it until no match is possible.
    fn run() -> super::Matching {
        let mut matching = build().expect("the net builds");
        let summary = drivers::run(
            &mut matching.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("nothing fails");
        assert!(summary.quiesced, "the guard stops the futile cycle");
        assert_eq!(summary.fired_count, 3);
        matching
    }

    #[test]
    fn every_possible_pair_is_matched_in_order() {
        let matching = run();

        assert_eq!(
            values(&matching.net, matching.matched),
            [
                (String::from("ab"), 2),
                (String::from("abc"), 3),
                (String::from("abcd"), 4),
            ],
        );
    }

    #[test]
    fn the_unmatched_tokens_stay_in_their_places() {
        let matching = run();

        assert_eq!(values(&matching.net, matching.strings), [String::from("a")]);
        assert_eq!(values(&matching.net, matching.lengths), [5, 99]);
    }

    #[test]
    fn the_domain_function_leaves_the_lists_alone_when_nothing_matches() {
        let (strings, lengths, pair) =
            match_one_string_to_one_length(vec![String::from("a")], vec![5, 99]);

        assert_eq!(strings, [String::from("a")]);
        assert_eq!(lengths, [5, 99]);
        assert_eq!(pair, None);
    }

    #[test]
    fn both_inputs_are_drawn_as_batch_edges() {
        let matching = build().expect("the net builds");
        let text = petritype_dot::dot(&matching.net);

        assert_eq!(text.matches(r#"label="*""#).count(), 2);
        // Three outputs, so each one carries the slot it is written from.
        for slot in ["0", "1", "2"] {
            assert!(text.contains(&format!(r#"label="{slot}""#)));
        }
    }
}
