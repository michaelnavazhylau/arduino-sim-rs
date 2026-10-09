# Third-party notices

`arduino-sim-rs` is the front-end for an independent reimplementation of AVR8js.
It redistributes no third-party source: every dependency below is fetched at
build or test time. The MIT license in [`LICENSE`](LICENSE) covers only this
project's own code.

The AVR8js derived work — the native simulator, the scenario runner and the
converted behavioral suites — no longer lives in this repository. It was split
out so that the generated scenarios and the hand-written engine stop sharing a
history and a lockfile with the front-end:

- [`avr8rs`](https://github.com/michaelnavazhylau/avr8rs) holds the simulator core
  and the scenario runner, published as
  [`avr8rs`](https://crates.io/crates/avr8rs) on crates.io.
- [`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity) holds the
  converted suites, the converter and the pinned upstream reference.

## AVR8js — derived work (MIT)

The simulator core, the scenario runner and the converted suites are derived from
AVR8js (<https://github.com/wokwi/avr8js>), pinned at revision
`bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.

Both repositories above carry this notice, and individual derived files carry an
SPDX header identifying the upstream copyright. It is reproduced here too,
because this repository's own history was derived from AVR8js and this repository
depends on that work through the published crate.

```
The MIT License (MIT)

Copyright (c) 2019-2025 Uri Shaked

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

## Rust dependencies — not redistributed

The headless [`breadboard/`](breadboard/) host and the optional
[`blink-gui/`](blink-gui/) demo pull in third-party Rust code, fetched from the
package index at build time and not part of this repository.

### avr8rs — not redistributed

[`breadboard/`](breadboard/) and [`blink-gui/`](blink-gui/) depend on
[`avr8rs`](https://crates.io/crates/avr8rs) 0.1 from crates.io, the AVR simulator
engine. It is itself a **derivative work** of AVR8js and keeps that project's MIT
license (notice above). It has an empty `[dependencies]` table of its own, so it
brings no further crates with it.

Consuming it by version means an engine change needs a version bump and a publish
before it is visible here; `avr8js-parity` keeps a path dependency instead, so it
can gate unreleased engine commits.

### ngspice-rs and its numerical dependencies — not redistributed

[`breadboard/`](breadboard/) depends on
[ngspice-rs](https://github.com/michaelnavazhylau/ngspice-rs) 0.1, a from-scratch
Rust reimplementation of the ngspice circuit simulator. It is a **derivative
work** of ngspice's C sources and keeps ngspice's Modified BSD (BSD-3-Clause)
license. There is no FFI: the C tree is used only as a read-only specification
and an out-of-process oracle, so no `libngspice` is linked and no C source is
redistributed here.

ngspice-rs in turn pulls `faer` and `faer-traits` (MIT), `diffsol`,
`diffsol-la` and `diffsol-nl` (MIT), `petgraph` (MIT OR Apache-2.0) and `winnow`
(MIT), plus their transitive dependencies, from the package index at build time.
None of them are part of this repository.

Because of that dependency, `breadboard` — and therefore `blink-gui` — is not
buildable `--offline` on a cold Cargo registry. The engine has no dependencies,
so its own offline gate is unaffected.

### raylib — not redistributed

The optional [`blink-gui/`](blink-gui/) demo pulls in
[raylib-rs](https://github.com/raysan5/raylib-rs) (`raylib` / `raylib-sys` +
~40 transitive crates), fetched from the package index at build time and not
part of this repository. raylib-rs is distributed under the Zlib license, and
raylib itself under the zlib/libpng license. `blink-gui` is configured with the
`nobuild` feature, which links the raylib already installed on the host instead
of compiling a vendored copy, so no raylib source is redistributed here.

## Notices that moved with the engine

Three sections that used to appear here now live in
[`avr8rs`](https://github.com/michaelnavazhylau/avr8rs/blob/main/THIRD_PARTY_NOTICES.md),
because the tests and references they cover moved with it:

- the `arduino:avr` boards core, installed at test time to compile the sketch in
  `tests/arduino-cli/uno_probe`;
- the SHA-256-pinned `arduino-cli` release binary (GPL-3.0) downloaded by the
  `Arduino CLI end-to-end` workflow;
- the Microchip/Atmel datasheet references under `specs/`, whose vendor terms are
  not covered by the MIT license and which are excluded from Git except for their
  published provenance in `specs/sources.json`.
