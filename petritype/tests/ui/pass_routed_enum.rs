//! The DESIGN §4.5 web-scrape shape: a routed enum, whose variants pick the
//! output place by the type they carry rather than by position.

use petritype::{NetBuilder, Routed, TakeEdge};

#[derive(Debug, Clone)]
struct ScrapedData(String);

#[derive(Debug, Clone)]
struct SuccessfulScrape(String);

#[derive(Debug, Clone)]
struct UncertainScrapeResult(String);

#[derive(Routed)]
enum Outcome {
    Ok(SuccessfulScrape),
    Unsure(UncertainScrapeResult),
    Nothing,
}

fn main() {
    let mut b = NetBuilder::new();
    let scraped = b.place::<ScrapedData>("Scraped");
    let successes = b.place::<SuccessfulScrape>("Successes");
    let uncertain = b.place::<UncertainScrapeResult>("Uncertain");

    // Declared in neither the variants' order nor any other: routed outputs
    // are a set, and a `Vec` of them is a batch.
    b.transition(
        "Interpret",
        (TakeEdge(scraped),),
        (uncertain, successes),
        |data: ScrapedData| {
            if data.0.is_empty() {
                Outcome::Nothing
            } else if data.0.contains('?') {
                Outcome::Unsure(UncertainScrapeResult(data.0))
            } else {
                Outcome::Ok(SuccessfulScrape(data.0))
            }
        },
    );

    b.transition(
        "Interpret many",
        (TakeEdge(scraped),),
        (successes, uncertain),
        |data: ScrapedData| vec![Outcome::Ok(SuccessfulScrape(data.0))],
    );

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 2);
}
