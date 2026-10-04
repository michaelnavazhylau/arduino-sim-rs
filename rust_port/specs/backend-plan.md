# Native backend implementation contract and milestones

Status: **milestones 0–5 implemented and verified**. The native backend runs
**283 of 347 converted scenarios** by default. The remaining **64 scenarios**
(EEPROM, ADC, SPI, USART, TWI and watchdog) remain ignored until implemented.
Official specifications are pinned locally; no JavaScript execution is used.

## Sources of truth

1. **Compatibility behavior:** pinned upstream `../../avr8js/src/` production
   source plus the converted Rust scenarios. Upstream revision:
   `bee6f0a94e0e27786f6bc21aee3c775849fb50fd`. Test input hashes are in
   `../conversion-manifest.json`.
2. **Hardware behavior:** the four pinned Microchip documents in
   [sources.json](sources.json), with section/page navigation in [README.md](README.md).
3. **Adapter behavior:** `../src/runtime.rs` (`Backend`, `Value`, synchronous
   callback invocation) and `../src/scenario.rs`.

The immediate target is a native **AVR8js compatibility port**. When source/tests
and hardware documentation differ, log the difference and preserve the existing
compatibility contract. Do not silently change timing or expectations to make a
more realistic hardware model. A strict-device/hardware-fidelity mode would need
an explicit design decision and a separate test set; no such mode exists yet.

## Upstream implementation files to port

All paths below are relative to `../../avr8js/src/`.

| Native responsibility | Software specification | Rust test module |
| --- | --- | --- |
| Assembler parser/encoder/result model | `utils/assembler.ts` | `utils_assembler` |
| CPU memory, register views, clock-event queue, interrupt queue | `cpu/cpu.ts` | `cpu_cpu` |
| Interrupt entry, stack, I flag | `cpu/interrupt.ts` | `cpu_interrupt` |
| Decode/execute, flags, addressing, cycle accounting | `cpu/instruction.ts` | `cpu_instruction` |
| Clock and protected prescaler writes | `peripherals/clock.ts` | `peripherals_clock` |
| GPIO, external/pin-change interrupt handling, alternate pin ownership | `peripherals/gpio.ts` | `peripherals_gpio` |
| Mega timers, register latches, PWM, event scheduling | `peripherals/timer.ts` | `peripherals_timer` |
| Tiny-specific 8-bit Timer1 and shared-register hooks | `peripherals/timer-attiny.ts` | `peripherals_timer_attiny` |
| EEPROM memory and erase/write timing | `peripherals/eeprom.ts` | `peripherals_eeprom` |
| ADC mux/reference/conversion | `peripherals/adc.ts` | `peripherals_adc` |
| SPI transfer and completion | `peripherals/spi.ts` | `peripherals_spi` |
| USART framing, transmission, receive and line callback | `peripherals/usart.ts` | `peripherals_usart` |
| TWI master state machine and default handler | `peripherals/twi.ts` | `peripherals_twi` |
| Watchdog protected writes, reset and interrupt | `peripherals/watchdog.ts` | `peripherals_watchdog` |
| USI (coverage extension, not an existing suite) | `peripherals/usi.ts` | None |
| Assembly/instruction runner helpers | `utils/test-utils.ts` | Already provided by Rust harness |
| Exported configs/enums/types | `index.ts`, `types.ts`, respective modules | Imported symbols listed per suite in conversion manifest |

Do not derive device configuration solely from the datasheets. Port the exact
exported config values and override fields first; tests intentionally use arbitrary
SRAM/flash sizes and non-default peripheral maps. Hardware-valid device profiles
are an additional layer, not a replacement for those fixtures.

## Compatibility details to preserve

### Addressing, memory and register access

- `cpu.pc` is a **word** address. Assembler labels, line offsets, and runner
  `runToAddress` targets are **byte** addresses. Program byte and word views alias.
- Data registers occupy 0–31; I/O addressing and data addressing differ by `0x20`.
  The configured SRAM follows the `0x100` register space in the source model.
- `CPU` defaults to **8,192 SRAM bytes**, not the physical ATmega328P's 2,048.
  Tests can pass a different size. Initial SP is the last allocated data byte.
  Source: `cpu/cpu.ts:15,49–60,86–99`; hardware: ATmega328P §§7–8.
