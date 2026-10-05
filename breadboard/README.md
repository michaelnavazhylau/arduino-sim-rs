# breadboard

Headless host, simulated-time scheduler and external components for the native
AVR simulator. The only dependency is the sibling
[`rust_port`](../rust_port/) AVR core; there is no GUI or renderer here, and
nothing in this crate is a dependency of the AVR parity gate.

## Why this crate exists

A resistor or LED is a memoryless current/voltage law, which is what
`circuit-components` provides. A **sensor is not that**: it is a stateful device
whose output depends on *when* things happened. An HC-SR04 reports a distance by
holding ECHO high for a specific time, so its identity is a duration, not a
voltage.

Squeezing that into `analog-solver::Device` would force digital timing into an
electrical solve. Instead this crate adds the layer the
[sim2real roadmap](../docs/sim2real-roadmap.md) calls for: it owns the MCU pin
drivers, the simulated-time event queue and the components wired to the board,
and it depends on the AVR core rather than the other way round.

## Simulated time is the contract

Every delay is in **AVR cycles at 16 MHz**. Components never read the wall clock,
the frame rate or the scheduler's internal order. Consequences:

* A fast host, a slow host and a host that pauses and resumes produce the same
  waveform — pause advances neither clock.
* Events are ordered by `(due_cycle, scheduling order)`, so ties are replayed
  in a fixed, recorded order instead of whatever a heap or sort happened to do.
* A dispatch hands the model the cycle the event *represents*, not the cycle the
  host reached, so a step overshooting by an instruction cannot accumulate into
  the model's arithmetic.

## The `Stimulus` contract

```text
observe pins  ->  on_input(...)     (a pad from `observes()` changed level)
schedule      ->  ctx.schedule_after(cycles, payload)
drive pins    ->  drive(pin)        (queried by the host, resolved against the AVR)
```

A component may only mutate itself, schedule through `StimulusCtx`, and report a
`Drive` per pin. It never touches the AVR, the event queue or other components.
The host resolves everything before a pad is written:

| Snapshot | Meaning |
| --- | --- |
| `HighZ` | not driving |
| `PullUp` | weak high; a strong `Low` wins |
| `High` / `Low` | low-impedance output |

`High` against `Low` is a **short**, reported as `HostError::PinConflict` rather
than averaged into a plausible-looking midpoint. An AVR output already reaches
`PINx` through the port register, so external drive is only written to pads the
AVR leaves as inputs.

## The world model

Sensors measure a `Scene`: reflectors at distances with a reflectivity, plus
air temperature for the speed of sound. Time of flight — not rendering — decides
what a sensor reports. A renderer may visualise a scene, but it never defines one.
A reflector below the sensor's detection threshold, an empty scene, and a target
beyond range all read identically as *no echo*.

## Analog coupling

A digital pin is not an ideal switch, and an analog input is not a number a host
invents. `AnalogCoupling` binds named netlist nets to Uno pins and closes the
loop in both directions:

* **MCU → circuit.** Each bound pin contributes a Thevenin driver reflecting its
  current drive mode. A pin in high impedance contributes **nothing**: an
  undriven pin really is undriven, and a huge fake resistor would make a floating
  input look like a measurement.
* **Circuit → MCU.** Solved net voltages feed the ADC mux channels, and a bound
  input pin's pad voltage is resolved through the AVR input thresholds
  (`≤0.3 Vcc` low, `≥0.6 Vcc` high) and written back to `PINx`. A voltage inside
  the band is *indeterminate* and leaves the previous sample alone rather than
  rounding it to a logic level.

Topology is reused: a `Low`↔`High` change only rewrites a source value, and only
a change of *driver shape* (driven, pulled up, or absent) recompiles. A
`DIDR0`-disabled analog pin gets no digital feedback, as on real silicon.

```rust
use breadboard::netlist::{led, resistor};
use breadboard::{AnalogCoupling, Netlist, Pin, Wiring};

let netlist = Netlist::new()
    .nets(["d9", "led_a"])
    .part(resistor("R1", 220.0, "d9", "led_a"))
    .part(led("D1", "led_a", "gnd"));
let coupling = AnalogCoupling::new(netlist, Wiring::new().bind(Pin::digital(9), "d9"))
    .expect("valid coupling");
```

A netlist that has no unique solution — a divider feeding nothing, for instance —
reports `CouplingOutcome::Indeterminate` with the solver error preserved, rather
than fabricating a voltage.

