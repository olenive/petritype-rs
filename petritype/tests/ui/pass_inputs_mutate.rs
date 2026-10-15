//! A struct of inputs may borrow tokens where they lie: a `&'a mut T` field
//! is filled by a `MutateEdge` or a `MutateWhereEdge`, the struct has the
//! one lifetime `'a`, and the body takes it as `Tally<'_>`.

use petritype::{Filter, Inputs, MutateEdge, MutateWhereEdge, NetBuilder, TakeEdge};

#[derive(Inputs)]
struct Tally<'a> {
    vote: String,
    total: &'a mut u32,
    leader: &'a mut (String, u32),
}

fn tally(t: Tally<'_>) -> (String,) {
    *t.total += 1;
    t.leader.1 += 1;
    (t.vote,)
}

fn main() {
    let mut b = NetBuilder::new();
    let votes = b.place::<String>("Votes");
    let total = b.place::<u32>("Total");
    let candidates = b.place::<(String, u32)>("Candidates");
    let counted = b.place::<String>("Counted");
    b.keys(candidates, ["name"], |(name, _): &(String, u32)| [name.as_str().into()]);
    b.seed(votes, [String::from("Ada")]);
    b.seed(total, [0]);
    b.seed(candidates, [(String::from("Bo"), 0), (String::from("Ada"), 0)]);

    b.transition(
        "Count",
        (TallyEdges {
            vote: TakeEdge(votes),
            total: MutateEdge(total),
            leader: MutateWhereEdge(candidates, Filter::new().key_in("name", ["Ada"])),
        },),
        (counted,),
        tally,
    );
    // A closure takes the struct with its lifetime elided, as a function does.
    let more = b.place::<String>("More");
    let again = b.place::<String>("Again");
    b.transition(
        "Recount",
        (TallyEdges {
            vote: TakeEdge(more),
            total: MutateEdge(total),
            leader: MutateWhereEdge(candidates, Filter::new()),
        },),
        (again,),
        |t: Tally<'_>| {
            *t.total += 10;
            (t.vote,)
        },
    );

    let mut net = b.build().unwrap();
    net.step().unwrap();
    let (token, _) = net.tokens(total.id()).next().unwrap();
    assert_eq!(net.token::<u32>(total.id(), token), Some(&1));
    let ada: Vec<u32> = net
        .tokens(candidates.id())
        .filter_map(|(token, _)| net.token::<(String, u32)>(candidates.id(), token))
        .filter(|(name, _)| name == "Ada")
        .map(|(_, count)| *count)
        .collect();
    assert_eq!(ada, [1], "the filter chose Ada, in place");
}
