//! A failed Metal painter, through a real `Shell` and no window: the app closes the window by the
//! path of every close, hides, and opens the next one on glow.
//!
//! The painter's failure flag is the process's, and it never goes down again, so this test is its own
//! process (`harness = false`), and AppKit's windows and loop are the main thread's, which libtest's
//! threads are not. `eframe::egui_wgpu::fail_detached` raises the flag with no painter to fail.
//!
//! It orders no window in, opens no eframe window and touches no display. What it reads that
//! `src/app.rs`'s tests cannot: that the real shell calls `window_gone` after `Hide` and reopens on
//! `Show`, with a real `Tray` for `Action::HideWindow` and for a window nobody sees. What it does not
//! read: the `Preview(None)` that goes to `drive::app`'s engine, whose channel this test cannot see,
//! and the `Close` coming back as a close request, which eframe feeds. `src/app.rs`'s tests read both,
//! from `detached_app`'s channel and a fed pass. The real window, eframe and the painter are read by
//! `scripts/painter-fallback.sh`.

#[cfg(target_os = "macos")]
#[macro_use]
mod pump;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    cases::main();
}

#[cfg(target_os = "macos")]
mod cases {
    use std::thread;
    use std::time::Duration;

    use arrakis::app::{App, Signals};
    use arrakis::drive::{self, Scenario};
    use arrakis::idle;
    use arrakis::model::Action;
    use arrakis::painter::{self, WindowPainter};
    use arrakis::settings::Settings;
    use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
    use fastframe_tray::Tray;

    use crate::pump::{case, watch};

    pub fn main() {
        watch();
        // The first run of `NSApp` finishes launching the app, once. A case is about the fallback,
        // not about that.
        idle::publish(Headless::Wait);
        idle::idle(Duration::from_millis(10));

        if !eframe::egui_wgpu::has_device() {
            eprintln!("skipped: no Metal device here, so no window gets the Metal painter to fail");
            return;
        }

        case(
            "1. before a failure, a window gets the Metal painter",
            || {
                check!(!painter::failed(), "the flag was up at the start");
                check!(
                    eframe::egui_wgpu::failure().is_none(),
                    "the crate holds a failure at the start"
                );
                check!(
                    matches!(painter::renderer(), eframe::Renderer::Wgpu),
                    "a Mac with a device did not get the Metal painter"
                );
            },
        );

        case(
            "2. a failed Metal window in an app with no tray hides and the next window is glow's",
            tray_less,
        );

        case("3. the flag stays up: every later window is glow's", || {
            check!(painter::failed(), "the flag went down");
            check!(
                matches!(painter::renderer(), eframe::Renderer::Glow),
                "a later window did not get glow"
            );
        });

        case(
            "4. with a tray, a hide after the failing pass clears the reopen and a close reopens",
            tray_backed,
        );

        case(
            "5. with a tray, a window nobody sees closes to the menu bar and reopens nothing",
            nobody_sees_it,
        );
    }

    /// The painter a window gets now, as `entrypoint` maps eframe's choice.
    fn next_window_painter() -> WindowPainter {
        if matches!(painter::renderer(), eframe::Renderer::Wgpu) {
            WindowPainter::Metal
        } else {
            WindowPainter::Glow
        }
    }

    /// One pass of `ctx` that runs `logic` alone, as eframe runs it for a window that is covered or
    /// minimized: says how many `Close` the app asked for in it.
    fn closes_asked(app: &mut App, ctx: &egui::Context) -> usize {
        closes_asked_with(app, ctx, &egui::RawInput::default())
    }

    fn closes_asked_with(app: &mut App, ctx: &egui::Context, input: &egui::RawInput) -> usize {
        let output = ctx.run_logic(input, |ctx| {
            app.background_frame(Some(ctx));
        });
        output
            .viewport_commands
            .into_values()
            .flatten()
            .filter(|command| matches!(command, egui::ViewportCommand::Close))
            .count()
    }

