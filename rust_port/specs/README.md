# AVR specifications for the native Rust backend

The required hardware references have been pulled from **official Microchip
URLs**. This directory is a reference bundle, not a native backend implementation.
See [backend-plan.md](backend-plan.md) for the implementation order and known
compatibility differences.

## Local files

| ID | PDF | Pinned revision | Pages | Why needed |
| --- | --- | --- | ---: | --- |
| `avr-instruction-set` | [AVR instruction manual](pdf/avr-instruction-set-ds40002198.pdf) | DS40002198B, February 2021 | 166 | Encodings, addressing, flags, lengths, core-specific timing |
| `atmega328p` | [ATmega328P-family datasheet](pdf/atmega328p-ds40002061a.pdf) | DS40002061A, 2018 | 662 | Primary AVR8js CPU/peripheral register and behavior reference |
| `attiny85` | [ATtiny25/45/85 datasheet](pdf/attiny25-45-85-atmel2586.pdf) | 2586Q, August 2013 | 234 | Distinct 8-bit Timer1, shared registers, USI, PLL, errata |
| `atmega2560` | [ATmega2560-family datasheet](pdf/atmega640-1280-2560-atmel2549.pdf) | 2549Q, February 2014 | 435 | Large-flash addresses, three-byte return PC, EIND/RAMPZ, extra compare channels |

Searchable full text is available under `text/`, with matching filenames. These
PDFs are complete documents, not summary datasheets. `pdfinfo` confirmed their
page counts; the ATmega328P-family document really has 662 pages.

[sources.json](sources.json) records the exact URL, revision, size, SHA-256 hash,
page count, and retrieval time of each PDF. The upstream legacy instruction-manual
URL returned HTTP 403; the official DS40002198B manual was downloaded instead.
The stable instruction-manual URL serves revision **B**, while a newer **C** online
revision exists. We do not label the downloaded PDF as the latest revision.

## Reproduce and verify

From `rust_port/`:

```sh
python3 tools/specs.py fetch --extract     # fetch missing PDFs; validate pinned hashes
python3 tools/specs.py check               # offline PDF integrity checks
python3 tools/specs.py extract             # regenerate searchable text from pinned PDFs
python3 tools/specs.py find --document atmega328p --pattern 'Interrupt Response Time'
python3 tools/specs.py page --document atmega328p --number 25
```

The script uses Python's standard library. Downloading requires `curl`; extracting
text requires Poppler (`brew install poppler` on macOS). These are documentation
tools, not Cargo or simulator-runtime dependencies. Fetches are atomic and fail
on HTTP, size, PDF signature, or SHA-256 mismatch. A changed upstream PDF needs
explicit review and a manifest update; the tool never quietly accepts it.

Use **1-based PDF page numbers** with the page command, not text-file line numbers.
Text preserves form-feed page boundaries. Page numbers below were verified against
those boundaries and the printed section headings in the downloaded revisions.

The approximately 46 MiB of vendor PDFs and generated text stay local and are
Git-ignored. The provenance, downloader, and our implementation notes remain
versionable. Vendor documents retain their own copyright/terms; the project's MIT
software license does **not** relicense them.

## Section map

### CPU and assembler

| Requirement | Document / section | PDF pages |
| --- | --- | --- |
| Operand/address notation | Instruction manual §1 | 6–7 |
| Extended addressing registers | Instruction manual §2 | 8 |
| Data/program addressing modes | Instruction manual §3 | 9–16 |
| Branch and instruction summaries | Instruction manual §§4–5 | 17–23 |
| Opcode, SREG boolean formulas, instruction-specific timing | Instruction manual §6 | 24–148 |
| Device/core instruction availability | Instruction manual Appendix A (§7) | 149–160 |
| CPU, SREG, register file, SP | ATmega328P §§7.1–7.6 | 18–23 |
| Interrupt response | ATmega328P §7.7.1 | 25 |
| SRAM/flash/EEPROM memory organization | ATmega328P §§8.1–8.5 | 26–30 |
| Extended Z and indirect PC registers | ATmega2560 §§7.6.1–7.6.2 | 16 |
| Large-device interrupt response and stack | ATmega2560 §7.8.1 | 19 |
| CALL / EICALL / EIJMP / ELPM | Instruction manual §§6.32, 6.51–6.53 | 54, 72–74 |
| RCALL / RET / RETI | Instruction manual §§6.87–6.89 | 110–112 |
| Primary-device instruction cycle summary | ATmega328P §37 | 625–627 |

