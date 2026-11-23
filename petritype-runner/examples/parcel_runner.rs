//! A runner binary: the parcel nets served over the net protocol.
//!
//! ```text
//! cargo run -p petritype-runner --example parcel_runner -- --describe
//! cargo run -p petritype-runner --example parcel_runner
//! {"id":"1","method":"load","params":{"net":"parcel-fleet","params":{"trucks":3}}}
//! {"id":"2","method":"step","params":{}}
//! {"id":"3","method":"get_state","params":{}}
//! {"id":"4","method":"shutdown","params":{}}
//! ```
//!
//! With `--net <name>` it carries that one net alone, so that `describe`
//! lists it only, as a shared transcript of PROTOCOL.md §11 recorded over
//! that net expects; a name it does not carry is refused with the names it
//! does:
//!
//! ```text
//! cargo run -p petritype-runner --example parcel_runner -- --net odd-names --describe
//! cargo run -p petritype-runner --example parcel_runner -- --net workshop
//! ```
//!
//! The nets are in `parcel_runner/nets.rs`, which the transcript tests load
//! too: the parcel nets, and the protocol's conformance nets `transit`, whose
//! vehicles carry nets of their own (`get_state {"nested_id": ...}` reads
//! one), `wildlife`, whose species carry keys (`query {"filter":
//! {"clauses": [{"key": "class", "values": ["Bird"]}]}}` finds the birds),
//! `shop`, whose one transition reads a place that may be empty through a
//! `survey` edge, and `traffic-light`, whose `Go` takes every car waiting
//! while the light is green through a `drain` edge, and whose own controls
//! add a car and change the light (`press {"control": "car", "value":
//! "Taxi"}`). It carries the rest of PROTOCOL.md §11's conformance nets too:
//! `stepping-rules`, `levels`, `groups`, `workshop` (whose cracked pots
//! fail their firings, and whose load parameter `restore` gives them back),
//! `cellar`, `harbour`, `odd-names`, `couriers` (whose orders and calls
//! each choose the courier they name), `roll-call` (whose roll call reads
//! the squads in the barracks without taking them) and `trays` (whose unit,
//! a net with two ports, is handed an order and gives up its shots). This file is the whole of what an
//! author writes besides them.

use std::process::ExitCode;

#[path = "parcel_runner/nets.rs"]
mod nets;

fn main() -> ExitCode {
    petritype_runner::main(nets::registry())
}
