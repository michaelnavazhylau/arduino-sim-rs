# arduino-sim-rs

[![crates.io](https://img.shields.io/crates/v/avr-sim.svg)](https://crates.io/crates/avr-sim)
[![docs.rs](https://docs.rs/avr-sim/badge.svg)](https://docs.rs/avr-sim)
[![license](https://img.shields.io/crates/l/avr-sim.svg)](LICENSE)

A native Rust AVR simulator compatibility port, with a pinned AVR8js reference
and a converted behavioral contract. No JavaScript runtime or bridge is used.

![To-scale Uno wired to an HC-SR04, with a target cube at the distance the firmware measured](blink-gui/docs/ranging.png)

*The optional raylib front-end running a real HC-SR04 sketch: a to-scale Uno
wired to the module, a 100 mm ruler track, and a target cube at the distance the
**firmware** measured. See [`blink-gui/`](blink-gui/) for more, including the
blink views and the circuit behind them.*

## Get started

```sh
git clone --recurse-submodules https://github.com/michaelnavazhylau/arduino-sim-rs.git
cd arduino-sim-rs/rust_port
cargo test --offline
```

For an existing clone, initialize the reference with
`git submodule update --init --recursive`.

## Layout and status

- [`rust_port/`](rust_port/): the dependency-free `avr-sim` crate — simulator
  core, host-side board glue, the scenario representation and its runner,
  hardware specification provenance and the offline engine gate. Published on
  [crates.io](https://crates.io/crates/avr-sim) with
  [API docs](https://docs.rs/avr-sim).
- [`blink-gui/`](blink-gui/): optional raylib GUI. It runs a real blink sketch
  on the simulator as a flat schematic or a procedural 3D rendition of the Uno,
  runs a real HC-SR04 sketch in a **to-scale** (1 unit = 10 mm) 3D scene where the
  Uno is wired to the module with jumpers and a target cube sits at the distance
  the firmware measured, and runs a three-LED breadboard demo where equal 330 Ω
  resistors turn out to be a different operating point on red, green and blue.
  Kept outside `avr-sim` (`rust_port/`) so the core stays dependency-free and the
  offline engine gate is unaffected.
- [`breadboard/`](breadboard/): headless host, simulated-time event scheduler,
  analog pin/ADC coupling, I2C and SPI device attachment, and external components
  (an HC-SR04 ultrasonic distance sensor) wired to a real AVR board. Electrical
  circuits are SPICE decks solved by
  [ngspice-rs](https://github.com/michaelnavazhylau/ngspice-rs), a from-scratch
  Rust port of ngspice with no FFI. Depends on `avr-sim`; the core does not
  depend on it.
- [`avr8js/`](avr8js/): upstream reference submodule pinned at
  `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`. Kept for provenance — it records
  the revision this port's compatibility claims are written against, and nothing
  here builds or executes it.
- [`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity), a
  **separate repository**: the converted contract — 14 AVR8js suites, 347 cases,
  853 assertion sites — plus the AST converter and the deterministic-conversion
  check. Split out so 27k lines of generated scenarios stop sharing a history and
  a lockfile with the hand-written engine.

**All 347 converted scenarios pass by default**, with **zero ignored cases**.
They run in `avr8js-parity` against this repository's `avr-sim` crate, alongside
a coverage test; the engine's own 45 tests run here. Implementation milestones
0–6 are complete: assembler, CPU/instructions/interrupts, clock, GPIO, megaAVR
timers, ATtiny Timer1, EEPROM, ADC, SPI, USART, TWI master states and watchdog.
Passing tests establish the converted baseline, not complete AVR8js API parity
or hardware fidelity. The engine set includes an `arduino-cli` integration test
that compiles an ATmega328P sketch and runs the produced HEX image on the native
backend (skipped when the `arduino:avr` core is unavailable).

Run `bash rust_port/tools/verify-native.sh` for the debug/release engine gate;
`avr8js-parity` runs its own gate over the converted contract and checks
deterministic conversion against the pinned reference. A separate `Arduino CLI
end-to-end` workflow installs a SHA256-pinned `arduino-cli` plus the pinned
`arduino:avr` core (cached) and runs the compiled sketch test against the native
backend. Node/TypeScript is used only in the parity repository, to convert and
verify scenarios, never to simulate.

`rust_port` exposes a small host-side API for running firmware outside the
converted scenarios: [`board::parse_hex`](rust_port/src/board.rs) decodes Intel
HEX, `Board::uno` wires the ATmega328P device profile, and `Board::step`
advances one instruction plus its peripheral tick. [`blink-gui/`](blink-gui/)
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

See [`rust_port/README.md`](rust_port/README.md) for the engine's checks, and
[`rust_port/specs/backend-plan.md`](rust_port/specs/backend-plan.md) for
milestones and residual limitations. Regeneration and contract checks live in
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity).
Downloaded vendor PDFs/text and build dependencies are excluded from Git; their
pinned provenance and fetch/check tools are included.

## Future electrical simulation work

The [sim2real implementation roadmap](docs/sim2real-roadmap.md) records the planned
path from today's DC operating points to backward-Euler RC/RL transients,
BJT/MOSFET models, deterministic AVR/analog time coupling, practical parasitics,
tolerances and temperature. ngspice-rs already supplies the device models and the
transient integrator on the electrical side; what remains is host-side causal
AVR/analog synchronization and a deck-level part catalogue with provenance.
These are future capabilities, not current feature claims.

## License and citation

Released under the [MIT License](LICENSE). Portions are derived from
[AVR8js](https://github.com/wokwi/avr8js) — MIT, Copyright (c) 2019-2025 Uri
Shaked — whose notice is retained in [`rust_port/LICENSE`](rust_port/LICENSE) and
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity) and
reproduced in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). Microchip
datasheets referenced under `rust_port/specs/` keep their vendor terms and are
not redistributed. The Arduino AVR core and `arduino-cli` used by the test suite
are installed at test time and are not part of this repository either.

To cite this project, use [`CITATION.cff`](CITATION.cff).