    /// What eframe tells `logic` of a window covered or minimized.
    fn out_of_sight(occluded: bool, minimized: bool) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.occluded = Some(occluded);
        viewport.minimized = Some(minimized);
        input
    }

    fn driver_app(tray: Option<Tray>) -> App {
        drive::app(
            Scenario::OpenClose,
            Settings::default(),
            tray,
            Signals::default(),
            |_| {},
        )
    }

    /// The path of a tray-less app, whose `attach` leaves `closed()` at
    /// `Quit`, so that only the fallback's own read can make it `Hide`.
    fn tray_less() {
        let waker = Waker::default();
        let mut windows: Vec<WindowPainter> = Vec::new();
        Shell::new(driver_app(None), &waker)
            .run(|lease| {
                let ctx = egui::Context::default();
                let mut app = lease.take(&ctx);
                let painter = next_window_painter();
                windows.push(painter);
                app.attach(painter);
                match windows.len() {
                    1 => {
                        check!(
                            painter == WindowPainter::Metal,
                            "the first window is Metal's"
                        );
                        check!(
                            app.closed() == Closed::Quit,
                            "a tray-less app quits on a close, so only the fallback can hide"
                        );
                        eframe::egui_wgpu::fail_detached("a failure raised by the test");
                        check!(painter::failed(), "the flag did not go up");
                        check!(
                            matches!(painter::renderer(), eframe::Renderer::Glow),
                            "the next window is not glow's"
                        );
                        let closes = closes_asked(&mut app, &ctx);
                        check!(closes == 1, "the failed window asked for {closes} closes");
                        check!(
                            app.closed() == Closed::Hide,
                            "the failed window closes to {:?}",
                            app.closed()
                        );
                        check!(
                            app.reopening_on_glow(),
                            "the close is not marked as a reopen"
                        );
                    }
                    2 => {
                        // Reached only if the shell called `window_gone` after `Hide`, and its
                        // headless pass answered `Show` from the hand-over of `reopen_on_glow`.
                        check!(
                            painter == WindowPainter::Glow,
                            "the second window is glow's"
                        );
                        check!(
                            app.closed() == Closed::Quit,
                            "the new window starts as any other"
                        );
                        check!(!app.reopening_on_glow(), "the reopen was not cleared");
                        let closes = closes_asked(&mut app, &ctx);
                        check!(closes == 0, "a glow window asked for {closes} closes");
                        app.apply(Action::Quit, None);
                    }
                    n => check!(false, "window {n}: the shell kept opening windows"),
                }
                Ok::<(), ()>(())
            })
            .expect("the shell's run");
        check!(
            windows == [WindowPainter::Metal, WindowPainter::Glow],
            "the windows were {windows:?}"
        );
    }

    /// A real `Tray` meets `Action::HideWindow` here and nowhere else. The flag is up already, so a
    /// window on Metal fails on its first pass. Called on the app directly, as a shell whose headless
    /// pass has to be told to quit could not tell "no `Show`" from a quit that came first.
    fn tray_backed() {
        let mut app = driver_app(Some(unseen_tray()));
        let hidden = egui::Context::default();

        // A failure, then a hide the person asked for in the same pass: it stays a hide.
        let ctx = egui::Context::default();
        app.attach(WindowPainter::Metal);
        check!(
            app.closed() == Closed::Hide,
            "a tray-backed app hides on a close"
        );
        let closes = closes_asked(&mut app, &ctx);
        check!(closes == 1, "the failed window asked for {closes} closes");
        check!(
            app.reopening_on_glow(),
            "the close is not marked as a reopen"
        );
        app.apply(Action::HideWindow, None);
        check!(!app.reopening_on_glow(), "a hide did not clear the reopen");
        check!(
            app.closed() == Closed::Hide,
            "the hide closes to {:?}",
            app.closed()
        );
        app.window_gone();
        let answer = app.headless_frame(&hidden);
        check!(
            !matches!(answer, Headless::Show | Headless::Quit),
            "after a hide the app answers {answer:?}"
        );

        // The same failure with no hide: the window reopens, on glow.
        app.attach(WindowPainter::Metal);
        let closes = closes_asked(&mut app, &ctx);
        check!(
            closes == 1,
            "the next failed window asked for {closes} closes"
        );
        check!(
            app.reopening_on_glow(),
            "the close is not marked as a reopen"
        );
        app.window_gone();
        let answer = app.headless_frame(&hidden);
        check!(
            matches!(answer, Headless::Show),
            "after a failed window the app answers {answer:?}"
        );
        app.shutdown();
    }

    /// A new centered window would take the keyboard from whatever the person is doing, so one nobody
    /// sees goes to the menu bar and waits. The flag is up already, as in [`tray_backed`], and so is the
    /// app's tray: without one it would have no way back, and `src/app.rs`'s tests read that case.
    fn nobody_sees_it() {
        let mut app = driver_app(Some(unseen_tray()));
        let hidden = egui::Context::default();
        for (what, input) in [
            ("covered", out_of_sight(true, false)),
            ("minimized", out_of_sight(false, true)),
        ] {
            let ctx = egui::Context::default();
            app.attach(WindowPainter::Metal);
            let closes = closes_asked_with(&mut app, &ctx, &input);
            check!(closes == 1, "{what}: it asked for {closes} closes");
            check!(
                app.closed() == Closed::Hide,
                "{what}: it closes to {:?}",
                app.closed()
            );
            check!(!app.reopening_on_glow(), "{what}: the close is a reopen");
            app.window_gone();
            let answer = app.headless_frame(&hidden);
            check!(
                !matches!(answer, Headless::Show | Headless::Quit),
                "{what}: with nobody asking the app answers {answer:?}"
            );
            // The person asks, as a click on the menu-bar item does, and that window is glow's.
            app.apply(Action::ShowWindow, None);
            let answer = app.headless_frame(&hidden);
            check!(
                matches!(answer, Headless::Show),
                "{what}: asked for a window the app answers {answer:?}"
            );
            check!(
                next_window_painter() == WindowPainter::Glow,
                "{what}: the window asked for is not glow's"
            );
        }
        app.shutdown();
    }

    /// A real `Tray` that shows nothing. Off the main thread, `create_item` gives up the settings it
    /// keeps for the item and makes none, so the main thread's `attach` finds none to make or to
    /// bring forward (fastframe-tray's macOS `Host`, at the tag in `Cargo.toml`).
    fn unseen_tray() -> Tray {
        thread::spawn(|| {
            let config = fastframe_tray::Config {
                id: "arrakis-painter-fallback",
                title: "Arrakis".to_owned(),
                icon: arrakis::icon::tray,
                template_icon: None,
                themed_icon: false,
                menu: App::tray_menu(),
            };
            let mut tray = Tray::spawn(config, || {}).expect("macOS always gives a tray");
            tray.create_item();
            tray
        })
        .join()
        .expect("the tray's thread")
    }
}
