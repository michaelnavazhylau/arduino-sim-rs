# blink-gui

A raylib front-end for the native AVR simulator in `../rust_port`. The simulator
runs a **real Arduino blink sketch** built by `arduino-cli` for an ATmega328P;
a separate nonlinear DC solver computes the external circuit's voltages and
currents, and raylib draws the result. Two views share one operating point:

| View | What it shows | Key |
| --- | --- | --- |
| **Schematic** | Flat external LED, lit by solved forward current | `v` / `tab` |
| **3D board** | Uno plus wired 220 Ω resistor and directional external LED | `v` / `tab` |

Both external-LED views use `Sim::analog.brightness()`. The onboard LED remains
an independent digital GPIO indicator; reversing the external LED makes it dark
without changing firmware or the onboard indicator.

```
  3D board view                                     Schematic view
 ┌────────────────────────────────────┐      ┌──────────────────────────────┐
 │ ATmega328P blink                   │      │ ATmega328P blink             │
 │ firmware: arduino-cli / avr:uno    │      │   ┌──────────────────────┐   │
 │        ╔═══╗  ▄▄▄▄  ██████         │      │   │        ( ● )         │   │
 │   ▮▮▮  ║USB║  ▓▓   ██████   ▮▮▮▮   │      │   │ External LED: A -> K │   │
 │        ╚═══╝  ▓▓   ██████   ▮▮▮▮   │      │   └──────────────────────┘   │
 │   ══════════════════════════════   │      │                              │
 │ sim time        4817 ms            │      │ sim time        4817 ms      │
 │ cycles         77074368       1.00x│      │ cycles         77074368 1.00x│
 │ 3D board view - space pause ...    │      │ schematic - space pause ...  │
 └────────────────────────────────────┘      └──────────────────────────────┘
```

## Why it lives here, not in `rust_port`

This crate is deliberately **outside** the `rust_port` package:

- `rust_port` stays dependency-free, so its `--locked --offline` CI gate
  (`tools/verify-native.sh`, `native-parity.yml`, `arduino-cli.yml`) keeps
  passing unchanged. Adding raylib there would pull roughly 40 crates into that
  lockfile and require vendoring for the offline jobs.
- The raylib dependency is confined to a demo, not the parity contract.
- Nothing here is part of the AVR8js compatibility surface. All simulator access
  goes through `avr_port_tests::board`.

For the same reason this crate is not wired into CI: the parity runners are
headless Ubuntu images, and raylib needs a GPU/display context.

## Requirements

A **system** raylib 5.5 or newer. The dependency is declared as
`default-features = false, features = ["nobuild"]`, which links the library
already on the machine instead of compiling the vendored copy with cmake — so
`cmake` is *not* needed.

```sh
brew install raylib              # macOS
sudo apt install libraylib-dev   # Debian/Ubuntu
```

If your raylib lives somewhere unusual, extend the linker search path in
[`.cargo/config.toml`](.cargo/config.toml), which already covers the Homebrew
prefixes for both macOS architectures.

> **Version note.** `raylib-rs` 6.0 generates its bindings from raylib 6.0
> headers, while Homebrew currently ships the 5.5 dylib. That combination works
> for every call this demo makes, but for an exact match either
> `brew install cmake` and drop `nobuild`, or pin the crate to the `5.5` line.

## Run

```sh
cd blink-gui
cargo run --release
```

Use `--release`. The simulator is roughly 10x slower in a debug build and the
blink will visibly lag.

| Input | Action |
| --- | --- |
| `space` | pause / resume |
| `r` | reset the simulated board, preserving external LED polarity |
| `p` | reverse the external LED's anode/cathode connections |
| `←` / `→` (or `↑` / `↓`) | simulated clock rate, 0.25x to 8x |
| `v` or `tab` | switch between schematic and 3D board |
| left-drag | orbit the 3D camera |
| wheel | zoom the 3D camera |

At **1.00x** the firmware runs in real time. Budgeting is done in simulated
*cycles*, not instructions: this firmware retires about 0.77 instructions per
cycle, so stepping a fixed instruction count would run ~1.3x fast.

### Smoke test without watching the window

