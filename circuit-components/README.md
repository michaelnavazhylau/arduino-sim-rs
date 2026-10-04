# circuit-components

Renderer-independent resistor and LED electrical models. The only dependency
is the sibling [`analog-solver`](../analog-solver/) crate; no AVR or GUI code.

## Models

### Resistor

`Resistor::new(ohms)` validates a finite, positive resistance. Ohm's law:

```text
I = V/R              dI/dV = 1/R              P = V*I
```

Current is bidirectional. Negative voltage gives negative current but positive
absorbed power. Zero-ohm wires are shared nodes, not resistor devices.

### LED

`Led::new(LedParameters { ... })` exposes the parameters; `Led::red()` supplies
an **illustrative red indicator LED**, not a fitted commercial device model.
Solver positive terminal = **anode**, negative terminal = **cathode**.

```text
V = Vanode - Vcathode
I = Is * (exp(V/(n*Vt)) - 1) + V/Rshunt
dI/dV = Is * exp(V/(n*Vt))/(n*Vt) + 1/Rshunt
```

Defaults: Is = 1e-20 A, n = 2, Vt = 0.02585 V, Rshunt = 1e12 Ω, nominal
optical current = 20 mA. Forward drop is approximately 2.14 V at 10 mA.
Reverse current is signed leakage (~−5 pA at −5 V), not forward conduction.
The explicit shunt represents parasitic leakage and gives an undriven LED
circuit a finite path to ground; it is not a solver-wide numerical gmin.

Evaluation uses log-domain exponentials and `expm1` near zero, plus an analytic
Jacobian. Overflow is an error, never a fabricated current cap.
`Led::brightness(current)` maps **forward** current to [0,1], normalized by the
nominal optical current. Reverse leakage never lights the LED. Brightness
clipping does not constrain the solved electrical current.

There is no avalanche/reverse breakdown, thermal evolution, capacitance,
internal series resistance, aging or destruction model. Add series resistance
as explicit branches. A real LED can be damaged by reverse bias or excess
current; this simplified model is not a hardware safety/design validator.

## Roadmap

The [sim2real implementation plan](../docs/sim2real-roadmap.md) covers capacitors
and inductors, multi-terminal BJT/MOSFET models, calibrated part profiles,
parasitics, tolerances and temperature. Those are planned extensions; the current
crate implements only the resistor and illustrative LED models described above.

## Headless example

A 5 V Thevenin source with 25 Ω output resistance drives 220 Ω and a red LED:

```sh
cargo run --manifest-path circuit-components/Cargo.toml --example blink --offline
cargo run --manifest-path circuit-components/Cargo.toml --example blink --offline -- --reverse
```

Expected forward solution: Vpin ≈ 4.709 V, Vled ≈ 2.151 V, Iled ≈ 11.630 mA.
Reversed: Vpin ≈ 5 V, Vled ≈ −5 V, Iled ≈ −5 pA and zero brightness.
This is **resistor current limiting**, not an active constant-current regulator.

The example in [`examples/blink.rs`](examples/blink.rs) shows the public API.
Tests compare MNA with an independent bisection root, check analytical
derivatives, signed currents, KCL/KVL/power conservation, reverse supply,
resistance changes and antiparallel LEDs.

```sh
bash analog-solver/tools/verify-analog.sh # from repository root
```

MIT; see the repository [LICENSE](../LICENSE).
