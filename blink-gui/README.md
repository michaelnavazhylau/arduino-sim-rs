# blink-gui

A raylib front-end for the native AVR simulator in `../rust_port`. Two demos
share one window: the simulator runs a **real Arduino sketch** built by
`arduino-cli` for an ATmega328P, and raylib only observes it.

* **Blink** — a separate nonlinear DC solver computes the external circuit's
  voltages and currents, and the external LED is lit by *solved* forward current.
* **Ultrasonic ranging** — a real HC-SR04 sketch pulses TRIG, times ECHO and
  reports its measurement over I2C. The 3D view is **to scale** (1 unit = 10 mm)
  and shows the Uno, the jumpers and the module, with a target cube at the
  distance the **firmware measured**.

| View | What it shows | Key |
| --- | --- | --- |
| **Schematic** | Flat external LED, lit by solved forward current | `v` / `tab` |
| **3D board** | Uno plus wired 220 Ω resistor and directional external LED | `v` / `tab` |
| **Ultrasonic ranging** | To-scale Uno wired to an HC-SR04, a 100 mm ruler track and a target cube at the measured distance | `v` / `tab` |

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
| `r` | reset the board (blink) or reboot the firmware keeping the reflector (ranging) |
| `p` | reverse the external LED's anode/cathode connections (blink views only) |
| `←` / `→` (or `↑` / `↓`) | simulated clock rate, 0.25x to 8x |
| `v` or `tab` | cycle schematic, 3D board and ultrasonic ranging |
| `-` / `=` (or `[` / `]`) | move the ultrasonic target nearer / further |
| left-drag | orbit the 3D camera |
| wheel | zoom the 3D camera |

At **1.00x** the firmware runs in real time. Budgeting is done in simulated
*cycles*, not instructions: this firmware retires about 0.77 instructions per
cycle, so stepping a fixed instruction count would run ~1.3x fast.

### Smoke test without watching the window

`BLINK_FRAMES=N` exits after `N` rendered frames and prints a summary;
`BLINK_VIEW=3d` or `=sensor` selects the starting presentation.
`BLINK_REVERSED=1` starts with the LED reversed, `SENSOR_DISTANCE_M=1.2` sets the
reflector's starting position, and `BLINK_SCREENSHOT=shot.png` writes the final
rendered frame — which is how the 3D views are reviewed without a human at the
window. The capture must happen outside the draw handle, because raylib batches
2D draws and only flushes them in `EndDrawing`. The blink summary includes signed
LED voltage/current and DC solves; the ranging summary includes the reflector's
true distance, the firmware's measurement, the echo width and the sequence count.

```sh
BLINK_FRAMES=600 ./target/release/blink-gui
# executed 123331360 instructions (160015880 cycles), 21 LED toggles, 10001 ms simulated
BLINK_VIEW=3d BLINK_REVERSED=1 BLINK_FRAMES=75 ./target/release/blink-gui
# D13 high, but external Iled ~-5e-12 A and brightness=0
BLINK_VIEW=sensor SENSOR_DISTANCE_M=0.6 BLINK_FRAMES=150 BLINK_SCREENSHOT=shot.png ./target/release/blink-gui
# reflector 0.600 m | firmware measured Some(0.596) | echo 3477 us, status 0, ...
```

At 1.00x, 600 frames should report roughly 10000 ms of simulated time and about
20 toggles for a 500 ms half-period.

## Firmware

`firmware/blink.hex` and `firmware/ultrasonic.hex` are committed so the demos need
no toolchain, and are embedded with `include_str!` so they work from any working
directory. They are built from [`sketches/blink/blink.ino`](sketches/blink/blink.ino)
and [`sketches/ultrasonic/ultrasonic.ino`](sketches/ultrasonic/ultrasonic.ino):

```sh
arduino-cli compile -b arduino:avr:uno --build-path build sketches/blink
cp build/blink.ino.hex firmware/blink.hex

arduino-cli compile -b arduino:avr:uno --build-path build sketches/ultrasonic
cp build/ultrasonic.ino.hex firmware/ultrasonic.hex
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

## The ultrasonic ranging demo

### What is actually measured

The point of this demo is that **the firmware does the measuring**, and the
number on screen is its result rather than the sensor model's opinion:

```text
D9 TRIG  (AVR output) ──> HC-SR04 model ──> ECHO (real pad transition)
                                                  │
                        pulseIn(ECHO, HIGH, 30 ms) ─┘   firmware times the pulse
                                                  │
                        mm = echo_us * 343 / 2000 ─┘   firmware does the physics
                                                  │
                        I2C write to 0x68 ─────────┘   firmware publishes
                                                  │
                        host reads the sink ───────┘   overlay + cube position
