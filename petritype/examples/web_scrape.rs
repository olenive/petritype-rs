//! A hypothetical web scrape: fetch, classify, and rescue the odd ones.
//!
//! A port of the Python
//! `examples/illustrations/hypothetical_web_scrape.py`. The fetches are
//! simulated, as they are there: the point is the shape of the pipeline, not
//! the network.
//!
//! What it demonstrates: a token branching **by type**, twice over.
//! `Classify` returns either a [`SuccessfulScrape`] or an
//! [`UncertainScrapeResult`], and `Special Cases` turns an uncertain one into
//! either a success or a [`FailedScrape`]. In Python those are `Union[...]`
//! return annotations resolved by reflection; in Rust they are
//! [`#[derive(Routed)]`](petritype::Routed) enums, whose payload types are
//! matched to output places when the transition is declared, so a route with
//! nowhere to go is a build error rather than a surprise at run time.
//!
//! Note that a *failed* scrape here is ordinary data routed to a place, not a
//! failed firing (DESIGN §7): an expected bad outcome is a token like any
//! other.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Net, NetBuilder, PlaceRef, Routed, TakeEdge};

/// What to scrape.
#[derive(Debug, Clone, PartialEq)]
struct ScrapeParameters {
    url: String,
}

/// What came back, before anyone has judged it.
#[derive(Debug, Clone, PartialEq)]
struct ScrapedData {
    parameters: ScrapeParameters,
    data: String,
}

/// Judged good.
#[derive(Debug, Clone, PartialEq)]
struct SuccessfulScrape {
    parameters: ScrapeParameters,
    result: ScrapedData,
}

/// Judged neither good nor bad; `Special Cases` decides.
#[derive(Debug, Clone, PartialEq)]
struct UncertainScrapeResult {
    parameters: ScrapeParameters,
    result: ScrapedData,
}

/// Judged bad, with the reason.
#[derive(Debug, Clone, PartialEq)]
struct FailedScrape {
    parameters: ScrapeParameters,
    result: ScrapedData,
    error: String,
}

/// What `Classify` returns: good, or not yet decided.
#[derive(Routed)]
enum Classified {
    Successful(SuccessfulScrape),
    Uncertain(UncertainScrapeResult),
}

/// What `Special Cases` returns: rescued, or given up on.
#[derive(Routed)]
enum Resolution {
    Successful(SuccessfulScrape),
    Failed(FailedScrape),
}

/// The net and the two places the run ends in.
struct Pipeline {
    net: Net,
    successes: PlaceRef<SuccessfulScrape>,
    failures: PlaceRef<FailedScrape>,
}

/// A scrape that never leaves the process: the URL decides what comes back.
fn simulated_scrape_attempt(parameters: ScrapeParameters) -> ScrapedData {
    let data = if parameters.url.starts_with("wrong/page") {
        "Encountered a wrong error"
    } else if parameters.url.starts_with("difficult/page") {
        "Maybe difficult valid data"
    } else {
        "Valid data"
    };
    ScrapedData {
        parameters,
        data: data.to_owned(),
    }
}

/// Anything that mentions trouble is uncertain; the rest is a success.
fn classify_scrape_data(data: ScrapedData) -> Classified {
    let lower = data.data.to_lowercase();
    if lower.contains("difficult") || lower.contains("wrong") {
        Classified::Uncertain(UncertainScrapeResult {
            parameters: data.parameters.clone(),
            result: data,
        })
    } else {
        Classified::Successful(SuccessfulScrape {
            parameters: data.parameters.clone(),
            result: data,
        })
    }
}

/// A "maybe" is rescued; anything else is a definite failure.
fn handle_special_cases(data: UncertainScrapeResult) -> Resolution {
    if data.result.data.to_lowercase().contains("maybe") {
        Resolution::Successful(SuccessfulScrape {
            parameters: data.parameters,
            result: data.result,
        })
    } else {
        Resolution::Failed(FailedScrape {
            parameters: data.parameters,
            result: data.result,
            error: String::from("Definite error encountered"),
        })
    }
}

