# arduino-sim-rs

A native Rust AVR simulator compatibility port, with a pinned AVR8js reference
and converted behavioral tests. No JavaScript runtime or bridge is used.

## Get started

```sh
git clone --recurse-submodules https://github.com/michaelnavazhylau/arduino-sim-rs.git
cd arduino-sim-rs/rust_port
cargo test --offline
```

For an existing clone, initialize the reference with
`git submodule update --init --recursive`.

## Layout and status

- [`rust_port/`](rust_port/): dependency-free Rust implementation, test harness,
  generated scenarios, conversion tools and specification provenance.
- [`avr8js/`](avr8js/): upstream reference submodule pinned at
  `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.

**283 of 347 converted scenarios pass by default**, alongside 25 supporting
tests. Assembler, CPU/instructions/interrupts, clock, GPIO, megaAVR timers and
ATtiny Timer1 are implemented. The remaining 64 scenarios for EEPROM, ADC, SPI,
USART, TWI and watchdog stay ignored until implemented. Passing tests do not
establish complete AVR8js parity or hardware fidelity.

See [`rust_port/README.md`](rust_port/README.md) for checks and regeneration, and
[`rust_port/specs/backend-plan.md`](rust_port/specs/backend-plan.md) for milestones
and residual limitations. Downloaded vendor PDFs/text and build dependencies
are excluded from Git; their pinned provenance and fetch/check tools are included.

The Rust port and derived tests retain the upstream MIT attribution in
[`rust_port/LICENSE`](rust_port/LICENSE). AVR8js retains its own license; vendor
specification documents retain their vendor licensing.
