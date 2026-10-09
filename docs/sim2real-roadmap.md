# Arduino/breadboard sim2real implementation roadmap

**Status: mixed.** The electrical engine decision below is implemented. The
host-side coupling, model-provenance and fidelity work is **proposed future
work** and is not implied to exist.

## Decision: ngspice-rs owns the electrical engine

The electrical side of this project is delegated to
[ngspice-rs](https://github.com/michaelnavazhylau/ngspice-rs), a from-scratch
Rust port of ngspice with no C dependency and no FFI. It supplies the netlist
front end, MNA assembly, Newton convergence with continuation, diode/BJT/MOS1
models and the `.op`/`.dc`/`.ac`/`.tran` drivers. Reimplementing those here was
the original plan; it is no longer the direction.

That division leaves this repository owning the parts ngspice-rs cannot know
about:

- the AVR core and its **simulated cycle time**;
- the board's pin drivers, rails, thresholds and ADC rules;
- the **causal** bridge between AVR cycle time and electrical time;
- component provenance, pinout and honest model limits;
- rendering and result presentation.

The architectural principle is therefore:

> Delegate device equations, nonlinear solving and integration to ngspice-rs.
> Own the *causal* AVR/analog coupling, the board model and the provenance.
> Never let rendering cadence drive electrical time.

Prioritize, in order: deterministic AVR/analog time coupling, then board/part
profiles with real provenance, then parasitics/tolerances, then temperature,
then analysis breadth in the GUI. Semiconductor physics is no longer on the
critical path.

## Current baseline — implemented

| Area | Current implementation | Missing capability |
| --- | --- | --- |
| Electrical engine | [`ngspice-rs`](https://github.com/michaelnavazhylau/ngspice-rs) 0.1, called through [`breadboard/src/spice.rs`](../breadboard/src/spice.rs): deck in, `.op` solved, node voltages and source currents out | Deck-level part catalogue, transient drives, `@device` observability (unported upstream) |
| Undetermined circuits | Nets with no DC path to ground are detected topologically and reported as `OpError::FloatingNets`, so `gmin` cannot pass an undriven net off as a voltage | Richer convergence diagnostics from the engine |
| LED model | A SPICE diode (`IS`, `N`) plus an explicit 1 TΩ shunt, at a circuit temperature that reproduces the project's illustrative 25.85 mV thermal voltage, in [`breadboard/src/led.rs`](../breadboard/src/led.rs) | Fitted colour-bin data and a documented validity range |
| AVR coupling | [`breadboard/src/analog.rs`](../breadboard/src/analog.rs): a Thevenin driver per bound pin, a declared switch as one finite-resistance element, solved node voltages pushed to the ADC mux channels, cached operating points | Timestamped pin events, transient synchronization, sample-and-hold/impedance effects, loaded output readback |
| Input feedback | Resolved pad voltages feed `PINx` through AVR thresholds; the indeterminate band retains the previous sample; `DIDR0` is honoured | Board-specific threshold/hysteresis profiles and crossing-time handling |
| Buses | [`bus.rs`](../breadboard/src/bus.rs): I2C master with addressable Rust slaves and real ACK/NACK, plus a single SPI device; core status codes, interrupts and timing are unchanged | Multi-master arbitration, clock stretching, SPI framing/chip-select, transfers paced by real bus rates |
| GUI | [`blink-gui/`](../blink-gui/) observes accepted operating points only; rendering never triggers a solve | Transient waveforms, AC sweeps |

Source of truth:

- [`breadboard/src/spice.rs`](../breadboard/src/spice.rs) (deck ↔ solve ↔ results)
- [`breadboard/src/analog.rs`](../breadboard/src/analog.rs) (deck ↔ pin/ADC coupling)
- [`breadboard/src/led.rs`](../breadboard/src/led.rs) (illustrative LED presets)
- [`breadboard/src/host.rs`](../breadboard/src/host.rs) (digital external-event coupling)
- [`breadboard/src/scheduler.rs`](../breadboard/src/scheduler.rs)
- [`breadboard/src/bus.rs`](../breadboard/src/bus.rs) (I2C/SPI device attachment)
- [`breadboard/src/ultrasonic.rs`](../breadboard/src/ultrasonic.rs)
- [`blink-gui/src/analog.rs`](../blink-gui/src/analog.rs)

Existing gates:

```sh
bash rust_port/tools/verify-native.sh          # avr-sim engine and its regressions
bash breadboard/tools/verify-breadboard.sh     # host, scheduler, analog coupling, buses
```

The converted AVR8js contract has its own gate in the separate
[avr8js-parity](https://github.com/michaelnavazhylau/avr8js-parity) repository:
it rejects any converted scenario that is `#[ignore]`d, then runs the contract in
debug and release, and separately verifies deterministic conversion and source
hashes against the pinned upstream submodule.

Keep the solved forward/reverse LED operating points, the Ohm's-law and diode
curve cross-check, the switch divider predictions, the threshold band and the
compiled-firmware tests as regression fixtures. The default LED parameters and
GPIO resistance are illustrative, not measured ATmega328P or commercial LED
characterizations.

## Boundaries

- **`avr-sim`** (`rust_port/`) keeps an empty `[dependencies]` table and its
  offline gate. Nothing electrical may move into it. It also no longer carries
  the generated AVR8js contract: that moved to
  [avr8js-parity](https://github.com/michaelnavazhylau/avr8js-parity), so 27k
  lines of generated scenarios cannot add a dependency or a lockfile entry here,
  and regeneration never lands in this repository's history.
- **`breadboard`** owns MCU pin drivers, board rails, threshold/ADC rules,
  timestamped external events and the analog/digital bridge. It owns the only
  ngspice-rs dependency.
- **`blink-gui`** observes accepted snapshots and sends configuration/input
  events. Rendering frames must not determine electrical integration steps.
- **ngspice-rs** is not vendored or wrapped in a second abstraction that
  pretends to be a general simulator: decks are the interface, and what it does
  not support is reported rather than approximated.

## Milestone 1 — deterministic AVR/analog time coupling

The 32-instruction polling and render-frame pacing are adequate for Blink but
cannot be the timing contract for short pulses, PWM, RC threshold crossings or
ADC acquisition. This is the hard part of the remaining work, and it is required
regardless of which engine integrates the circuit.

- [ ] Observe GPIO mode/value changes with AVR cycle timestamps, including timer
  overrides, and align the electrical solve with those event boundaries. Define
  ordering at coincident events and the instruction-cycle timing precision.
- [ ] Advance electrical state in **simulated time**, independently of wall
  time, frame rate, view, pause/resume or camera controls. Express the clock
  bridge as explicit AVR cycles ↔ seconds, never accumulated frame-time rounding.
- [ ] Feed threshold crossings back into digital inputs at bounded timing error.
  Keep invalid voltage bands explicit and handle feedback causally: do not accept
  an integration step past a CPU-affecting event without synchronization.
- [ ] Define what a `.op` cache hit means for a circuit with storage. A capacitor
  keeps charging when the GPIO state does not change, so the current
  "unchanged drive mode ⇒ reuse the reading" rule must **not** govern transient
  advancement.
- [ ] Define reset/pause/resume policy for committed electrical state. An MCU
  reset must not silently discharge real storage components; an explicit
  power-cycle action may have different semantics.
- [ ] Model ADC voltage/reference and acquisition timing through a headless
  adapter; add sample-and-hold impedance/capacitance where it matters.

**Acceptance:** GPIO → RC → input-threshold timing, PWM low-pass average/ripple
and ADC samples match analytic or captured reference traces at documented
tolerances. Different GUI frame rates and headless execution give the same
accepted waveform; pause advances neither clock. Pulses shorter than the current
polling interval are not lost. Regression tests keep the offline engine gate and
the converted contract's gate unchanged.

RC analytic checks remain the first vertical slice: for a zero-initial-voltage
step, \(V_C(t) = V_S(1-e^{-t/RC})\) should be reproduced to within 0.5% of the
source step at \(RC\) and \(5RC\) at a fixed step \(\Delta t \le RC/100\), with
first-order convergence under step halving, and the corresponding discharge.
RL and RLC follow; RLC behaviour must distinguish physical damping from
integrator damping under step refinement.

## Milestone 2 — board and part profiles with provenance

- [ ] Add board profiles: driver high/low impedances, pull-up values, rails,
  input thresholds and hysteresis, ADC reference and acquisition parameters.
  Loaded output pad readback and protection paths belong here, not in the AVR
  parity semantics.
- [ ] Introduce part/model profiles with units, parameter ranges, calibration
  temperature, **provenance**, version, pinout and known omissions. Fit actual
  diode/LED curves and typical kit BJTs/MOSFETs instead of treating illustrative
  defaults as measured facts. A pinout error is as important as a numeric error.
- [ ] Diagnose invalid ideal topologies rather than "repairing" them with
  undisclosed resistance. Source regulation/current limits are separate optional
  models, not an interpretation of the current Thevenin resistance.
- [ ] Keep real modeled leakage separate from numerical regularization. Never
  report success for a circuit modified by continuation or `gmin`.

**Acceptance:** every shipped profile names its source and validity range;
changing a documented parameter changes the trace in the documented direction;
unsupported model features fail explicitly rather than being approximated.

## Milestone 3 — parasitics, tolerances and corner analysis

- [ ] Support capacitor ESR/leakage, inductor winding resistance, source/cable
  impedance, wire/breadboard/contact resistance and optional stray capacitance.
  Model junction/gate capacitance without double-counting explicit parasitics.
- [ ] Provide ideal/nominal/practical profiles so parasitic assumptions are
  inspectable.
- [ ] Add component tolerance/corner analysis and seeded Monte Carlo runs. State
  distributions, bounds and parameter correlations explicitly; draw parameters
  once per run, not per timestep. Persist the seed, model versions and sampled
  values for replay.

**Acceptance:** increasing ESR/contact resistance changes traces as expected;
identical seeds reproduce sampled values and traces exactly.

## Milestone 4 — temperature

- [ ] Expose ambient/model temperature and reference-temperature metadata as
  explicit run settings, and keep them out of hidden defaults. Device temperature
  dependence itself comes from ngspice-rs models, including the diode's
  \(V_T = k_B T / q\); the project's LED presets currently pin the circuit
  temperature to reproduce an illustrative 25.85 mV thermal voltage.
- [ ] Test supported temperature sweeps and validate ranges against available
  part data; record the temperature in every reproducible run.
- [ ] Defer self-heating and electrothermal feedback until justified by practical
  circuits: they need additional state and validated thermal parameters.

**Acceptance:** diode forward-voltage and supported transistor/resistor trends
match the model's documented range; invalid temperatures fail explicitly. No
claims of predicting thermal damage from electrical power alone.

## Milestone 5 — analysis breadth in the GUI

Lower priority than the coupling and the profiles, and gated on both.

- [ ] Expose `.tran` waveforms for RC charging, PWM averaging and switching, once
  the coupling can advance electrical time causally (Milestone 1).
- [ ] Expose `.ac` sweeps for RC/RLC and biased small-signal response, with
  frequency, amplitude/phase and peak/RMS conventions documented. AC is a local
  linear approximation at a converged operating point, not a substitute for
  large-signal switching.
- [ ] Present accepted waveforms and diagnostics, never trial or rejected steps.

**Acceptance:** RC/RLC gain/phase matches analytical curves; biased small-signal
responses match an equivalent reference model; unsupported analyses produce
explicit errors.

## Execution order and definition of done

1. Causal AVR/analog time coupling, with the headless RC vertical slice
   (Milestone 1).
2. Board and part profiles with provenance (Milestone 2).
3. Parasitics, tolerances and corners (Milestone 3), then temperature
   (Milestone 4).
4. GUI waveforms and sweeps last (Milestone 5).

For each milestone: ship headless analytical or independent-reference tests,
explicit invalid-input tests, documented model limits and a small runnable
example before GUI polish. Run the electrical gate and the AVR engine gate
separately; new analysis work must not add dependencies to the core lockfile.
Captured reference data may be produced by tools, but ordinary tests must not
require a C ngspice build.

Record engine/model versions, initial conditions, stimuli, timestep and
tolerance settings, seeds, temperature and diagnostics with captured waveforms.
A failed step, a modified continuation circuit or an unsupported device model
must never be presented as a successful hardware prediction.
