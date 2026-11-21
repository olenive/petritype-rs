//! A hike: one walker's energy, shared by two concerns, each a group.
//!
//! What it demonstrates: groups, a label on parts of a net that only
//! drawings read. Walking and eating both read and change one place, the
//! walker's `Energy`: `Fund Walk` spends it on the next leg of the trail
//! while there is enough for one, and `Eat` tops it up from the pack once
//! there is not. A net whose parts all change one shared place cannot be
//! split into subnets without moving those parts out to the net around
//! them, so it stays flat and is read by its parts instead: `Walk` and
//! `Eat` are groups, which a drawing frames under their names, and `Energy`
//! is in none.
//!
//! The engine never reads a group. The net fires here exactly as it would
//! with no groups declared, and its ids, its events and its saved form are
//! the same.

use petritype::{BuildError, MutateEdge, Net, NetBuilder, PlaceId, TakeEdge};

/// The walker's energy: one token, in a place both concerns change.
#[derive(Debug, Clone, PartialEq)]
struct Energy(u32);

/// One leg of the trail, by its number.
#[derive(Debug, Clone, PartialEq)]
struct Leg(u32);

/// One meal in the pack.
#[derive(Debug, Clone, PartialEq)]
struct Meal(&'static str);

/// What one leg costs.
const LEG: u32 = 2;

/// What one meal gives.
const MEAL: u32 = 3;

/// The net. `Walk`: `Trail -Fund Walk-> Walk Paid -Walk Leg-> Walked`, the
/// funding paid out of `Energy`. `Eat`: `Pack -Eat-> Eaten`, paid into it.
/// `Energy`, which both change, is in no group.
fn build() -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let energy = b.place::<Energy>("Energy");
    b.seed(energy, [Energy(3)]);

    let walk = b.group("Walk");
    let trail = b.place::<Leg>("Trail");
    let paid = b.place::<Leg>("Walk Paid");
    let walked = b.place::<Leg>("Walked");
    for place in [trail, paid, walked] {
        b.in_group(walk, place);
    }
    b.seed(trail, (1..=4).map(Leg));
    b.transition(
        "Fund Walk",
        (TakeEdge(trail), MutateEdge(energy)),
        (paid,),
        |leg: Leg, energy: &mut Energy| {
            energy.0 -= LEG;
            (leg,)
        },
    )
    .guard(move |net| level(net, energy.id()) >= LEG)
    .group(walk);
    b.transition("Walk Leg", (TakeEdge(paid),), (walked,), |leg: Leg| (leg,))
        .group(walk);

    let eat = b.group("Eat");
    let pack = b.place::<Meal>("Pack");
    let eaten = b.place::<Meal>("Eaten");
    b.in_group(eat, pack);
    b.in_group(eat, eaten);
    b.seed(pack, [Meal("Apple"), Meal("Bread")]);
    b.transition(
        "Eat",
        (TakeEdge(pack), MutateEdge(energy)),
        (eaten,),
        |meal: Meal, energy: &mut Energy| {
            energy.0 += MEAL;
            (meal,)
        },
    )
    .guard(move |net| level(net, energy.id()) < LEG)
    .group(eat);
    b.build()
}

/// How much energy the walker has: the one token in `place`.
fn level(net: &Net, place: PlaceId) -> u32 {
    net.tokens(place)
        .find_map(|(_, value)| value.downcast_ref::<Energy>())
        .map_or(0, |energy| energy.0)
}

/// The places and the transitions in `group`, by name, in id order; `None`
/// for the nodes in no group.
fn members<'a>(net: &'a Net, group: Option<&str>) -> (Vec<&'a str>, Vec<&'a str>) {
    let places = net
        .places()
        .filter(|(_, info)| info.group == group)
        .map(|(_, info)| info.name)
        .collect();
    let transitions = net
        .transitions()
        .filter(|(_, info)| info.group == group)
        .map(|(_, info)| info.name)
        .collect();
    (places, transitions)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut net = build()?;
    let energy = net.place_named("Energy").expect("a place");

    let groups: Vec<Option<&str>> = net.groups().map(Some).chain([None]).collect();
    for group in groups {
        let (places, transitions) = members(&net, group);
        println!(
            "{:<12} places {}; transitions {}",
            group.unwrap_or("(no group)"),
            places.join(", "),
            if transitions.is_empty() {
                "none".to_owned()
            } else {
                transitions.join(", ")
            },
        );
    }

    let mut count = 0;
    while let Some(fired) = net.step()? {
        count += 1;
        let (_, info) = net
            .transitions()
            .find(|(id, _)| *id == fired)
            .expect("a transition of this net");
        println!(
            "{count:>2}  {:<10} in {:<5} energy {}",
            info.name,
            info.group.unwrap_or("-"),
            level(&net, energy)
        );
    }
    let walked = net.place_named("Walked").expect("a place");
    println!(
        "walked {} legs, energy {} left",
        net.count(walked),
        level(&net, energy)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build, level, members};

    /// Every firing to the end, by name.
    fn journey(net: &mut petritype::Net) -> Vec<String> {
        std::iter::from_fn(|| {
            let fired = net.step().expect("nothing fails")?;
            net.transitions()
                .find(|(id, _)| *id == fired)
                .map(|(_, info)| info.name.to_owned())
        })
        .collect()
    }

    #[test]
    fn the_walker_eats_whenever_a_leg_costs_more_than_is_left() {
        let mut net = build().expect("the net builds");
        assert_eq!(
            journey(&mut net),
            [
                "Fund Walk",
                "Walk Leg",
                "Eat",
                "Fund Walk",
                "Fund Walk",
                "Walk Leg",
                "Walk Leg",
                "Eat",
                "Fund Walk",
                "Walk Leg",
            ]
        );
        let energy = net.place_named("Energy").unwrap();
        assert_eq!(level(&net, energy), 1);
        assert_eq!(net.count(net.place_named("Walked").unwrap()), 4);
        assert_eq!(net.count(net.place_named("Pack").unwrap()), 0);
    }

    #[test]
    fn each_concern_is_a_group_and_the_shared_place_is_in_none() {
        let net = build().expect("the net builds");
        assert_eq!(net.groups().collect::<Vec<_>>(), ["Walk", "Eat"]);
        assert_eq!(
            members(&net, Some("Walk")),
            (
                vec!["Trail", "Walk Paid", "Walked"],
                vec!["Fund Walk", "Walk Leg"]
            )
        );
        assert_eq!(
            members(&net, Some("Eat")),
            (vec!["Pack", "Eaten"], vec!["Eat"])
        );
        assert_eq!(members(&net, None), (vec!["Energy"], vec![]));
    }

    #[test]
    fn the_net_draws_each_group_as_a_dashed_frame() {
        let net = build().expect("the net builds");
        let text = petritype_dot::dot(&net);

        assert!(text.contains("\n  p0 [shape=oval"), "Energy is in no frame");
        assert!(text.contains(concat!(
            "  subgraph cluster_0 {\n",
            "    label=\"Walk\";\n",
            "    style=dashed;\n",
            "    p1 [shape=oval",
        )));
        assert!(text.contains(concat!(
            "  subgraph cluster_1 {\n",
            "    label=\"Eat\";\n",
            "    style=dashed;\n",
            "    p4 [shape=oval",
        )));
        assert_eq!(text.matches(" -> ").count(), 8);
    }
}