`BLINK_FRAMES=N` exits after `N` rendered frames and prints a summary;
`BLINK_VIEW=3d` starts in the 3D view. `BLINK_REVERSED=1` starts with the LED
reversed. The final summary includes signed LED voltage/current and DC solves.

```sh
BLINK_FRAMES=600 ./target/release/blink-gui
# executed 123331360 instructions (160015880 cycles), 21 LED toggles, 10001 ms simulated
BLINK_VIEW=3d BLINK_REVERSED=1 BLINK_FRAMES=75 ./target/release/blink-gui
# D13 high, but external Iled ~-5e-12 A and brightness=0
```

At 1.00x, 600 frames should report roughly 10000 ms of simulated time and about
20 toggles for a 500 ms half-period.

## Firmware

`firmware/blink.hex` is committed so the demo needs no toolchain, and is embedded
with `include_str!` so it works from any working directory. It is built from
[`sketches/blink/blink.ino`](sketches/blink/blink.ino):

```sh
arduino-cli compile -b arduino:avr:uno --build-path build sketches/blink
cp build/blink.ino.hex firmware/blink.hex
```

Everything is loaded through `avr_port_tests::board::parse_hex`, which rejects a
bad checksum or an over-long image rather than fabricating flash contents.

## How the simulator is driven

```rust
let backend: Rc<dyn Backend> = native_backend();
let mut runtime = Runtime::new(backend.clone());
let board = Board::uno(backend.as_ref(), &mut runtime, parse_hex(FIRMWARE));
board.step(backend.as_ref(), &mut runtime);          // one instruction + peripheral tick
board.led_builtin(backend.as_ref(), &mut runtime)    // PORTB5 as a PinState
```

`Board::step` pairs `avrInstruction` with `tick`; the latter dispatches timer
overflows and interrupts, so `delay()`/`millis()` only work if it runs.

Long runs are the reason `Runtime::reset_budget` exists: timer interrupts burn the
runner's per-case callback budget, and a GUI must refill it rather than rebuild
the `Runtime` on every frame. See `Board::BUDGET_REFILL_CYCLES`.

## The 3D rendition

The board is **procedural**: three unit meshes raylib generates (cube, cylinder,
sphere) instanced with `draw_model_ex` transforms, in `board3d.rs`. Scale is
1.0 = 10 mm, so the 68.6 × 53.4 mm PCB is 6.86 × 5.34 and header pitch is
2.54 mm = 0.254.

### External analog blink circuit

```text
AVR Thevenin driver -- D13 -- red jumper -- 220 ohm -- orange jumper -- LED A
GND ------------------------ dark ground jumper -------------------- LED K
```

The electrical graph lives in `src/analog.rs`, using the independent
[`analog-solver`](../analog-solver/) and
[`circuit-components`](../circuit-components/) crates:

- Output high: 5 V source through 25 Ω; output low: 0 V through 25 Ω.
- Input: driver disconnected. Input pull-up: 5 V through 30 kΩ.
- The resistor uses Ohm's law; the LED uses a directional Shockley I(V) curve
  with explicit parasitic leakage. Node voltages and source/device currents
  are solved with nonlinear MNA, not assigned from a GPIO boolean.
- Forward: Vpin ≈ 4.709 V, Vled ≈ 2.151 V, Iled ≈ 11.630 mA.
  Reversed with D13 high: Vled ≈ −5 V, Iled ≈ −5 pA and zero brightness.
- The HUD shows signed voltage/current, absorbed/supplied power and KCL error.
  Solver failures appear as errors, not stale lit LEDs or partial solutions.

GPIO drive mode is sampled every 32 AVR instructions, using the core's pin
state API (including DDR, pull-ups and timer overrides). A changed state/polarity
triggers a solve; unchanged states reuse the solution. There is no analog cost
per rendered mesh. This sampling is adequate for Blink, but can miss shorter
pulses and does not average PWM. On inputs only, voltage ≤0.3 Vcc feeds a low
into the AVR and ≥0.6 Vcc feeds high; the band between thresholds is explicitly
indeterminate and retains the previous digital sample. Output readback remains
the AVR core's digital state, not a load-dependent pad-voltage measurement.

