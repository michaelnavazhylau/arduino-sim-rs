# Third-party notices

`arduino-sim-rs` is an independent reimplementation of AVR8js. It contains
derived work and, at test time, temporarily installs third-party toolchains.
Those components keep their own licenses. The MIT license in [`LICENSE`](LICENSE)
covers only this project's own code.

Nothing listed on this page is redistributed in this repository.

## AVR8js — derived work (MIT)

The native simulator (`rust_port/src/sim/`) and the scenario runner
(`rust_port/src/runtime.rs`) are derived from AVR8js
(<https://github.com/wokwi/avr8js>), pinned as a submodule at revision
`bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.

The converted behavioral suites — the largest derived work — live in the separate
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity)
repository, which pins the same upstream revision as a live submodule and carries
its own copy of the upstream notice.

The upstream notice is retained below and in [`rust_port/LICENSE`](rust_port/LICENSE),
and individual derived files carry an SPDX header identifying the upstream
copyright.

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

## Arduino AVR boards core (`arduino:avr`) — not redistributed

The end-to-end test in `rust_port/tests/arduino_cli.rs` installs the
`arduino:avr` platform through `arduino-cli` in order to compile the sketch in
`rust_port/tests/arduino-cli/uno_probe`. The platform is fetched from Arduino's
package index at test time and is not part of this repository. It is distributed
under Arduino's own license terms; see the installed package for the applicable
license.

## arduino-cli — not redistributed

The `Arduino CLI end-to-end` workflow downloads a SHA-256-pinned `arduino-cli`
release binary. It is licensed under the GNU General Public License v3.0; the
release archive ships the GPL-3.0 text as `LICENSE.txt`. It is not part of this
repository.

## Microchip / Atmel documents — not redistributed

`rust_port/specs/` cites official Microchip ATmega and ATtiny datasheets and the
AVR Instruction Set Manual as hardware references. These documents retain their
own vendor copyright and terms and are **not** covered by this project's MIT
license. They are excluded from Git; only their provenance — URL, revision, page
count and SHA-256 hash — is published, in
[`rust_port/specs/sources.json`](rust_port/specs/sources.json).

## Rust dependencies

The `avr-sim` crate in [`rust_port/`](rust_port/) has an empty `[dependencies]`
table, so no third-party Rust code is compiled into the AVR simulator, its
scenario runner or its gate.

The headless [`breadboard/`](breadboard/) host and the optional
[`blink-gui/`](blink-gui/) demo do pull in third-party Rust code, both fetched
from the package index at build time and not part of this repository.

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

Because of that dependency, `breadboard` — and therefore `blink-gui` — is no
longer buildable `--offline` on a cold Cargo registry. `rust_port` keeps its
empty `[dependencies]` table, so the offline engine gate is unaffected.

### raylib — not redistributed

The optional [`blink-gui/`](blink-gui/) demo pulls in
[raylib-rs](https://github.com/raysan5/raylib-rs) (`raylib` / `raylib-sys` +
~40 transitive crates), fetched from the package index at build time and not
part of this repository. raylib-rs is distributed under the Zlib license, and
raylib itself under the zlib/libpng license. `blink-gui` is configured with the
`nobuild` feature, which links the raylib already installed on the host instead
of compiling a vendored copy, so no raylib source is redistributed here.

## Development tooling — not distributed

Node/TypeScript is used only to regenerate the converted scenarios, in the
separate [`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity)
repository, pinned through `tools/package-lock.json`. It is not part of the
simulator, of the contract that is compiled, or of any produced artifact.