## Components

### HC-SR04 ultrasonic distance sensor

Default pins `D9` (TRIG) / `D10` (ECHO).

* A TRIG high pulse of at least 10 us (20 us recommended by most guides) starts a
  measurement; a shorter one is counted in `rejected_triggers()` and ignored.
* ECHO is actively driven low when idle, rises after the response delay, and
  falls after the acoustic round trip: `2 * distance / speed_of_sound`.
* ECHO is a real pad transition, so `digitalRead`, `pulseIn`, external interrupts
  and pin-change interrupts on the AVR all see it.
* A new trigger supersedes an in-flight measurement; stale callbacks carry an old
  generation and return without touching the pin.

Honest limits: no beam angle, no multi-path reflection, no cross-talk between two
sensors, no acoustic dead zone beyond `min_range_m`, and no timeout pulse on a
missing echo (ECHO simply stays low, since real modules disagree about the
timeout width).

```rust
use breadboard::{BreadboardHost, HcSr04, Scene, Stimulus};

let host = BreadboardHost::new(
    flash,                          // an ATmega328P byte image
    Scene::wall(0.5).unwrap(),      // 50 cm away
    vec![Box::new(HcSr04::with_defaults()) as Box<dyn Stimulus>],
)
.expect("host boots");
```

## Buses

The AVR core drives its TWI and SPI peripherals, but a bus with nothing on it is
not a bus. `Bridge` attaches Rust devices to those peripherals through the core's
own callback surface, so a device is addressed and read by the same path real
firmware uses. Nothing is reimplemented: the core still owns status codes,
interrupts and timing, and `BridgeBackend` only intercepts `breadboard:`
identities while delegating every other operation untouched.

```rust
pub trait I2cSlave {
    fn address(&self) -> u8;
    fn connect(&mut self, write: bool) -> bool;  // return value is the ACK bit
    fn write(&mut self, byte: u8) -> bool;        // return value is the ACK bit
    fn read(&mut self) -> u8;
}
```

Returning `false` from `connect` or `write` NACKs on the wire, which is how
firmware discovers that nobody is home. `RegisterMap` is a ready-made I2C device
with a pointer register and auto-increment — the shape of most I2C sensors — and
`SpiRegisterMap` is the command/register SPI equivalent.

```rust
host.attach_i2c(vec![Box::new(RegisterMap::new(0x68, 8).expect("8 registers"))]);
host.attach_spi(vec![Box::new(SpiRegisterMap::new(8).expect("8 registers"))])
    .expect("one device");
```

A host can refresh a device's registers between transactions with
`host.bridge_mut().i2c_device_as_mut::<RegisterMap>(0x68)`, which is the seam a
physics model writes sensor readings through.

Honest limits: I2C is a **master-only** bus with addressable slaves, matching the
core's TWI state machine — no multi-master arbitration and no clock stretching.
SPI has **no chip-select**, because the core does not model `SS`; a slave sees
one continuous session, and a device needing framing must be delimited by the
host (see `SpiRegisterMap::end_transaction`) or by a GPIO used as CS.

## Scope and non-claims

* Analog coupling is **DC only**. There is no transient integration, so no RC
  charging, no PWM low-pass averaging, no startup behaviour and no
  sample-and-hold timing. The MCU driver impedances (25 Ω output, 30 kΩ pull-up)
  are illustrative, not measured ATmega328P pad characteristics.
* `HcSr04` is the only modelled *sensor*. The bus devices are register-file
  models, not fitted parts, and no part has been characterised against hardware.
* Components are attached in code. The netlist is data, but there is no netlist
  file format and no schematic-to-model binding.
* Timing values are illustrative, and this is not a hardware safety or design
  validator.

## Verify

```sh
bash breadboard/tools/verify-breadboard.sh
```

* `tests/ultrasonic.rs` runs a genuinely assembled AVR program and drives TRIG by
  writing the same registers a sketch would; pulse widths are compared against an
  independently recomputed time of flight.
* `tests/analog.rs` drives pins through `DDR`/`PORT` and checks the solved
  operating point against the published hand-wired values, the threshold band,
  and a real ADC conversion (2.5 V of a 5 V reference reads 512 counts).
* `tests/bus.rs` runs the TWI and SPI state machines at register level and
  checks addressing, ACK/NACK, auto-increment and register round-trips.

MIT; see the repository [LICENSE](../LICENSE).
