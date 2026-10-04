# Arduino/breadboard sim2real implementation roadmap

**Status: proposed future work.** This document records the direction and
acceptance criteria; it does not imply the features below are implemented.

## Goal and architectural principle

Target Arduino/ELEGOO-style breadboard circuits, not industrial SPICE accuracy:

> MNA + Newton–Raphson + backward-Euler transient integration + moderately
> realistic diode/LED, BJT and MOSFET models, with source impedance and tolerances.

**Nonlinear DC solving is necessary, but not sufficient for sim2real.** Nonlinear
solving handles nonlinear device equations; transient integration handles
memory/energy-storage devices. A circuit containing both needs a nonlinear solve
inside each transient timestep, plus models appropriate to the operating range.
Matching a numerical reference alone does not establish hardware fidelity.

Prioritize RC charging/discharging, diode/LED behavior, transistor switching,
finite source impedance and component variation before detailed semiconductor
physics. The current resistor limits current; an active voltage/current regulator
would require its own device/control model and operating limits.

## Current baseline — implemented, not a future promise

| Area | Current implementation | Missing capability |
| --- | --- | --- |
| Linear MNA | Nonground node voltages, voltage-source current unknowns, ideal independent voltage/current sources, dense row-equilibrated partial-pivot elimination | Controlled sources, generalized multi-terminal stamps, reusable compiled topology |
| Nonlinear DC | Two-terminal `Device::evaluate(V)` returning current and analytic conductance; Newton with voltage-step limiting and residual backtracking | Transistor models, warm-start API, source/gmin continuation |
| Components | Ohmic resistor; illustrative red Shockley LED with explicit leakage shunt | Calibrated part models, capacitors, inductors, BJTs, MOSFETs, temperature evolution |
| Diagnostics | Signed branch voltage/current/power; KCL/source-voltage residuals; singular/nonfinite/nonconvergence errors | Per-device limiting diagnostics, transient error estimates and rejection history |
| AVR coupling | GUI-owned fixed netlist; GPIO mode sampled every 32 instructions; 5 V/0 V driver through 25 Ω, 30 kΩ pull-up; cached DC solutions | Timestamped pin events, transient synchronization, ADC electrical coupling, loaded output readback |
| Input feedback | Valid low/high voltages feed the AVR pin; indeterminate threshold band retains the previous digital sample | Explicit board-specific threshold/hysteresis profiles and crossing-time handling |

Source of truth:

- [`analog-solver/src/lib.rs`](../analog-solver/src/lib.rs)
- [`circuit-components/src/lib.rs`](../circuit-components/src/lib.rs)
- [`blink-gui/src/analog.rs`](../blink-gui/src/analog.rs)
- [`blink-gui/src/sim.rs`](../blink-gui/src/sim.rs)

Existing gates:

```sh
bash analog-solver/tools/verify-analog.sh
bash rust_port/tools/verify-native.sh
```

Keep the present forward/reverse LED, independent scalar-root, derivative,
KCL/KVL/power, solver-failure and compiled-Blink tests as regression fixtures.
The default LED parameters and GPIO resistance are illustrative, not measured
ATmega328P or commercial LED characterizations.

## Crate boundaries and proposed architecture

Preserve the dependency-free AVR parity core and its offline gate:

- **`analog-solver`** owns topology, unknown allocation, matrix assembly, linear
  algebra, Newton convergence, analysis sessions and time integration.
- **`circuit-components`** owns electrical constitutive laws, derivatives,
  charges/fluxes, validated parameters, model metadata and component defaults.
  No renderer or AVR dependencies.
- **Host/board adapter** owns MCU pin drivers, board rails, threshold/ADC rules,
  timestamped external events and analog/digital synchronization. It currently
  lives in `blink-gui`; extract a headless adapter crate when transient coupling
  needs reuse. Do not move electrical dependencies into `rust_port`.
- **GUI** observes accepted simulation snapshots and sends configuration/input
  events. Rendering frames must not determine electrical integration steps.

Conceptual analysis pipeline (not an already implemented API):

```text
Circuit definition + device parameters
                  |
         compile topology/unknowns
                  |
          device model evaluation
                  |
      +-----------+------------------+
      |           |                  |
      DC          transient          AC small signal (later)
      |           |                  |
      |      integration/history     linearize at a DC bias
      |           |                  |
      +---- reusable Newton -----+   complex linear solve
                  |              |
          linear matrix solve    |
                  |              |
        accepted operating point / waveform / diagnostics
```

Before transistors/dynamics, replace the strictly two-terminal interface with a
model/stamping contract that can declare terminals, additional branch/internal
unknowns, terminal-current residuals, their full Jacobian and dynamic charge/flux
contributions. MOSFET gate dependence and BJT base/collector dependence require
cross-terminal derivatives; transistor Jacobians need not be symmetric.
Retain a two-terminal convenience adapter for existing resistors and LEDs.