- DataView word operations are explicitly little-endian; u8/u16 writes narrow.
  Use explicit Rust conversions/wrapping operations, not host-endian assumptions
  or overflow panics that change simulator behavior.
- Raw writes to `cpu.data[...]` bypass hooks. `readData` and `writeData` use hooks;
  write hooks also receive the bit mask used by SBI/CBI. Preserve that distinction.
- Source `reset()` sets SP/PC and clears pending events/interrupts. It does not
  zero existing RAM or reset `cycles`. Do not substitute a whole-object reset.

### Events and interrupt accounting

- Delays are relative to the current cycle count, clamped to at least one cycle.
  Event cancellation/update uses callback identity, not an equivalent new closure.
- Source event insertion compares with `<`, so an equal-deadline event is inserted
  ahead of existing equal-deadline entries. Source `tick()` executes **one** due
  clock callback and then checks the next interrupt, not an unbounded event drain.
  Callbacks can mutate the same CPU and schedule further work synchronously.
  Source: `cpu/cpu.ts:197–272`; event APIs are simulator contracts, not datasheet APIs.
- **Confirmed hardware difference:** `avrInterrupt` pushes the return PC, clears I,
  and adds **two cycles**, even for the three-byte stack case. The converted
  interrupt suites assert this. The ATmega328P datasheet §7.7.1 (PDF p25) says
  **four cycles minimum** for hardware interrupt response; the large-device
  datasheet §7.8.1 (PDF p19) describes **five cycles minimum**. Do not silently
  replace the compatibility helper's two-cycle increment with either value.
- `pc22Bits` is selected from allocated program-byte length `> 0x20000` in the
  source. Return addresses use two or three stack bytes accordingly. Extended
  EIND/RAMPZ behavior and device masking require both source and instruction manual.
- The executor adds its final cycle increment and wraps PC by allocated program
  word count. Handler-specific increments occur before that final increment.
  Source: `cpu/instruction.ts:799–800`. This is not equivalent to simply giving
  every unknown opcode an illegal-instruction error.

### Software-only assembler contract

- Preserve the two passes, byte-addressed forward labels, little-endian output,
  `_REPLACE`, `_LOC`, `_IW`, comments, and the exact parsing/error behaviors.
- The returned `bytes`, `errors`, `lines`, and `labels` are part of the tests, not
  incidental metadata. Pass-one errors return empty bytes/lines/labels. Some
  diagnostics use zero-based source indexes while metadata uses one-based lines.
- Register parsing and integer parsing can accept unusual input that a strict
  assembler would reject. An existing test even contains a trailing backtick in
  a CPSE operand. Keep parity before considering stricter validation.
- Hardware instruction availability and assembler acceptance are separate:
  upstream explicitly does not enforce a target-device instruction whitelist.

### Peripheral-specific source behavior

- Clock prescaler indices 9–15 are hardware-reserved in the datasheet but have
  measured mappings in `peripherals/clock.ts:25–31`. Preserve the table for parity;
  do not infer new meanings from reserved register bits.
- Large cycle jumps, buffered compare writes, 16-bit latches and shared hooks are
  observable in tests. Timers depend on GPIO, and Tiny Timer1 is a different
  peripheral from Mega's 16-bit Timer1.
- TWI uses completion callbacks and implements master states; its source still
  has `TODO: add slave states`. GPIO/SPI/USART/ADC callbacks must run at the same
  observable points as upstream, including call-through spy instrumentation.
- The harness instruction runner's BREAK/error ordering is already implemented
  in Rust; do not bypass it with a second runner with different semantics.

## Implementation order and acceptance gates

| Milestone | Work | Behavior cases unlocked | Gate |
| --- | --- | ---: | --- |
| 0 ✓ | Native registry, memory views, Backend plumbing | 0 | 13 harness tests; unsupported operations fail |
| 1 ✓ | Assembler parser, directives, opcode encoding | 83 | `cargo test utils_assembler`; 4 additional unit tests |
| 2 ✓ | CPU memory, events, interrupts | 8 | `cargo test cpu_cpu` and `cargo test cpu_interrupt` |
| 3 ✓ | Instruction decode/execute, flags, stack, extended addressing | 97 | `cargo test cpu_instruction`; cumulative 188 |
| 4 ✓ | Clock and GPIO, external/pin-change interrupts | 30 | `cargo test peripherals_clock` and `cargo test peripherals_gpio`; cumulative 218 |
| 5 ✓ | Mega timers and Tiny Timer1, buffered OCR, shared hooks | 65 | `cargo test peripherals_timer`; cumulative 283 |
| 6 pending | EEPROM, ADC, SPI, USART, TWI, watchdog | 64 | Explicit ignored suites during development; final total 347 |

