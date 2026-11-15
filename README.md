<p align="center">
  <img src="docs/banner.png" width="800" alt="Arrakis. Night Shift, but it means it. The name and the line stand in the sky of a desert evening that the app draws itself: a low orange sun, and before it a sandworm's maw coming up out of the dunes, its rings of teeth lit.">
</p>

<p align="center">
  A macOS menu-bar app that turns every display red on a schedule.<br>
  <a href="#install">Install</a> · <a href="#use">Use</a> · <a href="#under-the-hood">Under the hood</a> · <a href="#known-problems">Known problems</a>
</p>

<p align="center">
  <img src="docs/evening.png" width="800" alt="The Arrakis window on an evening desktop, through one evening. At 20:55 the colors are normal. From 21:00 the whole screen fades to a deep orange over twenty minutes. The Temperature slider is dragged to pure red, its strip of light lit to the end, the cover's sandworm lifts its maw out of the dunes in front of the sun, and everything turns red while the blue sky goes dark. From 07:00 the screen fades back to normal.">
</p>
<p align="center"><sub>One evening, simulated: no screenshot can see the tint.</sub></p>

<p align="center">
  <img src="docs/temperature.png" width="800" alt="The Temperature scale as a gradient from white at 6500 K, through orange, to pure red. A bracket over the first part, ending at 2700 K, reads: Night Shift, to about 2700 K. A bracket over the whole scale reads: Arrakis, to pure red. A tick marks 1200 K, the default.">
</p>

Night Shift stops at a warm orange. Arrakis follows the black-body curve to
the end, where only red is left, and tints each display's color table, so
everything turns red: windows, video, even the cursor.

## Install