Keep immutable circuit/model parameters separate from **committed analysis
state** and **trial Newton/timestep state**. Device evaluation during Newton is
side-effect-free: failed iterations cannot change capacitor charge, inductor
current, temperature history or committed simulation time.

## Milestone 1 — generalize linear MNA and device stamps

- [ ] Separate unknown allocation/topology compilation, residual/Jacobian
  assembly and the linear solve from the current DC loop.
- [ ] Add VCCS, VCVS, CCCS and CCVS controlled sources with documented polarity,
  control references and any required sensing/branch-current unknowns.
- [ ] Introduce multi-terminal stamps and circuit-local validated identities;
  detect invalid references and explain singular/contradictory constraints.
- [ ] Reuse compiled topology and matrix storage across solves. Keep dense
  elimination initially; add sparse backends only after circuit-size profiling.
- [ ] Make voltage/current unknown scaling and pivot/conditioning diagnostics
  explicit; do not assume a symmetric matrix.

**Acceptance:** analytical resistor/source and controlled-source networks match;
branch orientation, KCL/KVL and power conventions remain consistent; floating
nodes and contradictory ideal sources fail explicitly. Existing DC APIs retain
an adapter or have a documented migration, with no AVR core dependency changes.

## Milestone 2 — reusable nonlinear solving and robust diode bias points

- [ ] Extract Newton into a reusable kernel for DC and timestep equations, with
  initial guesses/warm starts, tolerances, iteration limits and diagnostics.
- [ ] Preserve analytic Jacobians, voltage limiting and residual backtracking;
  test terminal derivatives by independent finite differences.
- [ ] Add optional source stepping and gmin stepping for difficult bias points.
  Record continuation settings and intermediate failures. These are numerical
  aids, not undocumented physical resistors.
- [ ] After continuation, verify the **original requested circuit** at its actual
  sources and without artificial gmin. Never report success for only a modified
  circuit. Keep real modeled leakage separate from numerical regularization.
- [ ] Report model validity/overflow and distinguish singular topology from
  exhausted nonlinear convergence. Multiple operating points remain possible.

**Acceptance:** diode/LED sweeps and harder nonlinear networks converge within
residual tolerances or return reproducible errors; independent roots/reference
solutions agree. No small-step-only convergence or silent current/voltage clamp.

## Milestone 3 — backward-Euler transient RC/RL/RLC behavior

This is the first major sim2real vertical slice: **charging is not a sequence of
DC cache hits**, even when the AVR output never changes.

At DC steady state, a capacitor is open and an ideal inductor is a zero-voltage
branch (not a zero-ohm resistor). DC initialization alone cannot predict startup,
filtering, delays or oscillation.

For a capacitor oriented p → n:

\[
I_C=C\frac{dV}{dt},\qquad
I_{C,n}=\frac{C}{\Delta t}(V_n-V_{n-1}).
\]

The backward-Euler companion stamp has conductance \(G=C/\Delta t\) and a
history current source \(I_h=-G V_{n-1}\), oriented p → n. Thus
\(I_{C,n}=G V_n+I_h\). Freeze accepted history throughout all Newton iterations.

For an inductor, add a branch-current unknown and enforce:

\[
V_n=\frac{L}{\Delta t}(I_n-I_{n-1}).
\]

This is an MNA branch equation, not a capacitor-style conductance substitution.
General nonlinear charge models later use the form
\(F(x,t)+[Q(x_n)-Q(x_{n-1})]/\Delta t=0\), with the charge Jacobian included
in Newton. A constant capacitance is the first implementation, not the limit
of the interface.

- [ ] Add capacitor voltage/charge and inductor current/flux histories,
  consistent signed companion stamps and transient source waveforms.
- [ ] Define initialization modes: DC operating point where one exists, or
  explicit capacitor voltages/inductor currents with consistent constraints.
  An optional supply ramp is a distinct stimulus, not an implicit assumption.
- [ ] Start with fixed-step backward Euler and a resumable transient session.
  Initialize Newton from the last accepted state; reuse Milestone 2 inside
  **every** timestep, including steps with unchanged GPIO.
- [ ] Add min/max timestep, growth/shrink bounds, maximum retries and an error
  estimate (e.g. one full step versus two half steps), separate from Newton's
  equation-residual convergence criterion.
- [ ] On failure or excessive estimated integration error, discard trial state,
  reduce the timestep and retry; never advance committed time on rejection.
- [ ] Stop at scheduled source discontinuities and requested sample times.
  Retain accepted time/step/error/Newton diagnostics for replay.
- [ ] Consider trapezoidal integration only after backward Euler is validated;
  document its numerical-ringing tradeoff and discontinuity restart policy.

