//! A small world of two levels, each a subnet place built from one template.
//!
//! What it demonstrates: a place that holds a net of its own. `Level 1` and
//! `Level 2` are subnet places of the world, each holding a net that the
//! `Level` template builds. The world reaches a level only through its
//! ports, the places the level's definition marks as such: a hero enters a
//! level by its `Lobby` and leaves by its `Exit Hall`, and the level's
//! `Arena` is out of the world's reach. The world's edges on a port are
//! ordinary edges, so `Climb` takes a hero from one level's exit hall and
//! puts it into the next level's lobby.
//!
//! Under the default stepping rule, inner first, a level moves its heroes
//! until it is at rest before the world fires again, and one `step_deep`
//! fires one transition somewhere in the whole world. Part of the way, the
//! world is saved and restored: each level is written inside the world's
//! marking, a list of one under its place, and the restored world carries
//! on to the same end.

use std::error::Error;

use petritype::serde_json;
use petritype::{
    BuildError, DeepStep, Event, Net, NetBuilder, Params, PlaceId, TakeEdge, Templates,
};
use serde::{Deserialize, Serialize};

/// The token: a hero, and how many levels it has cleared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Hero {
    name: String,
    cleared: u32,
}

/// The `Level` template: `Lobby -Fight-> Arena -Win-> Exit Hall`. The lobby
/// and the exit hall are its ports; the arena is its own.
fn level(_: &Params) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let arena = b.place::<Hero>("Arena");
    let exit = b.place::<Hero>("Exit Hall");
    for place in [lobby, arena, exit] {
        b.serializable(place);
    }
    b.port(lobby);
    b.port(exit);
    b.transition("Fight", (TakeEdge(lobby),), (arena,), |hero: Hero| (hero,));
    b.transition("Win", (TakeEdge(arena),), (exit,), |mut hero: Hero| {
        hero.cleared += 1;
        (hero,)
    });
    b.build()
}

/// The `World` template: heroes at the `Door` enter `Level 1` by its lobby,
/// climb from its exit hall into the lobby of `Level 2`, and leave its exit
/// hall for `Outside`. Both levels are built by the `Level` template, so the
/// world can be saved with them inside.
fn world(levels: &Templates) -> Result<Net, Box<dyn Error + Send + Sync>> {
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let outside = b.place::<Hero>("Outside");
    b.serializable(door);
    b.serializable(outside);
    let level_1 = b.subnet_place("Level 1", levels.build("Level", &Params::new())?);
    let level_2 = b.subnet_place("Level 2", levels.build("Level", &Params::new())?);
    let lobby_1 = b.port_of::<Hero>(level_1, "Lobby");
    let exit_1 = b.port_of::<Hero>(level_1, "Exit Hall");
    let lobby_2 = b.port_of::<Hero>(level_2, "Lobby");
    let exit_2 = b.port_of::<Hero>(level_2, "Exit Hall");
    b.seed(
        door,
        ["Ada", "Bram"].map(|name| Hero {
            name: name.to_owned(),
            cleared: 0,
        }),
    );
    b.transition("Enter", (TakeEdge(door),), (lobby_1,), |hero: Hero| (hero,));
    b.transition("Climb", (TakeEdge(exit_1),), (lobby_2,), |hero: Hero| {
        (hero,)
    });
    b.transition("Leave", (TakeEdge(exit_2),), (outside,), |hero: Hero| {
        (hero,)
    });
    Ok(b.build()?)
}

/// A registry holding both templates: what a world is built and restored
/// from.
fn templates() -> Templates {
    let mut levels = Templates::new();
    levels.register("Level", level);
    let mut templates = levels.clone();
    templates.register("World", move |_: &Params| world(&levels));
    templates
}

/// Where a deep step fired, as the subnet places it went through and the
/// transition: `"Level 1 / Fight"`.
fn firing(net: &Net, step: &DeepStep) -> String {
    match step {
        DeepStep::Subnet { place, step, .. } => {
            let subnet = net.subnet(*place).expect("a subnet place");
            format!("{} / {}", place_name(net, *place), firing(subnet, step))
        }
        step => net
            .transitions()
            .find(|(id, _)| *id == step.transition())
            .map(|(_, info)| info.name.to_owned())
            .expect("a transition of this net"),
    }
}

fn place_name(net: &Net, place: PlaceId) -> &str {
    net.places()
        .find(|(id, _)| *id == place)
        .map(|(_, info)| info.name)
        .expect("a place of this net")
}

/// Who a firing moved: the hero it produced, read in the net that fired,
/// down through the subnet places its event was lifted through.
fn moved(net: &Net, event: &Event) -> Option<String> {
    match event {
        Event::Fired { produced, .. } => {
            let (place, token) = produced.first()?;
            net.token::<Hero>(*place, *token)
                .map(|hero| hero.name.clone())
        }
        Event::Subnet { place, event, .. } => moved(net.subnet(*place)?, event),
        _ => None,
    }
}