/// `Input Parameters -> Scrape -> Response -> Classify -> {Successes,
/// Uncertain}`, and `Uncertain -> Special Cases -> {Successes, Failures}`.
fn build() -> Result<Pipeline, BuildError> {
    let mut b = NetBuilder::new();
    let input = b.place::<ScrapeParameters>("Input Parameters");
    let response = b.place::<ScrapedData>("Response");
    let uncertain = b.place::<UncertainScrapeResult>("Uncertain");
    let failures = b.place::<FailedScrape>("Failures");
    let successes = b.place::<SuccessfulScrape>("Successes");

    b.seed(
        input,
        [
            "valid/page_01",
            "wrong/page_01",
            "difficult/page_01",
            "valid/page_02",
        ]
        .map(|url| ScrapeParameters {
            url: url.to_owned(),
        }),
    );

    b.transition(
        "Scrape",
        (TakeEdge(input),),
        (response,),
        |parameters: ScrapeParameters| (simulated_scrape_attempt(parameters),),
    );
    b.transition(
        "Classify",
        (TakeEdge(response),),
        (successes, uncertain),
        classify_scrape_data,
    );
    b.transition(
        "Special Cases",
        (TakeEdge(uncertain),),
        (successes, failures),
        handle_special_cases,
    );

    Ok(Pipeline {
        net: b.build()?,
        successes,
        failures,
    })
}

/// One field of every token in a place, oldest first.
fn fields<T: 'static, R>(net: &Net, place: PlaceRef<T>, of: impl Fn(&T) -> R) -> Vec<R> {
    net.tokens(place.id())
        .filter_map(|(_, value)| value.downcast_ref::<T>())
        .map(of)
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pipeline = build()?;
    let summary = drivers::run(
        &mut pipeline.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    println!("{} firings", summary.fired_count);
    println!(
        "successes: {:?}",
        fields(&pipeline.net, pipeline.successes, |s| s
            .parameters
            .url
            .clone())
    );
    println!(
        "failures:  {:?}",
        fields(&pipeline.net, pipeline.failures, |f| f
            .parameters
            .url
            .clone())
    );
    println!("{}", petritype_dot::dot(&pipeline.net));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Classified, Resolution, ScrapeParameters};
    use super::{
        build, classify_scrape_data, fields, handle_special_cases, simulated_scrape_attempt,
    };
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    /// Builds the pipeline and runs it to quiescence.
    fn run() -> super::Pipeline {
        let mut pipeline = build().expect("the net builds");
        let summary = drivers::run(
            &mut pipeline.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("a bad scrape is data, not a failed firing");
        assert!(summary.quiesced);
        assert_eq!(summary.failed_count, 0);
        // Four scrapes, four classifications, two special cases.
        assert_eq!(summary.fired_count, 10);
        pipeline
    }

    #[test]
    fn the_clean_pages_and_the_rescued_one_end_in_successes() {
        let pipeline = run();

        assert_eq!(
            fields(&pipeline.net, pipeline.successes, |s| s
                .parameters
                .url
                .clone()),
            ["valid/page_01", "valid/page_02", "difficult/page_01"],
        );
    }

    #[test]
    fn the_erroneous_page_ends_in_failures_with_its_reason() {
        let pipeline = run();

        assert_eq!(
            fields(&pipeline.net, pipeline.failures, |f| f
                .parameters
                .url
                .clone()),
            ["wrong/page_01"],
        );
        let reasons = fields(&pipeline.net, pipeline.failures, |f| f.error.clone());
        assert_eq!(reasons, ["Definite error encountered"]);
    }

    #[test]
    fn nothing_is_left_in_the_middle_of_the_pipeline() {
        let pipeline = run();

        for name in ["Input Parameters", "Response", "Uncertain"] {
            let place = pipeline.net.place_named(name).expect("the place is there");
            assert_eq!(pipeline.net.count(place), 0, "{name} should be empty");
        }
    }

    #[test]
    fn each_branch_of_each_union_is_reachable() {
        let clean = simulated_scrape_attempt(ScrapeParameters {
            url: String::from("valid/page_03"),
        });
        assert!(matches!(
            classify_scrape_data(clean),
            Classified::Successful(_)
        ));

        let odd = simulated_scrape_attempt(ScrapeParameters {
            url: String::from("difficult/page_03"),
        });
        let Classified::Uncertain(odd) = classify_scrape_data(odd) else {
            panic!("a difficult page is uncertain");
        };
        assert!(matches!(
            handle_special_cases(odd),
            Resolution::Successful(_)
        ));
    }

    #[test]
    fn successes_is_written_from_two_transitions() {
        let pipeline = build().expect("the net builds");
        let text = petritype_dot::dot(&pipeline.net);

        // Classify is t1 and Special Cases t2; Successes is p4.
        assert!(text.contains("  t1 -> p4 ["));
        assert!(text.contains("  t2 -> p4 ["));
    }
}