**Acceptance:**

- RC charge/discharge matches \(V_C(t)=V_S(1-e^{-t/RC})\) for a zero-initial-voltage
  step, and the corresponding exponential discharge. At fixed step
  \(\Delta t\le RC/100\), sampled voltage error should be below 0.5% of the source
  step amplitude at \(RC\) and \(5RC\); halving the step demonstrates first-order
  convergence. This is an initial numerical fixture, not a hardware guarantee.
- RL current matches \(I_L(t)=(V_S/R)(1-e^{-tR/L})\); RLC behavior distinguishes
  physical damping from backward-Euler numerical damping under step refinement.
- Initial conditions, long constant-input runs and history-source polarity are
  correct. KCL/KVL hold at accepted steps. Stored-energy behavior is physically
  sensible; do not demand exact continuous-time energy conservation from a
  dissipative discretization such as backward Euler.
- Forced failures, adaptive-step rejection and full-step/two-half-step error
  trials leave committed histories/time untouched. Replaying the same settings
  produces the same accepted trace.

## Milestone 4 — transistor operating points and nonlinear transients

Once general stamps exist, DC transistor work can proceed independently of the
RC integrator. Merge the two for switching tests; neither alone is sufficient.

- [ ] Add NPN/PNP BJT models: begin with Ebers–Moll forward/reverse junctions and
  transport, then finite output resistance/Early effect and validated parameters.
  Cover cutoff, forward active, saturation and reverse operation rather than
  assuming a constant beta or a fixed 0.7 V base-emitter drop everywhere.
- [ ] Add NMOS/PMOS models with explicit terminal/polarity definitions: cutoff,
  triode and saturation regions, channel-length modulation, body effect and body
  diode. Define source/drain reversal consistently. A square-law model alone
  remains an educational tier, not a claim of fitted silicon behavior.
- [ ] Expose \(V_{GS},V_{DS},I_D\) or \(V_{BE},V_{CE},I_C\), base/gate currents and
  dissipated power. Verify terminal-current conservation and cross-derivatives.
- [ ] Add moderately realistic junction/gate capacitances and BJT stored-charge
  behavior through the dynamic contract. Document approximations; a static BJT
  plus arbitrary fixed capacitors does not reproduce all storage/switching delay.
- [ ] Solve an RC-driven transistor circuit with companion stamps and transistor
  residuals in the **same Newton system**, not sequential one-way evaluations.

**Acceptance:** DC I–V sweeps cover all supported regions and polarities; RC-fed
BJT and MOSFET switches give repeatable turn-on/off trajectories under timestep
refinement. Body/freewheel-diode tests exercise inductive switching. Reference
comparisons use equivalent documented models; validity bounds and omitted
breakdown/thermal effects are visible rather than hidden.

## Milestone 5 — deterministic AVR/analog time coupling

The current 32-instruction polling and render-frame pacing are adequate for
Blink but cannot be the timing contract for short pulses, PWM, RC threshold
crossings or ADC acquisition.

- [ ] Observe GPIO mode/value changes with AVR cycle timestamps, including
  timer overrides, and align the analog solver with those event boundaries.
  Define ordering at coincident events and instruction-cycle timing precision.
- [ ] Advance analog state in **simulated time**, independently of wall time,
  frame rate, view, pause/resume or camera controls. Express the clock bridge as
  explicit AVR cycles ↔ seconds rather than accumulated frame-time rounding.
- [ ] Add board-specific driver high/low impedances, pull-ups, high impedance,
  rails and input thresholds. Loaded output pad readback and protection paths
  need explicit adapter behavior, not changes to the AVR parity semantics.
- [ ] Feed analog threshold crossings back into digital inputs at bounded timing
  error; add hysteresis only where the board profile specifies it. Keep invalid
  voltage bands explicit. Handle feedback causally; do not accept analog steps
  past a CPU-affecting event without a synchronization/refinement strategy.
- [ ] Model ADC voltage/reference and acquisition timing through a headless
  adapter; add sample-and-hold impedance/capacitance where it matters.
- [ ] Specify analog startup/reset/history policy and electrical topology-edit
  policy. MCU reset alone must not silently discharge real storage components;
  explicit power-cycle/reset-circuit actions may have different semantics.

**Acceptance:** GPIO→RC→input threshold timing, PWM low-pass average/ripple and
ADC samples match analytic/reference traces at defined timing tolerances.
Different GUI frame rates and headless execution give the same accepted waveform;
pause advances neither clock. Faster-than-current-polling pulses are not lost.
Regression tests retain the existing AVR offline parity gate unchanged.

## Milestone 6 — practical model fidelity, parasitics and tolerances

Some of this work should start with the RC/transistor milestones, not wait until
all mechanisms are implemented.

