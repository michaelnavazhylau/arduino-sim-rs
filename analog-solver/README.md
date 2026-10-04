# analog-solver

Dependency-free **nonlinear DC operating-point solver**. No AVR, raylib, workspace,
SPICE executable, BLAS, or platform libraries are required. Device models live
in the sibling [`circuit-components`](../circuit-components/) crate.

## Contract

- Allocate nodes with `Circuit::node()`; `Node::GROUND` is exactly 0 V.
- Wires share a node identity. Do not emulate a wire with a zero-ohm resistor.
- Add two-terminal `Device` models, ideal current sources and voltage sources.
- Every branch has an orientation **positive terminal → negative terminal**.
  Voltage = Vp − Vn; positive current flows p → n; power = V × I.
  A supplying voltage source therefore has negative power.
- A `Device` returns `I(V)` and its analytic `dI/dV` in SI units.
- `Circuit::solve(SolveOptions::default())` returns node voltages, signed branch
  voltage/current/power, Newton step count and KCL/KVL residuals.
- IDs are circuit-local; retain only IDs allocated by that circuit. Invalid
  numeric IDs return errors/`None`, but IDs do not encode circuit ownership.

## Method

Modified nodal analysis uses nonground node voltages and voltage-source branch
currents as unknowns. Newton linearizes each device. The dense Jacobian is
row-equilibrated and solved by partial-pivot Gaussian elimination, with a
0.25 V default node-voltage step limit and residual-reducing backtracking.
Device exponential overflow trials are rejected, not silently current-clamped.

Convergence requires the **equation residuals**, not merely a small Newton
step, to meet tolerances. KCL tolerance is in amperes; voltage-source constraint
tolerance is in volts. Relative tolerance scales by branch-current magnitudes
and terminal/source voltages. `NoConvergence.residual` is a dimensionless maximum
normalized residual (converged ≤ 1), while `Solution` reports physical residuals.
Set tighter absolute current tolerance for picoampere leakage measurements.

Floating/ill-conditioned nodes, redundant or contradictory ideal sources,
invalid parameters, nonfinite model results and convergence failure are
explicit errors. Even an initially zero-residual circuit is factored to reject
floating nodes. **No hidden conductance-to-ground (gmin)** is inserted.

## Scope

Small, memoryless educational circuits. This is not a full SPICE replacement:
there is no transient integration, capacitor/inductor support, AC analysis,
source stepping, sparse matrix solver, global convergence guarantee or device
damage model. A DC nonlinear circuit can have no solution or multiple solutions;
an error must be handled rather than displaying a partial iterate.

## Roadmap

See the [sim2real implementation plan](../docs/sim2real-roadmap.md) for generalized
MNA stamps, reusable Newton solving, backward-Euler transient sessions and later
AC analysis. It distinguishes the implemented DC baseline from future milestones
and defines analytical tests, state rollback and convergence requirements.

## Checks

From the repository root, verify both headless electrical crates offline:

```sh
bash analog-solver/tools/verify-analog.sh
```

The gate covers debug/release tests, formatting, clippy and the forward/reverse
headless example. It is separate from `rust_port/tools/verify-native.sh` and does
not modify the AVR parity workflows or lockfile.

MIT; see the repository [LICENSE](../LICENSE).