The AVR8js assembler grammar, directives, labels, diagnostics, and returned metadata
are **software contracts**. Use `../../avr8js/src/utils/assembler.ts` and its tests
for those; a hardware manual or GNU assembler cannot replace this specification.

### ATmega328P peripherals

| Rust suite | Datasheet sections | PDF pages | Main concerns |
| --- | --- | --- | --- |
| `cpu_interrupt` | §§7.7, 12.4 | 23–25, 74–76 | Entry, priority, vector addresses, global enable |
| `peripherals_eeprom` | §§8.4, 8.6.1–8.6.3 | 29–34 | EEMPE window, write/erase modes, ready interrupt |
| `peripherals_clock` | §§9.11–9.12 | 45–47 | CLKPR enable window and divider |
| `peripherals_watchdog` | §§11.6, 11.8–11.9 | 59–65 | Reset source, protected writes, WDR, interrupt/reset modes |
| `peripherals_gpio` | §§13–14 | 79–101 | INT/PCINT, DDR/PORT/PIN, pull-ups, alternate pin control |
| `peripherals_timer` (Timer0) | §15 | 102–119 | Counter, compare, overflow, PWM, forced compare |
| `peripherals_timer` (Timer1) | §16; especially §16.3 | 120–146; 122–125 | 16-bit latch, capture, buffering, WGM modes |
| `peripherals_timer` (prescalers) | §17 | 147–149 | Internal/external clocks, synchronization, reset |
| `peripherals_timer` (Timer2) | §18 | 150–168 | Distinct prescalers, asynchronous counter |
| `peripherals_spi` | §19 | 169–178 | Dividers, modes, buffering, collision/complete flags |
| `peripherals_usart` | §20 | 179–204 | Baud/frame timing, TX/RX flags, interrupts |
| `peripherals_twi` | §22 | 215–242 | Status transitions, ACK/NACK, repeated START, STOP |
| `peripherals_adc` | §24; especially §§24.4, 24.7 | 246–261; 249–250, 256 | Conversion time, mux/reference, result layout |
| All configurations | §36 | 621–624 | Data-space and I/O-space register addresses |
| Silicon-specific behavior | §40; ATmega328/P §§40.7–40.8 | 641–656; 651–656 | Errata by silicon revision |

ATmega328P is the primary profile, not the only profile. Preserve the configurable
register/vector/pin overrides used in the timer tests. Do not treat all family
members' memory sizes or vector tables as interchangeable.

### ATtiny85 and large-flash extensions

| Requirement | Document / section | PDF pages |
| --- | --- | --- |
| Tiny interrupt vectors | ATtiny85 §9 | 48–52 |
| Tiny pin routing | ATtiny85 §10 | 53–64 |
| Tiny Timer0 | ATtiny85 §11 | 65–82 |
| **Tiny Timer1, not mega 16-bit Timer1** | ATtiny85 §12 | 83–94 |
| Tiny Timer1 prescaler / PWM / shared registers | ATtiny85 §§12.1–12.3 | 83, 86–88, 89–94 |
| Tiny PLL fast peripheral clock | ATtiny85 §6.1.5 | 24 |
| ATtiny15 compatibility timer mode | ATtiny85 §13 | 95–104 |
| Dead-time generator | ATtiny85 §14 | 105–107 |
| USI | ATtiny85 §15 | 108–118 |
| Tiny register map / instruction summary / errata | ATtiny85 §§23, 24, 27 | 200–201, 202–203, 212–215 |
| Large-device CPU and memory | ATmega2560 §§7–8 | 11–26 |
| Large-device vector table | ATmega2560 §14 | 101–108 |
| Extra 16-bit timers and compare channels | ATmega2560 §17 | 133–163 |
| Large-device register map / cycle summary / errata | ATmega2560 §§33, 34, 37 | 399–403, 404–406, 416–421 |

USI has source code but no converted upstream test suite. PLL, dead time, sleep,
flash programming, analog comparator, and silicon errata are not automatically
covered by the 347 tests. Their datasheet sections are references for later work,
not a claim that the compatibility port already implements them.