The first six milestones above execute real Rust code. Milestone 6 remains to
be implemented; EEPROM and serial work may be reordered. Watchdog depends on
the clock. Current full checks pass: `cargo fmt --check`, offline tests, strict
Clippy and converter `--check`.

Each milestone should include native unit tests beyond the converted cases.
Keep `Runtime`/`Value` in the test-adapter layer rather than making the production
CPU depend on the scenario interpreter. Represent host/peripheral events with
stable IDs and an explicit dispatch layer; release mutable borrows before host
callbacks reenter the backend. Wire a real adapter into `native_backend()` only
when its supported operations actually execute native code. Unsupported operations
must remain explicit failures.

Do not remove all `#[ignore]` attributes at once. Enable suites as they pass and
update `tools/convert.cjs` so regeneration retains the intended gates. The final
behavior check must run the simulator suites, not just the green harness build.

## Current architecture and residual limitations

- `src/sim/` implements native state and dispatch; the generated suites are
  unchanged except for their enablement gates. No JS engine is embedded.
- Peripheral hooks receive a live CPU through the native instruction bus.
  Raw memory writes bypass hooks; host read/write hooks support synchronous
  reentry and write suppression/fallback.
- GPIO listeners execute after output state changes and **before PIN updates**,
  with no registry/port borrow held. CPU and timer state are moved temporarily
  back into the registry at synchronous host boundaries, then recovered even
  when callbacks panic. This is not an end-of-instruction notification queue.
- Eight extra native regression tests cover ADC operand aliasing/flags, GPIO
  OUT ordering, CPU hook reentry, timer listener reentry, Tiny shared native/host
  hooks and phase PWM, plus callback panic recovery.
- The implementation still uses scenario `Value` in event/hook storage and
  config parsing. Splitting a production-only host/event interface from the
  adapter is outstanding architectural work; milestones denote implemented
  compatibility behavior, not completion of that separation.
- Program byte/word views alias CPU storage, but the original program Buffer
  passed to the CPU constructor is currently copied. Hook arrays do not expose
  native installed hooks for arbitrary direct introspection/calling. These
  broader source API contracts need follow-up tests and implementation.
- Parser diagnostics, DataView defaults/bounds, invalid memory accesses and
  comprehensive arithmetic flags remain insufficiently verified. The ADC
  instruction now retains original operands before writing its destination;
  the tested source-compatible flag formulas are not a hardware-fidelity claim.
- Timer input capture, asynchronous clocking, PLL/dead-time and physical-device
  profiles are not established by the converted tests. Upstream omissions
  (for example compare-C flag/enable handling in some mega timer hooks) are
  preserved instead of silently corrected.

## Coverage additions required for stronger fidelity

The converted suite is a parity baseline, not exhaustive instruction or device
coverage. Before claiming a sophisticated hardware model, add at least:

- Exhaustive/parameterized arithmetic flags, signed edge cases, both PC widths,
  branch/skip lengths and register alias hazards; generated instruction programs
  compared against a trusted oracle or hardware where feasible.
- Equal-cycle event ordering, multiple overdue callbacks, cancellation during
  dispatch, reset during callbacks, pending interrupt priority and enable changes.
- SRAM/flash boundaries, invalid addresses/opcodes, physical-device sizes,
  stack overflow/underflow, and explicit compatibility-vs-strict semantics.
- Timer large-jump equivalence, shared-register bit isolation, all WGM/prescaler
  combinations and buffering/latch rules; PLL/dead-time/async behavior if implemented.
- Serial framing/error modes, TWI slave/arbitration/bus errors, and USI. The current
  USI implementation has no matching upstream suite.
- SLEEP and SPM/flash programming, currently marked unimplemented in
  `cpu/instruction.ts:663–671`; their assembly tests do not prove execution.
- Silicon-revision errata only for an explicitly selected device revision. Do not
  apply every erratum from every family member to the same simulated chip.

External circuit models, electrical analog simulation, firmware HEX/ELF loaders,
Arduino UI integration, fuses, and bootloader fidelity require additional contracts
when their implementation is requested; they are not covered by this test bundle.
