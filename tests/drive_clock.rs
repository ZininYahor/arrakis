//! The driver's clock is one per process, started by its first app. It is a
//! static, so this is the only test in its binary.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use arrakis::app::{App, Signals};
use arrakis::drive::{self, Scenario};
use arrakis::settings::Settings;

fn app(scenario: Scenario) -> App {
    drive::app(
        scenario,
        Settings::default(),
        None,
        Signals::default(),
        |_| {},
    )
}

/// What a panic said.
fn said(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_default()
}

#[test]
fn the_clock_starts_with_the_first_app_and_a_second_moment_is_refused() {
    // Before any app there is no moment to count from. The clock used to
    // start at midnight here and stay there for every app after it.
    let early = catch_unwind(drive::clock).expect_err("a clock with no start");
    let message = said(&*early);
    assert!(message.contains("before drive::app()"), "{message}");

    // The first app starts it at its scenario's moment.
    let day = app(Scenario::ClosedDay);
    let moment = Scenario::ClosedDay.moment();
    let now = drive::clock();
    assert!(moment <= now && now < moment + 1.0, "{now} from {moment}");

    // A scenario with the same moment is the same clock.
    assert_eq!(Scenario::OpenIdle.moment(), moment);
    let idle = app(Scenario::OpenIdle);

    // One with another moment cannot have it: it used to count from the
    // first one's moment without a word.
    assert_ne!(Scenario::ClosedNight.moment(), moment);
    let late = catch_unwind(AssertUnwindSafe(|| app(Scenario::ClosedNight)));
    let message = said(&*late.err().expect("a refused second moment"));
    assert!(message.contains("another moment"), "{message}");
    assert!(message.contains(&moment.to_string()), "{message}");

    // The refusal left the clock where it was.
    let after = drive::clock();
    assert!(moment <= after && after < moment + 1.0, "{after}");
    drop((day, idle));
}
