# Native Rust test contract for the AVR port

All **14 AVR8js TypeScript test suites**, **347 test cases**, and **853 assertion
sites** have been converted into typed Rust scenarios. The original `../avr8js/`
implementation and tests are untouched. The native Rust backend now implements
assembler, CPU/instructions/interrupts, clock, GPIO, megaAVR timers, ATtiny
Timer1, EEPROM, ADC, SPI, USART, TWI master states and watchdog. Milestones 0–6
are complete. There is **no JavaScript runtime or JS bridge**.

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

**All 347 converted scenarios run and pass by default** (14 suites), with
**zero ignored cases**. Another 4 assembler unit tests, 13 harness/coverage tests,
8 original native regressions, 20 peripheral/recovery regressions and the
toolchain-gated `arduino_cli` end-to-end check pass: **393 tests total** in both
debug and release builds.

`tests/arduino_cli.rs` compiles [`tests/arduino-cli/uno_probe`](tests/arduino-cli/uno_probe)
with `arduino-cli` for `arduino:avr:uno` and executes the resulting HEX image on
the native backend, exercising the real ATmega328P register and interrupt-vector
map. It skips when the `arduino:avr` core is unavailable, so the default suite
stays hermetic; set `AVR_SIM_REQUIRE_ARDUINO_CLI=1` to make a missing toolchain a
failure. The LED/millis timing assertion needs a multi-second simulation and runs
in release builds.

A green build validates the converted compatibility baseline, **not complete
AVR8js API parity or hardware correctness**. Unsupported constructors and methods
fail explicitly rather than fabricating simulator behavior.

```sh
cargo test -- --list                       # discover every case
cargo test peripherals_timer              # both native timer suites
cargo test peripherals_spi                # native SPI suite
cargo test --test peripherals             # peripheral edge/reentry regressions
cargo test --test peripheral_recovery     # panic recovery and callback ordering
AVR_SIM_REQUIRE_ARDUINO_CLI=1 cargo test --release --test arduino_cli  # real HEX image
bash tools/verify-native.sh               # full debug/release parity gate
```

There are no Cargo dependencies. Node/TypeScript is needed only to regenerate or
check the conversion, never to compile or execute the Rust scenarios.

## Layout

- `src/suites/`: one generated Rust module per original suite. Every case contains
  setup, ordered operations, expected values, and source-line annotations.
- `src/scenario.rs`: typed test operations, expressions, and assertion definitions.
- `src/runtime.rs`: native Rust test runner, lexical callback captures, mock call
  recording/reset, assertion evaluation, typed-array fixtures, assembly helpers,
  and instruction-runner helpers. It implements only the test-language subset;
  it does not evaluate JS text or emulate AVR hardware.
- `src/lib.rs`: `native_backend()` constructs a fresh native simulator adapter.
- `src/sim/`: assembler, CPU/executor, clock, GPIO, timers, EEPROM, ADC, SPI,
  USART, TWI and watchdog. `peripheral_adapter.rs` handles registry/callback
  ownership; `peripheral.rs` supplies common event and interrupt dispatch.
- `tests/harness.rs`: executable harness regressions and inventory checks.
- `tests/arduino_cli.rs`: compiles `tests/arduino-cli/uno_probe` with arduino-cli
  and runs the produced ATmega328P HEX image on the native backend (see above).
- `tests/native.rs`: arithmetic flags, synchronous callback reentry, panic recovery,
  hook fallback/chaining and Tiny PWM regression tests.
- `tests/peripherals.rs`, `tests/peripheral_recovery.rs`: EEPROM protection,
  ADC references/differential conversion, serial timing/masking, watchdog modes,
  synchronous peripheral/CPU reentry, call-through spies and panic restoration.
- `tools/verify-native.sh`: fail-closed native parity gate; CI additionally runs
  converter `--check` against the pinned submodule.
- `conversion-manifest.json`: source hashes, original names/lines, imports, case
  mappings, and per-case assertion counts.
- `tools/convert.cjs`: fail-closed AST converter, not a regex translation.

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

Every current suite is in the converter's `IMPLEMENTED` set. For future
extensions, enable suites only after native verification; regeneration preserves
the intended gate. `tools/verify-native.sh` rejects ignored converted scenarios
and uses `--include-ignored` in both builds as defense in depth. The GitHub Actions
workflow also installs pinned TypeScript and checks regeneration/source hashes.

Known groundwork limitations include adapter `Value` types in native event/hook
storage, incomplete hook introspection, constructor-buffer sharing and untested
parser/memory-boundary behavior. See the backend plan for remaining architecture
and fidelity work.

## Coverage

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

## Regenerate/check against upstream

```sh
cd rust_port/tools
npm ci --ignore-scripts
npm run generate
npm run check
```

The converter discovers every `src/**/*.spec.ts`, parses with pinned TypeScript,
and emits rustfmt-formatted Rust. Unsupported executable syntax or matchers fail
conversion. `--check` compares all output and source hashes without writing; an
obsolete generated suite is also an error. No original tests are removed.

Baseline upstream revision: `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`.
Per-file SHA-256 hashes in the manifest identify the actual input. Derived tests
retain upstream attribution and the MIT license in `LICENSE`.
