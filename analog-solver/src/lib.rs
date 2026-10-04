// SPDX-License-Identifier: MIT

//! Nonlinear DC operating points via modified nodal analysis (MNA).
//!
//! Nodes are measured against [`Node::GROUND`]. Each branch is oriented from
//! its positive terminal to its negative terminal: voltage is Vp - Vn and
//! positive current flows p -> n. Voltage-source current is an MNA unknown,
//! so a supplying source reports negative power, not a fabricated zero.
//!
//! Models live outside this crate and implement [`Device`]. Newton iterations
//! use a pivoted dense solve, voltage-step limiting and residual backtracking.
//! Singular circuits and nonconvergence are errors. No hidden gmin resistors
//! are inserted, and no capacitors, inductors or transient integrator exist.

use std::error::Error;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node(pub usize);
impl Node {
    pub const GROUND: Self = Self(0);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BranchId(pub usize);

/// Device I(V) and its analytic derivative, both in SI units.
#[derive(Clone, Copy, Debug)]
pub struct Linearization {
    pub current: f64,
    pub conductance: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModelError {
    InvalidParameter(&'static str),
    OutOfRange(&'static str),
}
impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device model: {self:?}")
    }
}
impl Error for ModelError {}

/// Memoryless two-terminal device; no dependence on renderer or AVR state.
pub trait Device {
    fn evaluate(&self, voltage: f64) -> Result<Linearization, ModelError>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum SolveError {
    InvalidNode(Node),
    InvalidBranch(BranchId),
    InvalidValue(&'static str),
    Model { branch: BranchId, error: ModelError },
    NonFinite,
    Singular,
    NoConvergence { iterations: usize, residual: f64 },
}
impl fmt::Display for SolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DC solve: {self:?}")
    }
}
impl Error for SolveError {}

/// Absolute tolerances use separate units for KCL (A) and source KVL (V).
#[derive(Clone, Copy, Debug)]
pub struct SolveOptions {
    pub max_iterations: usize,
    pub current_tolerance: f64,
    pub voltage_tolerance: f64,
    pub relative_tolerance: f64,
    pub max_voltage_step: f64,
    pub backtracking_steps: usize,
}
impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            max_iterations: 200,
            current_tolerance: 1e-12,
            voltage_tolerance: 1e-9,
            relative_tolerance: 1e-8,
            max_voltage_step: 0.25,
            backtracking_steps: 40,
        }
    }
}
impl SolveOptions {
    fn validate(self) -> Result<Self, SolveError> {
        for (name, value) in [
            ("current tolerance", self.current_tolerance),
            ("voltage tolerance", self.voltage_tolerance),
            ("maximum voltage step", self.max_voltage_step),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(SolveError::InvalidValue(name));
            }
        }
        if !self.relative_tolerance.is_finite() || !(0.0..1.0).contains(&self.relative_tolerance) {
            return Err(SolveError::InvalidValue("relative tolerance"));
        }
        if self.max_iterations == 0 || self.backtracking_steps == 0 {
            return Err(SolveError::InvalidValue("iteration limits"));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BranchPoint {
    pub voltage: f64,
    pub current: f64,
}
impl BranchPoint {
    /// Positive = absorbed power; negative = supplied power, in watts.
    pub fn power(self) -> f64 {
        self.voltage * self.current
    }
}

#[derive(Clone, Debug)]
pub struct Solution {
    voltages: Vec<f64>,
    branches: Vec<BranchPoint>,
    pub iterations: usize,
    /// Maximum absolute KCL imbalance across nonground nodes, in amperes.
    pub max_kcl_residual: f64,
    /// Maximum voltage-source constraint error, in volts.
    pub max_voltage_residual: f64,
}
impl Solution {
    pub fn voltage(&self, node: Node) -> Option<f64> {
        self.voltages.get(node.0).copied()
    }
    pub fn branch(&self, id: BranchId) -> Option<BranchPoint> {
        self.branches.get(id.0).copied()
    }
}

enum Kind {
    Device(Box<dyn Device>),
    Current(f64),
    Voltage(f64),
}
struct Branch {
    p: Node,
    n: Node,
    kind: Kind,
}

/// Allocate nodes before adding branches. Wires are shared node identities,
/// not near-zero resistors. A zero-ohm resistor is not a supported wire model.
pub struct Circuit {
    nodes: usize,
    branches: Vec<Branch>,
}
impl Default for Circuit {
    fn default() -> Self {
        Self::new()
    }
}
impl Circuit {
    pub fn new() -> Self {
        Self {
            nodes: 1,
            branches: Vec::new(),
        }
    }
    pub fn node(&mut self) -> Node {
        let node = Node(self.nodes);
        self.nodes += 1;
        node
    }
    fn check_node(&self, node: Node) -> Result<(), SolveError> {
        if node.0 >= self.nodes {
            Err(SolveError::InvalidNode(node))
        } else {
            Ok(())
        }
    }
    fn push(&mut self, p: Node, n: Node, kind: Kind) -> Result<BranchId, SolveError> {
        self.check_node(p)?;
        self.check_node(n)?;
        let id = BranchId(self.branches.len());
        self.branches.push(Branch { p, n, kind });
        Ok(id)
    }
    pub fn device(
        &mut self,
        p: Node,
        n: Node,
        device: impl Device + 'static,
    ) -> Result<BranchId, SolveError> {
        self.push(p, n, Kind::Device(Box::new(device)))
    }
    /// An ideal current source delivering `amps` from p to n.
    pub fn current_source(&mut self, p: Node, n: Node, amps: f64) -> Result<BranchId, SolveError> {
        if !amps.is_finite() {
            return Err(SolveError::InvalidValue("source current"));
        }
        self.push(p, n, Kind::Current(amps))
    }
    /// An ideal source constraining Vp - Vn to `volts`.
    pub fn voltage_source(&mut self, p: Node, n: Node, volts: f64) -> Result<BranchId, SolveError> {
        if !volts.is_finite() {
            return Err(SolveError::InvalidValue("source voltage"));
        }
        self.push(p, n, Kind::Voltage(volts))
    }
    pub fn set_source(&mut self, id: BranchId, value: f64) -> Result<(), SolveError> {
        if !value.is_finite() {
            return Err(SolveError::InvalidValue("source value"));
        }
        let branch = self
            .branches
            .get_mut(id.0)
            .ok_or(SolveError::InvalidBranch(id))?;
        match &mut branch.kind {
            Kind::Voltage(v) | Kind::Current(v) => *v = value,
            Kind::Device(_) => return Err(SolveError::InvalidValue("branch is not a source")),
        }
        Ok(())
    }

    pub fn solve(&self, options: SolveOptions) -> Result<Solution, SolveError> {
        let options = options.validate()?;
        let voltages = self.nodes - 1;
        let sources = self
            .branches
            .iter()
            .filter(|b| matches!(b.kind, Kind::Voltage(_)))
            .count();
        let mut x = vec![0.0; voltages + sources];
        let mut system = self.assemble(&x, options)?;
        for iteration in 0..=options.max_iterations {
            // Factor even a zero-residual system: a floating node at zero volts
            // must not masquerade as a unique solution.
            let rhs: Vec<f64> = system.residual.iter().map(|v| -v).collect();
            let delta = linear_solve(system.jacobian.clone(), rhs)?;
            if system.norm <= 1.0 {
                return self.solution(&x, iteration, &system);
            }
            if iteration == options.max_iterations {
                break;
            }
            let voltage_step = delta[..voltages]
                .iter()
                .fold(0.0_f64, |m, v| m.max(v.abs()));
            let mut damping = if voltage_step > options.max_voltage_step {
                options.max_voltage_step / voltage_step
            } else {
                1.0
            };
            let mut accepted = None;
            for _ in 0..options.backtracking_steps {
                let candidate: Vec<f64> = x
                    .iter()
                    .zip(&delta)
                    .map(|(v, dv)| v + damping * dv)
                    .collect();
                match self.assemble(&candidate, options) {
                    Ok(next) if next.norm < system.norm || next.norm <= 1.0 => {
                        accepted = Some((candidate, next));
                        break;
                    }
                    // Out-of-range exponential trials are rejected, not clamped
                    // to a fake constant-current device.
                    Err(SolveError::Model {
                        error: ModelError::OutOfRange(_),
                        ..
                    })
                    | Err(SolveError::NonFinite)
                    | Ok(_) => {}
                    Err(error) => return Err(error),
                }
                damping *= 0.5;
            }
            if let Some((candidate, next)) = accepted {
                x = candidate;
                system = next;
            } else {
                return Err(SolveError::NoConvergence {
                    iterations: iteration + 1,
                    residual: system.norm,
                });
            }
        }
        Err(SolveError::NoConvergence {
            iterations: options.max_iterations,
            residual: system.norm,
        })
    }

    fn assemble(&self, x: &[f64], options: SolveOptions) -> Result<System, SolveError> {
        if x.iter().any(|v| !v.is_finite()) {
            return Err(SolveError::NonFinite);
        }
        let count = x.len();
        let node_unknowns = self.nodes - 1;
        let mut jacobian = vec![vec![0.0; count]; count];
        let mut residual = vec![0.0; count];
        let mut scales = vec![0.0; count];
        let mut source = node_unknowns;
        for (id, branch) in self.branches.iter().enumerate() {
            let p = branch.p.0.checked_sub(1);
            let n = branch.n.0.checked_sub(1);
            let vp = p.map_or(0.0, |i| x[i]);
            let vn = n.map_or(0.0, |i| x[i]);
            let voltage = vp - vn;
            let current = match &branch.kind {
                Kind::Device(device) => {
                    let iv = device
                        .evaluate(voltage)
                        .map_err(|error| SolveError::Model {
                            branch: BranchId(id),
                            error,
                        })?;
                    if !iv.current.is_finite() || !iv.conductance.is_finite() {
                        return Err(SolveError::NonFinite);
                    }
                    stamp_g(&mut jacobian, p, n, iv.conductance);
                    iv.current
                }
                Kind::Current(amps) => *amps,
                Kind::Voltage(volts) => {
                    if let Some(p) = p {
                        jacobian[p][source] += 1.0;
                        jacobian[source][p] += 1.0;
                    }
                    if let Some(n) = n {
                        jacobian[n][source] -= 1.0;
                        jacobian[source][n] -= 1.0;
                    }
                    residual[source] = voltage - volts;
                    scales[source] = vp.abs().max(vn.abs()).max(volts.abs());
                    let current = x[source];
                    source += 1;
                    current
                }
            };
            if let Some(p) = p {
                residual[p] += current;
                scales[p] += current.abs();
            }
            if let Some(n) = n {
                residual[n] -= current;
                scales[n] += current.abs();
            }
        }
        if residual.iter().chain(scales.iter()).any(|v| !v.is_finite())
            || jacobian.iter().flatten().any(|v| !v.is_finite())
        {
            return Err(SolveError::NonFinite);
        }
        let mut norm = 0.0_f64;
        for (i, value) in residual.iter().enumerate() {
            let abs = if i < node_unknowns {
                options.current_tolerance
            } else {
                options.voltage_tolerance
            };
            let tolerance = abs + options.relative_tolerance * scales[i];
            if !tolerance.is_finite() {
                return Err(SolveError::NonFinite);
            }
            norm = norm.max(value.abs() / tolerance);
        }
        Ok(System {
            jacobian,
            residual,
            norm,
        })
    }

    fn solution(
        &self,
        x: &[f64],
        iterations: usize,
        system: &System,
    ) -> Result<Solution, SolveError> {
        let mut voltages = vec![0.0];
        voltages.extend_from_slice(&x[..self.nodes - 1]);
        let mut source = self.nodes - 1;
        let mut branches = Vec::new();
        for (id, branch) in self.branches.iter().enumerate() {
            let voltage = voltages[branch.p.0] - voltages[branch.n.0];
            let current = match &branch.kind {
                Kind::Device(device) => {
                    device
                        .evaluate(voltage)
                        .map_err(|error| SolveError::Model {
                            branch: BranchId(id),
                            error,
                        })?
                        .current
                }
                Kind::Current(amps) => *amps,
                Kind::Voltage(_) => {
                    let current = x[source];
                    source += 1;
                    current
                }
            };
            branches.push(BranchPoint { voltage, current });
        }
        let maximum = |slice: &[f64]| slice.iter().fold(0.0_f64, |max, v| max.max(v.abs()));
        Ok(Solution {
            voltages,
            branches,
            iterations,
            max_kcl_residual: maximum(&system.residual[..self.nodes - 1]),
            max_voltage_residual: maximum(&system.residual[self.nodes - 1..]),
        })
    }
}
struct System {
    jacobian: Vec<Vec<f64>>,
    residual: Vec<f64>,
    norm: f64,
}

fn stamp_g(j: &mut [Vec<f64>], p: Option<usize>, n: Option<usize>, g: f64) {
    if let Some(p) = p {
        j[p][p] += g;
    }
    if let Some(n) = n {
        j[n][n] += g;
    }
    if let (Some(p), Some(n)) = (p, n) {
        j[p][n] -= g;
        j[n][p] -= g;
    }
}

/// Row equilibration plus partial-pivot Gaussian elimination. No external
/// BLAS/LAPACK dependency; intended for small educational circuits.
fn linear_solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Result<Vec<f64>, SolveError> {
    let n = b.len();
    for row in 0..n {
        let scale = a[row].iter().fold(0.0_f64, |max, v| max.max(v.abs()));
        if scale == 0.0 {
            return Err(SolveError::Singular);
        }
        for v in &mut a[row] {
            *v /= scale;
        }
        b[row] /= scale;
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&a_row, &b_row| a[a_row][col].abs().total_cmp(&a[b_row][col].abs()))
            .unwrap();
        if a[pivot][col].abs() <= 1e-15 {
            return Err(SolveError::Singular);
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let factor = a[row][col] / a[col][col];
            a[row][col] = 0.0;
            let (prefix, remaining) = a.split_at_mut(row);
            for (entry, pivot_entry) in remaining[0][col + 1..]
                .iter_mut()
                .zip(&prefix[col][col + 1..])
            {
                *entry -= factor * pivot_entry;
            }
            b[row] -= factor * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let tail: f64 = (row + 1..n).map(|col| a[row][col] * x[col]).sum();
        x[row] = (b[row] - tail) / a[row][row];
    }
    if x.iter().any(|v| !v.is_finite()) {
        Err(SolveError::NonFinite)
    } else {
        Ok(x)
    }
}
