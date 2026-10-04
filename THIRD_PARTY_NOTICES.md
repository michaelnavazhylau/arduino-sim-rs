# Third-party notices

`arduino-sim-rs` is an independent reimplementation of AVR8js. It contains
derived work and, at test time, temporarily installs third-party toolchains.
Those components keep their own licenses. The MIT license in [`LICENSE`](LICENSE)
covers only this project's own code.

Nothing listed on this page is redistributed in this repository.

## AVR8js — derived work (MIT)

The native simulator (`rust_port/src/sim/`), the scenario runner
(`rust_port/src/runtime.rs`) and the converted behavioral suites
(`rust_port/src/suites/`) are derived from AVR8js
(<https://github.com/wokwi/avr8js>), pinned as a submodule at revision
`bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.

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

None. `rust_port` has an empty `[dependencies]` table, so no third-party Rust
code is compiled into the simulator.

## Development tooling — not distributed

Node/TypeScript is used only to regenerate the converted scenarios under
`rust_port/tools/`, pinned through `package-lock.json`. It is not part of the
simulator or of any produced artifact.
