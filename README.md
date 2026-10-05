# arduino-sim-rs

A native Rust AVR simulator compatibility port, with a pinned AVR8js reference
and converted behavioral tests. No JavaScript runtime or bridge is used.

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

- [`rust_port/`](rust_port/): dependency-free Rust implementation, test harness,
  generated scenarios, conversion tools and specification provenance.
- [`blink-gui/`](blink-gui/): optional raylib GUI. It runs a real blink sketch
  on the simulator as a flat schematic or a procedural 3D rendition of the Uno,
  and runs a real HC-SR04 sketch in a **to-scale** (1 unit = 10 mm) 3D scene where
  the Uno is wired to the module with jumpers and a target cube sits at the
  distance the firmware measured. Kept outside `rust_port` so the core stays
  dependency-free and the offline parity gate is unaffected.
- [`analog-solver/`](analog-solver/): independent, dependency-free nonlinear DC
  MNA solver (node voltages, signed branch currents and power).
- [`circuit-components/`](circuit-components/): resistor and directional Shockley
  LED models plus a declarative netlist format and part catalogue, depending only
  on `analog-solver`; includes a headless example.
- [`breadboard/`](breadboard/): headless host, simulated-time event scheduler,
  analog pin/ADC coupling, I2C and SPI device attachment, and external components
  (an HC-SR04 ultrasonic distance sensor) wired to a real AVR board. Depends on
  `rust_port`; nothing in the parity core depends on it.
- [`avr8js/`](avr8js/): upstream reference submodule pinned at
  `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.

**All 347 converted scenarios pass by default**, with **zero ignored cases**
and 46 supporting tests (393 tests total). Implementation milestones 0–6 are
complete: assembler, CPU/instructions/interrupts, clock, GPIO, megaAVR timers,
ATtiny Timer1, EEPROM, ADC, SPI, USART, TWI master states and watchdog.
Passing tests establish the converted baseline, not complete AVR8js API parity
or hardware fidelity. The supporting set includes an `arduino-cli` integration
test that compiles an ATmega328P sketch and runs the produced HEX image on the
native backend (skipped when the `arduino:avr` core is unavailable).

Run `bash rust_port/tools/verify-native.sh` for the debug/release native parity
gate. GitHub Actions also checks deterministic conversion against the pinned
reference. A separate `Arduino CLI end-to-end` workflow installs a SHA256-pinned
`arduino-cli` plus the pinned `arduino:avr` core (cached) and runs the compiled
sketch test against the native backend. Node/TypeScript is used only for
conversion tooling, not simulation.

`rust_port` exposes a small host-side API for running firmware outside the
converted scenarios: [`board::parse_hex`](rust_port/src/board.rs) decodes Intel
HEX, `Board::uno` wires the ATmega328P device profile, and `Board::step`
advances one instruction plus its peripheral tick. [`blink-gui/`](blink-gui/)
is a worked example — a raylib window with a schematic view and a procedural 3D
board. `PORTB5` drives a finite-impedance source feeding a 220 Ω resistor and
LED; the external LED's brightness follows solved forward current, and reversing
it blocks conduction. The UI shows voltage/current/power. This is a nonlinear DC
model, not transient SPICE or an active current regulator. Verify the separate
electrical libraries with `bash analog-solver/tools/verify-analog.sh`.
The GUI crate links a system raylib and is
intentionally not part of the offline CI gate; see its README for requirements,
for why raylib cannot load STL, and for the `nobuild` camera-module caveat.

[`breadboard/`](breadboard/) is the headless counterpart for **time, analog
coupling and buses**: it owns simulated-time scheduling, external components, a
netlist-driven analog loop that drives `PINx` and the ADC from solved node
voltages, and I2C/SPI devices attached through the core's own callback surface.
An HC-SR04 demonstrates a sensor whose ECHO pulse is produced at cycle
granularity and read back through the AVR's `PINx` register; a sensor is a
stateful device whose output depends on *when* things happened, so it implements
a `Stimulus` rather than `analog_solver::Device`, keeping digital timing out of
the electrical solve. The analog loop is **DC only** (no RC transients), SPI has
no chip-select because the core does not model `SS`, and I2C is master-only.
Verify it with `bash breadboard/tools/verify-breadboard.sh`.

[`blink-gui/`](blink-gui/) shows that same sensor: the HC-SR04 sketch times ECHO
with `pulseIn`, publishes its own result over I2C (`Wire`) to a host sink, and a
to-scale 3D scene draws the Uno, its jumpers to the module, a 100 mm ruler track
and a target cube at that distance. Because the firmware does the measuring, the
demo exercises the AVR core, the sensor model and the I2C bridge in one path —
and the ranging tests run headlessly against the compiled firmware.

See [`rust_port/README.md`](rust_port/README.md) for checks and regeneration, and
[`rust_port/specs/backend-plan.md`](rust_port/specs/backend-plan.md) for milestones
and residual limitations. Downloaded vendor PDFs/text and build dependencies
are excluded from Git; their pinned provenance and fetch/check tools are included.

## Future electrical simulation work

The [sim2real implementation roadmap](docs/sim2real-roadmap.md) records the planned
path from today's nonlinear DC demo to backward-Euler RC/RL transients,
BJT/MOSFET models, deterministic AVR/analog time coupling, practical parasitics,
tolerances and temperature. It includes architecture boundaries and acceptance
criteria; these are future capabilities, not current feature claims.

## License and citation

Released under the [MIT License](LICENSE). Portions are derived from
[AVR8js](https://github.com/wokwi/avr8js) — MIT, Copyright (c) 2019-2025 Uri
Shaked — whose notice is retained in [`rust_port/LICENSE`](rust_port/LICENSE) and
reproduced in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). Microchip
datasheets referenced under `rust_port/specs/` keep their vendor terms and are
not redistributed. The Arduino AVR core and `arduino-cli` used by the test suite
are installed at test time and are not part of this repository either.

To cite this project, use [`CITATION.cff`](CITATION.cff).
