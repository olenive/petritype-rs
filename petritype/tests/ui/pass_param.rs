//! A `Param` stands in the input tuple like an edge, anywhere in it, and the
//! body takes the parameter's type by value.

use petritype::{NetBuilder, Param, TakeEdge};

/// Deliberately not `Debug`: a parameter type needs no such bound, and a
/// secret should not have one.
#[derive(Clone)]
struct ApiToken(String);

#[derive(Clone)]
struct Scale(u32);

fn main() {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let pages = b.place::<String>("Pages");

    b.param(ApiToken(String::from("s3cret")));
    b.param(Scale(2));

    b.transition(
        "Fetch",
        (Param::<Scale>::new(), TakeEdge(urls), Param::<ApiToken>::new()),
        (pages,),
        |scale: Scale, url: String, token: ApiToken| {
            (format!("{url}?n={}&token={}", scale.0, token.0),)
        },
    );

    let net = b.build().unwrap();
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(info.inputs.len(), 1, "one edge; the parameters are not edges");
    assert_eq!(info.params.len(), 2);
}