- [ ] Introduce model/part profiles with units, parameter ranges, calibration
  temperature, provenance, version, pinout and known omissions. Fit actual
  diode/LED curves and typical kit BJTs/MOSFETs, rather than treating illustrative
  defaults as measured facts. Pinout errors are as important as numeric errors.
- [ ] Support capacitor ESR/leakage, inductor winding resistance, source/cable
  impedance, wire/breadboard/contact resistance and optional stray capacitance.
  Model junction/gate capacitance without double-counting explicit parasitics.
- [ ] Provide ideal/nominal/practical profiles so parasitic assumptions are
  inspectable. Diagnose invalid ideal topologies rather than "repairing" them
  with undisclosed resistance. Source regulation/current limits are separate
  optional models, not an interpretation of the current Thevenin resistance.
- [ ] Add component tolerance/corner analysis and seeded Monte Carlo runs. State
  distributions, bounds and parameter correlations explicitly; draw parameters
  once per run, not on every timestep. Validate positive physical values and
  persist the seed, model versions and sampled parameter set for replay.
- [ ] Compare representative breadboard measurements (DC curves, RC time
  constants, PWM ripple and switching traces) against prediction envelopes.
  Distinguish numerical error, model error and physical component variation.
- [ ] Only later consider a **documented subset** of SPICE model-card import.
  Reject unsupported parameters instead of implying full SPICE compatibility;
  retain provenance and check redistribution licenses for vendor models.

**Acceptance:** increasing ESR/contact resistance changes traces as expected;
measured data fits documented envelopes across more than a single calibration
point; identical seeds reproduce sampled values/traces. Hardware-validation
budgets are chosen per fixture and operating range, not a universal accuracy
claim. These models are not a substitute for hardware safety verification.

## Milestone 7 — static temperature first; electrothermal coupling later

- [ ] Add explicit ambient/model temperature in kelvin and reference-temperature
  metadata. Use \(V_T=k_B T/q\) together with appropriate saturation-current,
  threshold/mobility/beta and resistance temperature dependence; changing only
  diode thermal voltage is not a sufficient temperature model.
- [ ] Test supported temperature sweeps and validate ranges against available
  part data. Record temperature in every reproducible run.
- [ ] Defer self-heating/thermal storage and electrical/thermal feedback until
  justified by practical circuits; they require additional state and validated
  thermal parameters. Overstress warnings need documented part ratings even
  before a destruction model exists.

**Acceptance:** diode forward-voltage and supported transistor/resistor trends
match the model's documented range; invalid temperatures fail explicitly.
No claims of predicting thermal damage from electrical power alone.

## Milestone 8 — optional AC analysis and higher-fidelity models

Lower priority than RC and switching for the intended kit circuits.

- [ ] Linearize current and charge equations about a converged DC operating point
  and solve the complex small-signal system
  \([J_F+j\omega J_Q]\,\delta x=\delta b\).
- [ ] Document frequency, amplitude/phase and peak/RMS conventions. AC is a
  local linear approximation, not a replacement for large-signal switching.
- [ ] Add validated higher-tier transistor models, sparse matrices or more
  integration methods only when measured failures/performance justify them.

**Acceptance:** RC/RLC gain/phase matches analytical curves; biased transistor
small-signal responses match an equivalent reference model. Missing bias points
or unsupported model features produce explicit errors.

## Recommended execution order and definition of done

1. Preserve current regression fixtures; generalize MNA/stamps (Milestone 1).
2. Extract reusable Newton and initial-guess/state contracts (Milestone 2).
3. Deliver the headless backward-Euler **RC charging/discharging vertical slice**
   before expanding semiconductor detail (Milestone 3).
4. Add DC BJT/MOSFET models and their combined nonlinear transient tests
   (Milestone 4); transistor DC work may overlap step 3 after stamp contracts exist.
5. Replace GUI polling with deterministic headless AVR event/time coupling
   (Milestone 5); expose accepted waveforms/measurements in the GUI.
6. Iterate measured part profiles, source/parasitic impedance and tolerance
   envelopes (Milestone 6), then temperature (Milestone 7).
7. Treat AC and industrial-model breadth as optional extensions (Milestone 8).

For each milestone: ship headless analytical/independent-reference tests,
convergence and invalid-input tests, documented model limits and a small runnable
example before adding GUI polish. Run the electrical library gate and the AVR
parity gate separately; new analysis work must not add dependencies to the core
lockfile. Reference SPICE runs/hardware measurements can be optional fixture
capture tools, while checked-in numerical fixtures keep ordinary tests offline.

Record solver/model versions, initial conditions, stimuli, timestep/tolerance
settings, seed/parameter samples, temperature and diagnostics with captured
waveforms. A failed timestep, modified continuation circuit or unsupported device
model must never be presented as a successful hardware prediction.