You need macOS 14 or later, [Rust](https://rustup.rs) and the Xcode Command
Line Tools.

```sh
git clone https://github.com/ZininYahor/arrakis
cd arrakis
scripts/bundle.sh
ditto target/bundle/Arrakis.app /Applications/Arrakis.app
```

> [!NOTE]
> The default schedule runs from 21:00 to 07:00. Inside it, the screen turns
> red about a second after the app starts. Quit from the menu bar, or press
> <kbd>⌘</kbd> <kbd>Q</kbd> in its window, to get the normal colors back.

<details>
<summary><b>Build notes and updates</b></summary>

The Command Line Tools come with `xcode-select --install`. Arrakis is tested
on macOS 26 on Apple silicon.

`scripts/bundle.sh` builds the release binary and wraps it in
`target/bundle/Arrakis.app`, with the app's icon, signed ad hoc. To update,
quit Arrakis, pull, and run the same two commands again. On a Mac older than
macOS 14 the script stops with a message before it builds or writes anything.

`cargo run --release` still works. It runs the bare binary, which has no icon
of its own: macOS shows a generic executable icon for it. Only the bundle
carries the app's icon. Arrakis keeps out of the Dock either way, so the
bundle's icon shows in Finder and Spotlight, not in the Dock. The bare binary
also runs without the memory flag that the bundle's `Info.plist` sets
(`MallocSpaceEfficient=1`, which lets malloc give memory back sooner): it holds
3 to 9 MiB more than the bundle does, by state.

Xcode itself is needed only to change the window's Metal shader:
`scripts/build-metallib.sh` compiles it ahead, with Xcode's Metal Toolchain,
into the file the app loads, so that the first window opens without compiling
it.

</details>

## Use

<p align="center">
  <img src="docs/window.png" width="420" alt="The Arrakis window in normal colors, dark oxide brown with a golden accent. A card at the top shows the night look like a track now playing: a cover of a sandworm's maw coming up out of the dunes before a low orange sun, 1200 K as the title, Turns red at 21:00 under it, the status line Now: normal colors · 1 display under that, and tonight from 21:00 to 07:00 as its progress bar. Under the card, the Temperature row reads 1200 K. Its track is a strip of light that runs from white through orange to deep red, lit up to 1200 K and dimmed beyond it. The names Daylight, Incandescent, Desert, Ember and Pure red sit under it, Ember in brighter ink, with small ticks at the Incandescent, Desert and Ember stops. The Brightness row at 100% is a flat orange strip from Dim to Full. Below are Turn red automatically, switched on, and the From 21:00, To 07:00 and Fade 20 min fields, each with a stepper. Red Now and a golden Close sit in a bar at the foot.">
</p>

- **Temperature**: how red the night gets, from Daylight to Pure red.
- **Brightness**: how dim, down to 20%.
- **From, To, Fade**: the daily window, and the minutes each end takes.
- **Red Now**: the night look right away, whatever the time.

Closing the window keeps the schedule running, with the sun in the menu bar.

<details>
<summary><b>More about the window and the menu</b></summary>

The window opens with the sun in the menu bar, half sunk behind a dune.

While you hold a slider, the real screen follows. The picture at the top of
the window always shows your night look. The window keeps its contrast in
brightness, not in color, so it stays readable through its own red tint.

The Temperature track is a strip of light, lit up to the value. Its stops,
from left to right, are Daylight, at 6500 K; Incandescent, at 2700 K; Desert,
at 1760 K, halfway between Incandescent and Ember; Ember, at 1200 K; and Pure
red. Each has its name under the track. A drag catches on the three stops
between the ends and on pure red's end, and a click or press near one catches
it too. A Force Touch trackpad ticks there.

The daily window may cross midnight. The menu-bar menu can go red now, pause
the schedule, open the window, or quit.

</details>

<details>
<summary><b>Keyboard</b></summary>

- <kbd>Tab</kbd> moves between controls.
- <kbd>←</kbd> <kbd>↓</kbd> move a slider one step left, and <kbd>→</kbd>
  <kbd>↑</kbd> one step right. <kbd>Page Down</kbd> and <kbd>Page Up</kbd>
  move it four steps, and <kbd>Home</kbd> and <kbd>End</kbd> go to its ends.
- On Temperature, an arrow or a Page key lands on a stop it would step past,
  and <kbd>⌥</kbd> with an arrow jumps from stop to stop.
- In a time or the fade, <kbd>↑</kbd> <kbd>↓</kbd>, digits or the stepper
  change the value. In From and To, <kbd>←</kbd> <kbd>→</kbd> switch between
  hours and minutes.
- <kbd>Space</kbd> presses the focused button or switch.
- <kbd>Esc</kbd> or <kbd>⌘</kbd> <kbd>W</kbd> hides the window.
- <kbd>⌘</kbd> <kbd>Q</kbd> quits.

</details>

<details>
<summary><b>Settings file and command line</b></summary>

Settings, and a log named `arrakis.log`, are kept in
`~/Library/Application Support/dev.Arrakis.Arrakis/`. On its first start,
Arrakis copies the settings of Sunset Parabellum, as it was called before.
You can edit `settings.json` by hand:

```json
{
  "schedule_on": true,
  "start": "21:00",
  "end": "07:00",
  "fade_minutes": 20,
  "kelvin": 1200,
  "brightness": 100
}
```

`kelvin` runs from 6500 down to 850, which is pure red. `brightness` runs
from 100 down to 20. Values outside these ranges are pulled back in.

`--hidden` starts in the menu bar without the window. `--verbose` logs at
debug level.

</details>

## Under the hood

<p align="center">
  <img src="docs/bench.png" width="800" alt="Readings from a MacBook Pro with an M1 Pro. On top, three readouts: 119.7 frames a second in a drag on a 120 Hz panel; more than 1000 tests; no network crate and no async runtime among the 169 crates in the tree. Under them, three strips, each lit up to the latest reading and dimmed up to what it was: CPU energy a frame while dragging, 239 µJ on 12 November, was 510 on the GL painter on 7 November; memory with the window open, about 19 MiB on 7 November, was about 32 on 31 October; a cold first window, 152 ms, was 227, both on 7 November 2026.">
</p>

- **Its own Metal painter**: egui's meshes go straight to Metal, with no wgpu
  in the binary, and GL as the fallback.
- **Built ahead**: the color table, the menu-bar icon and the shader are
  embedded.
- **Small**: a 4.8 MB binary in Rust, with no async runtime, no network and
  no telemetry.

<details>
<summary><b>How the color works</b></summary>

- **Color.** Planck's law and the CIE 1931 color-matching functions give
  each temperature an sRGB color, scaled so that 6500 K changes nothing.
  Below about 850 K, green leaves the gamut and clips to zero.
- **Fades** move along that curve rather than between two colors, so they
  pass through orange and never through pink.
- **Your colors come back.** Before the first tint, the app saves each
  display's own color table, and it writes that table back when it quits.
  While the screen is red, it puts the tint back when macOS or another app
  resets it: macOS tells it of sleep, user switches and apps quitting, and
  it checks on its own, every 2 to 30 seconds, for the rest.

</details>

<details>
<summary><b>Checks, and how these pictures are made</b></summary>

```sh
cargo fmt --check
cargo clippy --all-targets --features demo -- -D warnings
cargo test --features demo
```

The color curve is not copied from anywhere. Tests check it against the
table in redshift's `colorramp.c` (Ingo Thies, 2013).

The pictures come from the app itself. The `demo` build draws a scene with
a clock it moves by hand, and leaves the real displays alone:

```sh
cargo run --features demo -- --demo-shot docs/banner.png --demo-scene banner
cargo run --features demo -- --demo-shot docs/evening.png --demo-scene evening
cargo run --features demo -- --demo-shot docs/temperature.png --demo-scene scale
cargo run --features demo -- --demo-shot docs/window.png --demo-scene day
cargo run --features demo -- --demo-shot docs/bench.png --demo-scene bench
```

The banner's worm is the window's: the same code draws both. The figures in
`docs/bench.png` are constants in `src/demo.rs`, each with the reading it
comes from: its build, its date and its recipe.

Screenshots of a window cannot see the system's traffic lights either, so
the demo paints them in and rounds the corners as macOS does.

The app's icon is drawn the same way, from the cover's sun and dunes and the
palette. `assets/icon.png` is its 1024 px master, and `scripts/bundle.sh`
makes the bundle's `.icns` from it:

```sh
cargo run --features demo -- --demo-icon assets/icon.png
```

</details>

## Known problems

- Only one app can own a display's color table. f.lux, Lunar and other
  gamma tools will fight with Arrakis.
- Sidecar, AirPlay and DisplayLink displays may ignore color tables. Some
  M5 Macs on macOS 26 are reported to accept them without showing them.
- Night Shift is applied on top. Turn it off for an exact look.

<br>

<p align="center"><sub>Color fit by Wyman, Sloan and Shirley · built on egui and <a href="https://github.com/crmne/fastframe">fastframe</a> · <a href="LICENSE">MIT license</a></sub></p>
