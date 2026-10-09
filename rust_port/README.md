# avr-sim — native Rust AVR simulator core

[![crates.io](https://img.shields.io/crates/v/avr-sim.svg)](https://crates.io/crates/avr-sim)
[![docs.rs](https://docs.rs/avr-sim/badge.svg)](https://docs.rs/avr-sim)
[![license](https://img.shields.io/crates/l/avr-sim.svg)](LICENSE)

A dependency-free native Rust AVR8 simulator: assembler, CPU/instructions/
interrupts, clock, GPIO, megaAVR timers, ATtiny Timer1, EEPROM, ADC, SPI, USART,
TWI master states and watchdog. Backend milestones 0–6 are complete. There is
**no JavaScript runtime and no JS bridge**.

The crate also carries the typed scenario representation (`scenario.rs`) and its
runner (`runtime.rs`) that express the port's behavior contract as Rust data
rather than embedded JavaScript. The **generated** contract is not here. The 14
converted AVR8js suites, 347 cases and 853 assertion sites live in
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity) together
with the converter that produces them and the pinned upstream revision they are
asserted against, so 27k lines of machine-written scenarios no longer share a
history or a lockfile with this crate. `avr-sim` keeps an empty `[dependencies]`
table, which is what makes the offline gate hermetic and the crate publishable.

## Hardware and compatibility specifications

Official AVR instruction and ATmega328P/ATtiny85/ATmega2560 datasheets have been
pulled into `specs/`, with pinned hashes, searchable text, and a reproducible
fetch/check tool. See [specs/README.md](specs/README.md) for the section map and
[specs/backend-plan.md](specs/backend-plan.md) for implementation milestones and
hardware-vs-AVR8js compatibility differences. The vendor PDFs/text are local,
Git-ignored reference files; the provenance and tooling remain versionable.

## What runs today

```sh
cd rust_port
cargo test --offline
cargo fmt --check
cargo clippy --all-targets --offline -- -D warnings
```

**45 tests pass by default** in both debug and release: 4 assembler unit tests,
12 harness regressions, 8 native regressions, 5 peripheral-recovery regressions,
15 peripheral edge/reentry regressions and the toolchain-gated `arduino_cli`
end-to-end check.

`tests/arduino_cli.rs` compiles [`tests/arduino-cli/uno_probe`](tests/arduino-cli/uno_probe)
with `arduino-cli` for `arduino:avr:uno` and executes the resulting HEX image on
the native backend, exercising the real ATmega328P register and interrupt-vector
map. It skips when the `arduino:avr` core is unavailable, so the default suite
stays hermetic; set `AVR_SIM_REQUIRE_ARDUINO_CLI=1` to make a missing toolchain a
failure. The LED/millis timing assertion needs a multi-second simulation and runs
in release builds.

A green build validates this crate's own regressions, **not complete AVR8js API
parity or hardware correctness**. Unsupported constructors and methods fail
explicitly rather than fabricating simulator behavior.

```sh
cargo test -- --list                       # discover every test
cargo test --test native                  # arithmetic flags, reentry, panic recovery
cargo test --test peripherals             # peripheral edge/reentry regressions
cargo test --test peripheral_recovery     # panic recovery and callback ordering
cargo test --test harness                 # runner and harness regressions
AVR_SIM_REQUIRE_ARDUINO_CLI=1 cargo test --release --test arduino_cli  # real HEX image
bash tools/verify-native.sh               # full debug/release engine gate
```

There are no Cargo dependencies. Node/TypeScript is not needed to compile or
execute anything here; it is used only in `avr8js-parity`, to regenerate and
verify that repository's converted scenarios.

## Layout

- `src/sim/`: assembler, CPU/executor, clock, GPIO, timers, EEPROM, ADC, SPI,
  USART, TWI and watchdog. `peripheral_adapter.rs` handles registry/callback
  ownership; `peripheral.rs` supplies common event and interrupt dispatch.
- `src/runtime.rs`: native Rust scenario runner, lexical callback captures, mock
  call recording/reset, assertion evaluation, typed-array fixtures, assembly
  helpers, and instruction-runner helpers. It implements only the test-language
  subset; it does not evaluate JS text or emulate AVR hardware. It is also the
  `Backend` contract that `breadboard` and `blink-gui` drive in production.
- `src/scenario.rs`: typed test operations, expressions, and assertion definitions.
- `src/board.rs`: host-side glue for running a real firmware image — Intel HEX
  parsing and ATmega328P device wiring — without reimplementing AVR behavior.
- `src/lib.rs`: `native_backend()` constructs a fresh native simulator adapter.
- `tests/harness.rs`: executable harness and runner regressions.
- `tests/native.rs`: arithmetic flags, synchronous callback reentry, panic recovery,
  hook fallback/chaining and Tiny PWM regression tests.
- `tests/peripherals.rs`, `tests/peripheral_recovery.rs`: EEPROM protection,
  ADC references/differential conversion, serial timing/masking, watchdog modes,
  synchronous peripheral/CPU reentry, call-through spies and panic restoration.
- `tests/arduino_cli.rs`: compiles `tests/arduino-cli/uno_probe` with arduino-cli
  and runs the produced ATmega328P HEX image on the native backend (see above).
- `tools/verify-native.sh`: fail-closed engine gate.
- `tools/specs.py`: fetches and verifies the pinned vendor datasheets in `specs/`.

Scenarios are deliberately declarative rather than tied to speculative Rust CPU
structs or `Rc<RefCell<CPU>>` ownership. The port can use a system/bus, owned
peripherals, arenas, or another architecture without rewriting the behavioral
contract. API spellings in scenarios retain the upstream names for traceability;
only the native adapter needs to map them to idiomatic Rust APIs. This is not a
completed set of tests directly calling a yet-to-be-defined typed simulator API.

## Extend the native port

`sim::Simulator` implements `runtime::Backend` and is returned by
`native_backend()` in `src/lib.rs`. Extend its supported operations incrementally:

| Operation | Responsibility |
| --- | --- |
| `resolve(name)` | Imported configs/enums and free-function identities |
| `construct(runtime, kind, args)` | CPU, EEPROM memory, clock, GPIO, timers, ADC, SPI, TWI, USART, watchdog |
| `get(handle, key)` | Properties, indexed register/memory views, original callable methods |
| `set(runtime, handle, key, value)` | Raw assignments and callback installation |
| `call(runtime, receiver, name, args)` | Native methods or free `assemble`, `avrInstruction`, `avrInterrupt` |
| `properties(handle)` | Configuration fields for spread/override fixtures |

`Value::Handle` is a backend-owned identity; choose the backing storage freely.
Config/enum values can also be returned as test-owned `Value::Object` values.
Return `Value::Native(name)` for native free functions, and
`Value::Method(Box::new(Value::Handle(handle)), method_name)` for original methods.

Important semantic requirements:

1. **Memory aliasing and hooks:** `cpu.data`, `dataView`, `progMem`, and `progBytes`
   must address the same native CPU memory. Indexed `set` is a *raw* assignment;
   `writeData`/`readData` invoke peripheral hooks. Do not conflate them.
2. **Typed arrays and assembly:** CPU constructors receive `Value::Buffer` with
   16-bit little-endian words. `assemble` returns an object containing a width-1
   byte Buffer, errors array, lines array (objects with `byteOffset`, `bytes`,
   `line`, `text`), and labels object. `asmProgram` checks errors and builds words
   explicitly from little-endian bytes, retaining labels and instruction count.
3. **Callbacks:** Store the actual callback `Value` (identity matters for removing
   listeners/events) and invoke `runtime.invoke(callback, args)` at the native
   event point. Captures remain mutable, and assertions can execute inside them.
   Release internal borrow/lock guards before invoking: callbacks can reenter the
   same CPU or peripheral synchronously.
4. **Spies:** `vi.spyOn` installs a recording wrapper but **calls through** to the
   original native method; do not replace it with a no-op. Property reads must
   return installed wrappers; native method dispatch invokes the original method
   body, not recursively re-read the wrapper. `mockClear` keeps implementation;
   `mockReset` removes it, matching the source tests.
5. **Timing:** Preserve explicit cycle jumps, instruction/tick ordering, delayed
   event scheduling, pending interrupts, and all intermediate observations. A
   `tick` may dispatch callbacks synchronously before returning.
6. **Runner behavior:** `TestProgramRunner` retains the upstream default BREAK
   exception, BREAK-before-predicate ordering, and 5,000-iteration timeout. Its
   optional BREAK callback is not silently discarded. The harness also has an
   overall evaluation budget to catch accidental infinite loops.
7. **Unsupported operations fail:** Unknown globals/fields/methods must fail
   visibly, never fabricate expected values. Harness fixtures are not native AVR
   implementations and must not be used as the real backend.

Every suite in the converted contract is currently enabled. When extending the
port, the contract's own gate rejects any converted scenario that is `#[ignore]`d,
so coverage cannot silently shrink; `tools/verify-native.sh` does the same here
and uses `--include-ignored` in both builds as defense in depth.

Known groundwork limitations include adapter `Value` types in native event/hook
storage, incomplete hook introspection, constructor-buffer sharing and untested
parser/memory-boundary behavior. See the backend plan for remaining architecture
and fidelity work.

## Contract coverage

The engine satisfies every suite below. `avr8js-parity` holds the source of truth
for the inventory — `conversion-manifest.json` records the upstream SHA-256 per
file, and the generated modules themselves are the cases.

| Suite | Cases | Assertions |
| --- | ---: | ---: |
| CPU | 6 | 9 |
| Instructions | 97 | 337 |
| Interrupts | 2 | 14 |
| ADC | 2 | 4 |
| Clock | 8 | 12 |
| EEPROM | 10 | 29 |
| GPIO | 22 | 64 |
| SPI | 11 | 32 |
| ATtiny timer | 10 | 12 |
| Timers | 55 | 167 |
| TWI | 7 | 19 |
| USART | 28 | 52 |
| Watchdog | 6 | 19 |
| Assembler | 83 | 83 |
| **Total** | **347** | **853** |

Counts refer to static assertion sites, including callbacks; dynamic invocation
counts can differ. Each case gets fresh setup and inherited `beforeEach` hooks.
Assembly templates, config overrides, issue-regression names, and numeric/floating
expectations are preserved, including source typos and unusual assembler inputs.

## Upstream reference and regeneration

Baseline upstream revision: `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`. This
repository pins it as an uninitialized `avr8js/` submodule for provenance: it
records which upstream revision the port's compatibility claims are written
against, and `specs/backend-plan.md` cites its sources by path. Nothing here
builds or executes it.

Regeneration is owned by
[`avr8js-parity`](https://github.com/michaelnavazhylau/avr8js-parity), which
pins the same revision as a live submodule, carries `tools/convert.cjs`, and
verifies determinism and source hashes with `npm run check`. Derived tests retain
upstream attribution and the MIT license in `LICENSE`.
