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
//! The nets are in `parcel_runner/nets.rs`, which the transcript tests load
//! too: the parcel nets, and the protocol's conformance net `transit`, whose
//! vehicles carry nets of their own (`get_state {"nested_id": ...}` reads
//! one). This file is the whole of what an author writes besides them.

use std::process::ExitCode;

#[path = "parcel_runner/nets.rs"]
mod nets;

fn main() -> ExitCode {
    petritype_runner::main(nets::registry())
}