/// One deep step, printed; `false` once the whole world is at rest.
fn step(world: &mut Net, count: &mut usize) -> Result<bool, Box<dyn Error>> {
    let Some(step) = world.step_deep()? else {
        return Ok(false);
    };
    *count += 1;
    let events: Vec<Event> = world.drain_events().collect();
    let who = events.iter().find_map(|event| moved(world, event));
    println!(
        "{count:>2}  {:<18} {}",
        firing(world, &step),
        who.unwrap_or_default()
    );
    Ok(true)
}

/// The heroes in a place, oldest first.
fn heroes(net: &Net, place: PlaceId) -> Vec<Hero> {
    net.tokens(place)
        .filter_map(|(_, value)| value.downcast_ref::<Hero>().cloned())
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let templates = templates();
    let mut world = templates.build("World", &Params::new())?;
    let mut count = 0;
    while count < 7 && step(&mut world, &mut count)? {}

    // Part of the way: Ada is in the lobby of level 2, and Bram has cleared
    // level 1 and waits at its exit hall, a port, so the level's entry in
    // the saved marking holds him and the world's own entries do not.
    let saved = world.to_json()?;
    println!(
        "saved after {count} firings, Level 1 as {}",
        serde_json::to_string(&saved["marking"]["Level 1"])?
    );
    let mut world = templates.restore(saved)?;
    println!("restored; on from there");
    while step(&mut world, &mut count)? {}

    let outside = world.place_named("Outside").expect("a place");
    for hero in heroes(&world, outside) {
        println!("Outside: {} cleared {} levels", hero.name, hero.cleared);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Hero, firing, heroes, templates};
    use petritype::serde_json::{self, json};
    use petritype::{Net, Params};

    fn world() -> Net {
        templates().build("World", &Params::new()).expect("a world")
    }

    /// Every firing to the end, by where it fired.
    fn journey(world: &mut Net) -> Vec<String> {
        std::iter::from_fn(|| {
            let step = world.step_deep().expect("nothing fails")?;
            Some(firing(world, &step))
        })
        .collect()
    }

    #[test]
    fn each_level_comes_to_rest_before_the_world_fires_again() {
        let mut world = world();
        assert_eq!(
            journey(&mut world),
            [
                "Enter",
                "Level 1 / Fight",
                "Level 1 / Win",
                "Enter",
                "Level 1 / Fight",
                "Level 1 / Win",
                "Climb",
                "Level 2 / Fight",
                "Level 2 / Win",
                "Climb",
                "Level 2 / Fight",
                "Level 2 / Win",
                "Leave",
                "Leave",
            ]
        );
        assert!(world.is_quiescent_deep());
    }

    #[test]
    fn every_hero_clears_both_levels_and_comes_out() {
        let mut world = world();
        journey(&mut world);
        let outside = world.place_named("Outside").unwrap();
        assert_eq!(
            heroes(&world, outside),
            ["Ada", "Bram"].map(|name| Hero {
                name: name.to_owned(),
                cleared: 2
            })
        );
        for level in ["Level 1", "Level 2"] {
            let level = world.subnet(world.place_named(level).unwrap()).unwrap();
            assert!(level.places().all(|(_, place)| place.count == 0));
            assert_eq!(level.fired_count(), 4);
        }
    }

    #[test]
    fn a_level_is_saved_inside_the_world_as_a_list_of_one() {
        let mut world = world();
        world.step_deep().unwrap();
        assert_eq!(
            serde_json::to_value(world.export_marking().unwrap()).unwrap(),
            json!({
                "Door": [{"name": "Bram", "cleared": 0}],
                "Outside": [],
                "Level 1": [{"template": "Level",
                             "marking": {"Arena": [], "Exit Hall": [],
                                         "Lobby": [{"name": "Ada", "cleared": 0}]},
                             "params": {}, "enabled": {"Fight": true, "Win": true}}],
                "Level 2": [{"template": "Level",
                             "marking": {"Arena": [], "Exit Hall": [], "Lobby": []},
                             "params": {}, "enabled": {"Fight": true, "Win": true}}],
            })
        );
    }

    #[test]
    fn a_world_saved_part_of_the_way_ends_as_a_run_straight_through_does() {
        let mut straight = world();
        let mut saved = world();
        let first: Vec<String> = (0..7)
            .map(|_| {
                let step = saved.step_deep().unwrap().unwrap();
                firing(&saved, &step)
            })
            .collect();
        let mut restored = templates().restore(saved.to_json().unwrap()).unwrap();
        let rest = journey(&mut restored);
        assert_eq!([first, rest].concat(), journey(&mut straight));
        assert_eq!(
            restored.export_marking().unwrap(),
            straight.export_marking().unwrap()
        );
    }
}
