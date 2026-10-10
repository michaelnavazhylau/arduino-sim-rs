# arduino-sim-rs

[![crates.io](https://img.shields.io/crates/v/avr8rs.svg)](https://crates.io/crates/avr8rs)
[![docs.rs](https://docs.rs/avr8rs/badge.svg)](https://docs.rs/avr8rs)
[![license](https://img.shields.io/crates/l/avr8rs.svg)](LICENSE)

A native Rust AVR simulator compatibility port, with a pinned AVR8js reference
and a converted behavioral contract. No JavaScript runtime or bridge is used.

The simulator engine lives in its own repository and is consumed here by version:
[`avr8rs`](https://github.com/michaelnavazhylau/avr8rs) on
[crates.io](https://crates.io/crates/avr8rs) · [API docs](https://docs.rs/avr8rs).

![To-scale Uno wired to an HC-SR04, with a target cube at the distance the firmware measured](blink-gui/docs/ranging.png)

*The optional raylib front-end running a real HC-SR04 sketch: a to-scale Uno
wired to the module, a 100 mm ruler track, and a target cube at the distance the
**firmware** measured. See [`blink-gui/`](blink-gui/) for more, including the
blink views and the circuit behind them.*

## Get started

```sh
git clone https://github.com/michaelnavazhylau/arduino-sim-rs.git
cd arduino-sim-rs/blink-gui
cargo test --release --locked
```

The engine is a crates.io dependency, so no submodule initialization is needed
and a cold clone builds from the registry.

## Working on the engine

Because the engine is consumed by version, an engine change is visible here only
after a version bump and a `cargo publish`. While you are iterating on it, you can
compile against a sibling checkout instead:

```sh
cp .cargo/config.toml.example .cargo/config.toml     # points at ../avr8rs
```

Cargo then builds `breadboard` and `blink-gui` against that checkout, picking up
uncommitted engine edits with no publish. Two things to know:

- The override rewrites `Cargo.lock`: the `avr8rs` entry stops being a registry
  package, so it loses its `source` and `checksum`. Don't commit that — when the
  engine work is done, `rm .cargo/config.toml` and
  `git checkout -- breadboard/Cargo.lock blink-gui/Cargo.lock`.
- CI never sees the file, so its `--locked` gates keep resolving the published
  crate.

[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity) needs none of
this: it depends on the checkout by path already, because its job is to gate
*unreleased* engine commits.

## Layout and status

This repository holds the front-end. The engine and the converted contract are
separate repositories, consumed here by version.

- [`blink-gui/`](blink-gui/): optional raylib GUI. It runs a real blink sketch
  on the simulator as a flat schematic or a procedural 3D rendition of the Uno,
  runs a real HC-SR04 sketch in a **to-scale** (1 unit = 10 mm) 3D scene where the
  Uno is wired to the module with jumpers and a target cube sits at the distance
  the firmware measured, and runs a three-LED breadboard demo where equal 330 Ω
  resistors turn out to be a different operating point on red, green and blue.
  Kept out of the engine repository so the engine stays dependency-free.
- [`breadboard/`](breadboard/): headless host, simulated-time event scheduler,
  analog pin/ADC coupling, I2C and SPI device attachment, and external components
  (an HC-SR04 ultrasonic distance sensor) wired to a real AVR board. Electrical
  circuits are SPICE decks solved by
  [ngspice-rs](https://github.com/michaelnavazhylau/ngspice-rs), a from-scratch
  Rust port of ngspice with no FFI. Depends on `avr8rs` from crates.io; the
  engine does not depend on it.
- [`avr8rs`](https://github.com/michaelnavazhylau/avr8rs), a **separate
  repository**: the dependency-free engine — simulator core, host-side board
  glue, the scenario representation and its runner, and the AVR hardware
  reference bundle (`specs/`). It also pins the AVR8js submodule at
  `bee6f0a94e0e27786f6bc21aee3c775849fb50fd` for provenance.
- [`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity), a
  **separate repository**: the converted contract — 14 AVR8js suites, 347 cases,
  853 assertion sites — plus the AST converter and the deterministic-conversion
  check. Split out so 27k lines of generated scenarios stop sharing a history and
  a lockfile with the hand-written engine, now in its own repository.

**All 347 converted scenarios pass by default**, with **zero ignored cases**.
They run in `avr8js-parity` against the engine crate, alongside a coverage test;
the engine's own 45 tests run in `avr8rs`. Implementation milestones
0–6 are complete: assembler, CPU/instructions/interrupts, clock, GPIO, megaAVR
timers, ATtiny Timer1, EEPROM, ADC, SPI, USART, TWI master states and watchdog.
Passing tests establish the converted baseline, not complete AVR8js API parity
or hardware fidelity. The engine also carries an `arduino-cli` integration test
that compiles an ATmega328P sketch and runs the produced HEX image on the native
backend (skipped when the `arduino:avr` core is unavailable).

Gates are split along the same boundaries: `bash breadboard/tools/verify-breadboard.sh`
here, `bash tools/verify-native.sh` in `avr8rs` (plus its `Arduino CLI end-to-end`
workflow), and the converted contract's own gate in `avr8js-parity`, which also
verifies deterministic conversion against the pinned reference. Node/TypeScript is
used only in the parity repository, to convert and verify scenarios, never to
simulate.

The engine exposes a small host-side API for running firmware outside the
converted scenarios: [`board::parse_hex`](https://github.com/michaelnavazhylau/avr8rs/blob/main/src/board.rs)
decodes Intel HEX, `Board::uno` wires the ATmega328P device profile, and
`Board::step` advances one instruction plus its peripheral tick.
[`blink-gui/`](blink-gui/)
is a worked example — a raylib window with a schematic view and a procedural 3D
board. `PORTB5` drives a finite-impedance source feeding a 220 Ω resistor and
LED; the external LED's brightness follows solved forward current, and reversing
it blocks conduction. The UI shows voltage/current/power. The operating point is
a `.op` solve by ngspice-rs — nonlinear DC, not transient SPICE and not an
active current regulator.
The GUI crate links a system raylib and is
intentionally not part of the offline CI gate; see its README for requirements,
for why raylib cannot load STL, and for the `nobuild` camera-module caveat.

[`breadboard/`](breadboard/) is the headless counterpart for **time, analog
coupling and buses**: it owns simulated-time scheduling, external components, a
deck-driven analog loop that drives `PINx` and the ADC from solved node
voltages, and I2C/SPI devices attached through the core's own callback surface.
An HC-SR04 demonstrates a sensor whose ECHO pulse is produced at cycle
granularity and read back through the AVR's `PINx` register; a sensor is a
stateful device whose output depends on *when* things happened, so it implements
a `Stimulus` rather than an element in a SPICE deck, keeping digital timing out
of the electrical solve. The analog loop is **DC only** (no RC transients), SPI
has no chip-select because the core does not model `SS`, and I2C is master-only.
Verify it with `bash breadboard/tools/verify-breadboard.sh`.

[`blink-gui/`](blink-gui/) shows that same sensor: the HC-SR04 sketch times ECHO
with `pulseIn`, publishes its own result over I2C (`Wire`) to a host sink, and a
to-scale 3D scene draws the Uno, its jumpers to the module, a 100 mm ruler track
and a target cube at that distance. Because the firmware does the measuring, the
demo exercises the AVR core, the sensor model and the I2C bridge in one path —
and the ranging tests run headlessly against the compiled firmware. A fourth view
puts three indicator LEDs on a to-scale breadboard, one 330 Ω resistor each,
where the point is that equal resistors are *not* equal currents: the solved
forward voltages span about a volt from red to blue. Two more views wire the
**same push button two ways** — to ground under the AVR's internal pull-up, and
to 5 V with an external pull-down — so it reads opposite levels in each.

See [`avr8rs/README.md`](https://github.com/michaelnavazhylau/avr8rs#readme) for
the engine's checks, and
[`specs/backend-plan.md`](https://github.com/michaelnavazhylau/avr8rs/blob/main/specs/backend-plan.md)
for milestones and residual limitations. Regeneration and contract checks live in
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity).
Downloaded vendor PDFs/text and build dependencies are excluded from Git; their
pinned provenance and fetch/check tools are included in `avr8rs`.

## Future electrical simulation work

The [sim2real implementation roadmap](docs/sim2real-roadmap.md) records the planned
path from today's DC operating points to backward-Euler RC/RL transients,
BJT/MOSFET models, deterministic AVR/analog time coupling, practical parasitics,
tolerances and temperature. ngspice-rs already supplies the device models and the
transient integrator on the electrical side; what remains is host-side causal
AVR/analog synchronization and a deck-level part catalogue with provenance.
These are future capabilities, not current feature claims.

## License and citation

Released under the [MIT License](LICENSE). Portions of this project's history are
derived from [AVR8js](https://github.com/wokwi/avr8js) — MIT, Copyright (c)
2019-2025 Uri Shaked. That derived work now lives in
[`avr8rs`](https://github.com/michaelnavazhylau/avr8rs) and
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity), which keep
the upstream notice; it is reproduced here in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) as well. Microchip datasheets
referenced under `avr8rs/specs/` keep their vendor terms and are not
redistributed. The Arduino AVR core and `arduino-cli` used by the engine's test
suite are installed at test time and are not part of this repository either.

To cite this project, use [`CITATION.cff`](CITATION.cff).