The axial resistor has red/red/brown/gold bands (220 Ω, ±5%, nominal value only).
The external LED has separate leads, flange, body and dome. Its anode terminal
has a green marker; the dark cathode marker and leads swap when `p` is pressed,
even while paused. The displayed digital header is ordered D8..D13, GND, AREF,
SDA, SCL; wires end on D13 and GND. Start with
`BLINK_VIEW=3d cargo run --release`.

**Model limits:** this is a memoryless DC solve, not full SPICE. No capacitors,
inductors, dynamic transients, diode breakdown, thermal damage, MCU protection
diodes or regulator/current-limit behavior. The source resistance and red LED
parameters are illustrative, not hardware-characterized. The series resistor
limits current; there is no active constant-current regulator. The onboard
indicator and power LED are decorative/digital, outside this analog netlist.
The fixed topology is not an interactive circuit editor.

### Why not STL

**raylib has no STL loader at all.** Its entire `LoadModel` dispatch is:

```c
.obj  .iqm  .gltf  .glb  .vox  .m3d
```

Worse, it fails *silently* rather than erroring: `load_model("x.stl")` returns
`Ok` with `meshCount == 0` plus a few `WARNING:` log lines, so you get a valid
`Model` that draws nothing. Convert to OBJ/GLB first (`assimp export in.stl
out.obj`, Blender, `trimesh`) — or, as here, build the geometry in code and skip
the binary asset, the conversion step and the model licence entirely.

### Why there is a custom shader

raylib's built-in shader is unlit, and every mesh raylib generates carries
normals but **no** vertex colours (`colors().len() == 0` for `gen_mesh_cube`,
`_cylinder` and `_sphere`), so there is nothing to bake shading into and no
in-box lighting shader. `shading.rs` supplies two short GLSL 330 programs that
use the normals raylib already provides to add one diffuse term. raylib passes
`matNormal` as the inverse-transpose of the model matrix, so the heavily
non-uniform scale of the PCB still shades correctly.

### Why the camera is hand-rolled

raylib-rs gates its **entire** camera module behind
`#[cfg(not(feature = "nobuild"))]` — under `nobuild` there is no `Camera3D`,
`Camera2D`, `UpdateCamera` or mouse-orbit helper. Every symbol that module needs
is nonetheless present in the system raylib (checked with `nm`), so the gate is
conservative rather than a real limitation. `camera.rs` is a ~60-line orbit
camera that builds the raw `raylib::ffi::Camera3D` struct itself; `projection` is
a plain `i32` in the raylib-sys 6.0 bindings rather than the enum.

## Layout

| File | Role |
| --- | --- |
| `src/main.rs` | window, input, view switching, main loop |
| `src/sim.rs` | AVR stepping, GPIO sampling and analog input feedback |
| `src/analog.rs` | electrical netlist, driver models and cached operating points |
| `src/board3d.rs` | procedural Uno geometry and LED rendering |
| `src/circuit3d.rs` | jumper wires, banded resistor, current-lit LED and polarity markers |
| `src/schematic.rs` | flat 2D view |
| `src/hud.rs` | stats overlay and control hint, shared by both views |
| `src/shading.rs` | the diffuse shader |
| `src/camera.rs` | orbit camera on raw `ffi::Camera3D` |

## Future sim2real work

See the [implementation roadmap](../docs/sim2real-roadmap.md) for transient RC
behavior, transistor switching, timestamped AVR coupling and model validation.
It also defines why rendering cadence must not drive electrical timesteps and
why a board reset should not implicitly erase stored capacitor/inductor state.
The current GUI remains the fixed nonlinear DC demonstration described above.

## Checks

```sh
# From blink-gui (requires installed system raylib, but no display for unit tests):
cargo fmt --check
cargo clippy --all-targets --release --locked --offline -- -D warnings
cargo test --release --locked --offline

# From repository root (headless and no raylib required):
bash analog-solver/tools/verify-analog.sh
bash rust_port/tools/verify-native.sh
```

GUI tests exercise compiled Blink firmware, finite output impedance, signed
forward/reverse current, input/pull-up thresholds, polarity changes while paused
and solution caching. The electrical libraries additionally check KCL/KVL,
power balance, an independent scalar root and explicit solver error cases.

## Licence

MIT, matching the rest of the repository. raylib is linked, not redistributed;
see [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