```

The HC-SR04 is a `Stimulus` in `../breadboard`, so its ECHO is a scheduled,
cycle-accurate pad transition that the AVR sees through `PINx` exactly as a real
one would. `pulseIn` then measures that transition with its own 16-cycle assembly
loop, and the sketch converts to millimetres with its own constant.

### The telemetry channel

A real board would print over `Serial`, but this simulator has no console
attached, so the sketch publishes to an I2C "host sink" that the host implements
as a register-map device at address `0x68`. This has a useful side effect: the
demo drives the real Arduino `Wire` library through the whole TWI state machine,
so the ranging tests double as an end-to-end check of the I2C bridge. Register
layout (auto-incrementing pointer):

| Register | Meaning |
| --- | --- |
| 0 | status: 0 = ok, 1 = no echo inside the timeout, 2 = out of range |
| 1 | wrapping measurement counter |
| 2-3 | distance in millimetres, big endian |
| 4-5 | ECHO pulse width in microseconds, big endian |

### Measured agreement

Observed over 0.1-2 m, real-time at 1.00x:

| Reflector | Firmware measured | Error |
| --- | --- | --- |
| 0.100 m | 0.099 m | −1 mm |
| 0.600 m | 0.596 m | −4 mm |
| 2.000 m | 1.985 m | −15 mm |

Both numbers are shown side by side on purpose; the gap is not smoothed away. It
comes from two documented sources: `pulseIn`'s 16-cycle loop quantisation, and
the sketch's 343 m/s constant against the model's temperature-dependent
343.42 m/s at 20 °C. The tests assert an accuracy band rather than equality, and
one of them asserts the published pulse width and published distance are
mutually consistent using the *sketch's* arithmetic — so the demo cannot pass by
copying the scene's distance into the sink.

### Rendering scale

**This scene is to scale: one unit is 10 mm, exactly as in the board view.** The
Uno is 68.6 x 53.4 mm, the HC-SR04 module is 45 x 20 mm with a 2.54 mm header
pitch, the jumpers are 1.5 mm across, the track is a 40 mm lane with 100 mm ruler
ticks, and the target sits at its true distance.

That has an honest consequence worth stating plainly. At 4 m the target is 400
units away — **58 board lengths** — so the board, the module and the wiring are
a few pixels across, because they really are that small against the distance
being measured. Roughly 0.1-0.4 m is where the rig reads best; the overlay
carries the numbers at every distance. Because the scene spans three orders of
magnitude, the camera re-frames whenever you move the target, which resets the
wheel zoom.

There is no ground grid in this view for the same reason: a 10 mm grid across a
400-unit scene is 400 lines of moire. The ruler track carries the scale instead.

### Wiring

The Uno is not re-modelled here. `Board3D::draw_board` draws the *same* geometry
with an offset, and `board3d::header_pin(Header, index)` reports where each
illustrated header pin actually is, so the jumpers land on real pins rather than
on guessed coordinates.

| Jumper | Arduino pin | Module pin |
| --- | --- | --- |
| TRIG | digital header, `D9` | 1 |
| ECHO | digital header, `D10` | 2 |
| GND | digital header, `GND` | 3 |
| VCC | power header, `5V` | 0 |

`TRIG`, `ECHO` and `GND` come off the digital header, which faces the module.
`VCC` has to come from the power header on the far edge, so it is routed around
the board's left side rather than across it — which is what you would do with a
real jumper. The module sits on a blue PCB so it stays legible against the Uno's
teal one, and the ranging overlay is laid out to the right so the rig on the left
of the frame is never covered by a panel.

### Model limits

No beam angle, no multi-path reflection, no cross-talk between two sensors, no
acoustic dead zone beyond the module's `min_range_m`, and no timeout pulse: an
undetectable target simply leaves ECHO low, because real modules disagree about
the timeout width. TWI is master-only, matching the core's state machine, so
there is no arbitration or clock stretching. The 343 m/s constant is a textbook
value rather than a calibration, and nothing here is a substitute for measuring
a real module.

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
| `src/sensor_sim.rs` | ultrasonic demo: firmware boot, HC-SR04, I2C telemetry readback |
| `src/sensor3d.rs` | to-scale ranging rig: module, Uno placement, jumpers, ruler track and target |
| `src/wires3d.rs` | cylinder/sphere segment primitives shared by both 3D views |
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
and solution caching. The ranging demo additionally boots its own compiled
firmware headlessly and asserts on real `pulseIn` measurements, the published
pulse-versus-distance self-consistency, no-echo handling, the proximity
indicator and publication-gap accounting — **none of which needs a display**.
The electrical libraries additionally check KCL/KVL, power balance, an
independent scalar root and explicit solver error cases.

## Licence

MIT, matching the rest of the repository. raylib is linked, not redistributed;
see [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
