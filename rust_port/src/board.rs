// SPDX-License-Identifier: MIT

//! Host-side glue for running a real ATmega328P firmware image.
//!
//! The converted scenarios in `src/suites/` describe AVR8js behavior through the
//! dynamic [`Backend`] API. A host that only wants to *run* an Arduino sketch —
//! a GUI, a CLI, an integration harness — would otherwise repeat the same Intel
//! HEX parsing and peripheral wiring. This module owns that glue once.
//!
//! Every operation delegates to the same `construct`/`get`/`call` entry points
//! the scenarios use, so the parity contract is unchanged: nothing here
//! reimplements AVR behavior, and the module adds no dependencies.

use crate::runtime::{Backend, Handle, Runtime, Value};

/// Clock rate of a stock Arduino Uno, in Hz.
pub const UNO_CLOCK_HZ: f64 = 16_000_000.0;
/// SRAM size of an ATmega328P, in bytes.
pub const UNO_SRAM_BYTES: f64 = 2048.0;
/// Flash size of an ATmega328P, in bytes. Images are padded to this size.
pub const UNO_FLASH_BYTES: usize = 0x8000;

/// Data-space address of `PORTB`.
pub const PORTB: usize = 0x25;
/// Data-space address of `PORTC`.
pub const PORTC: usize = 0x28;
/// Data-space address of `PORTD`.
pub const PORTD: usize = 0x2b;
/// `PORTB` pin driving the Uno's built-in LED (Arduino digital pin 13).
pub const LED_BUILTIN_PIN: u8 = 5;

/// Observed state of a GPIO pin, mirroring AVR8js's `PinState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinState {
    /// Driven low by the port.
    Low,
    /// Driven high by the port.
    High,
    /// Input with no pull-up (high impedance).
    Input,
    /// Input with the internal pull-up engaged.
    InputPullUp,
}

impl PinState {
    fn from_raw(raw: i64) -> Self {
        match raw {
            0 => Self::Low,
            1 => Self::High,
            2 => Self::Input,
            3 => Self::InputPullUp,
            other => panic!("unknown PinState value {other}"),
        }
    }
    /// True when the pin reads as high, i.e. driven high or pulled up.
    pub fn is_high(self) -> bool {
        matches!(self, Self::High | Self::InputPullUp)
    }
}

/// A constructed ATmega328P board: CPU plus every peripheral a stock Uno has.
pub struct Board {
    /// CPU object, passed to the `avrInstruction` free function.
    pub cpu: Value,
    /// Identity of the CPU object, for `get`/`call` receivers.
    pub cpu_handle: Handle,
    /// Data-space memory view, for reading I/O registers directly.
    pub data: Handle,
    /// `PORTB` — Arduino digital pins 8-13.
    pub portb: Handle,
    /// `PORTC` — Arduino analog pins A0-A5.
    pub portc: Handle,
    /// `PORTD` — Arduino digital pins 0-7.
    pub portd: Handle,
    /// `AVRADC`, so a host can supply `channelValues` from a solved circuit.
    pub adc: Handle,
    /// `AVRTWI`, so a host can attach an I2C slave handler.
    pub twi: Handle,
    /// `AVRSPI`, so a host can attach a peripheral transfer handler.
    pub spi: Handle,
    /// EEPROM backend; see [`Board::eeprom_bytes`].
    pub eeprom: Handle,
}

impl Board {
    /// Wire a stock Uno to the real ATmega328P device profile.
    ///
    /// `flash` is the byte image produced by [`parse_hex`]. The peripheral set
    /// is the one a `digitalWrite`/`delay`/`Serial` sketch needs: three I/O
    /// ports, timer0/1/2, USART0, ADC, SPI, TWI, EEPROM, watchdog and the
    /// 16 MHz [`Clock`](crate::sim::clock::Clock) that gives `millis()` its base.
    pub fn uno(backend: &dyn Backend, runtime: &mut Runtime, flash: Vec<u8>) -> Self {
        let cpu = backend.construct(
            runtime,
            "CPU",
            vec![Value::buffer(flash, 2), Value::Number(UNO_SRAM_BYTES)],
        );
        let cpu_handle = cpu.handle();
        let frequency = Value::Number(UNO_CLOCK_HZ);

        let clock = backend.construct(
            runtime,
            "AVRClock",
            vec![
                cpu.clone(),
                frequency.clone(),
                backend.resolve("clockConfig"),
            ],
        );

        let mut ports = [Handle(0); 3];
        for (slot, name) in ["portBConfig", "portCConfig", "portDConfig"]
            .into_iter()
            .enumerate()
        {
            let port = backend.construct(
                runtime,
                "AVRIOPort",
                vec![cpu.clone(), backend.resolve(name)],
            );
            ports[slot] = port.handle();
        }

        for name in ["timer0Config", "timer1Config", "timer2Config"] {
            backend.construct(
                runtime,
                "AVRTimer",
                vec![cpu.clone(), backend.resolve(name)],
            );
        }

        backend.construct(
            runtime,
            "AVRUSART",
            vec![
                cpu.clone(),
                backend.resolve("usart0Config"),
                frequency.clone(),
            ],
        );
        let adc = backend
            .construct(
                runtime,
                "AVRADC",
                vec![cpu.clone(), backend.resolve("adcConfig")],
            )
            .handle();
        let spi = backend
            .construct(
                runtime,
                "AVRSPI",
                vec![cpu.clone(), backend.resolve("spiConfig"), frequency.clone()],
            )
            .handle();
        let twi = backend
            .construct(
                runtime,
                "AVRTWI",
                vec![cpu.clone(), backend.resolve("twiConfig"), frequency],
            )
            .handle();
        backend.construct(
            runtime,
            "AVRWatchdog",
            vec![cpu.clone(), backend.resolve("watchdogConfig"), clock],
        );

        let eeprom_backend =
            backend.construct(runtime, "EEPROMMemoryBackend", vec![Value::Number(1024.0)]);
        let eeprom = eeprom_backend.handle();
        backend.construct(
            runtime,
            "AVREEPROM",
            vec![cpu.clone(), eeprom_backend, backend.resolve("eepromConfig")],
        );

        let data = backend.get(cpu_handle, "data").handle();
        let [portb, portc, portd] = ports;
        Self {
            cpu,
            cpu_handle,
            data,
            portb,
            portc,
            portd,
            adc,
            twi,
            spi,
            eeprom,
        }
    }

    /// Execute one instruction, then let peripherals tick.
    ///
    /// The `tick` half is not optional: it dispatches due clock events (timer
    /// overflows feeding `millis()`) and pending interrupts, so stepping with
    /// `avrInstruction` alone silently freezes timers and `delay()`.
    pub fn step(&self, backend: &dyn Backend, runtime: &mut Runtime) {
        backend.call(runtime, None, "avrInstruction", vec![self.cpu.clone()]);
        backend.call(runtime, Some(self.cpu_handle), "tick", vec![]);
    }

    /// Execute `cycles` instructions, occasionally refilling the callback budget.
    ///
    /// A long run dispatches far more callbacks than
    /// [`crate::runtime::DEFAULT_BUDGET`] allows, so the budget is reset every
    /// [`BUDGET_REFILL_CYCLES`](Self::BUDGET_REFILL_CYCLES) instructions.
    pub fn run_cycles(&self, backend: &dyn Backend, runtime: &mut Runtime, cycles: u64) {
        for index in 0..cycles {
            if index > 0 && index % Self::BUDGET_REFILL_CYCLES == 0 {
                runtime.reset_budget();
            }
            self.step(backend, runtime);
        }
    }

    /// Instruction interval between callback-budget refills.
    ///
    /// Comfortably below [`crate::runtime::DEFAULT_BUDGET`] at any realistic
    /// interrupt rate, so the budget never triggers mid-run.
    pub const BUDGET_REFILL_CYCLES: u64 = 250_000;

    /// Simulated cycles executed since reset.
    pub fn cycles(&self, backend: &dyn Backend) -> f64 {
        backend.get(self.cpu_handle, "cycles").number()
    }

    /// Simulated time in milliseconds, on the same time base as `millis()`.
    pub fn millis(&self, backend: &dyn Backend) -> f64 {
        self.cycles(backend) / (UNO_CLOCK_HZ / 1000.0)
    }

    /// Read one data-space byte, including I/O registers.
    ///
    /// This is a plain memory read: it does not fire peripheral read hooks, so
    /// it is safe to call between instructions but cannot be used to emulate a
    /// device driving the bus. Use [`Board::pin_state`] for pin levels.
    pub fn read_data(&self, backend: &dyn Backend, address: usize) -> u8 {
        backend.get(self.data, &address.to_string()).number() as u8
    }

    /// A single bit of a data-space byte; handy for a port register.
    pub fn read_bit(&self, backend: &dyn Backend, address: usize, bit: u8) -> bool {
        self.read_data(backend, address) & (1 << bit) != 0
    }

    /// Observed pin state, accounting for DDR, pull-ups and timer overrides.
    pub fn pin_state(
        &self,
        backend: &dyn Backend,
        runtime: &mut Runtime,
        port: Handle,
        pin: u8,
    ) -> PinState {
        let raw = backend.call(
            runtime,
            Some(port),
            "pinState",
            vec![Value::Number(pin as f64)],
        );
        PinState::from_raw(raw.number() as i64)
    }

    /// Drive an external signal onto a pin, as a wired-up component would.
    pub fn set_input(
        &self,
        backend: &dyn Backend,
        runtime: &mut Runtime,
        port: Handle,
        pin: u8,
        high: bool,
    ) {
        backend.call(
            runtime,
            Some(port),
            "setPin",
            vec![Value::Number(pin as f64), Value::Bool(high)],
        );
    }

    /// State of the built-in LED: Arduino digital pin 13, `PORTB5`.
    pub fn led_builtin(&self, backend: &dyn Backend, runtime: &mut Runtime) -> PinState {
        self.pin_state(backend, runtime, self.portb, LED_BUILTIN_PIN)
    }

    /// Snapshot of the EEPROM contents.
    pub fn eeprom_bytes(&self, backend: &dyn Backend) -> Vec<u8> {
        match backend.get(self.eeprom, "memory") {
            Value::Buffer { bytes, .. } => bytes.borrow().clone(),
            other => panic!("expected an EEPROM buffer, got {other:?}"),
        }
    }
}

/// Decode an Intel HEX image into a flash byte vector.
///
/// The result is always [`UNO_FLASH_BYTES`] long and `0xff`-filled, matching an
/// erased ATmega328P, so unwritten words stay as `0xff` rather than zero.
/// Data (`00`), end-of-file (`01`) and extended-linear-address (`04`) records
/// are honoured; records for other address spaces are ignored. Lines without a
/// leading `:` (blank lines, comments) are skipped.
///
/// Malformed input fails loudly rather than fabricating an image: a bad
/// checksum, a truncated record or an address past the end of flash panics.
pub fn parse_hex(text: &str) -> Vec<u8> {
    let mut flash = vec![0xffu8; UNO_FLASH_BYTES];
    let mut base: usize = 0;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        let Some(record) = line.strip_prefix(':') else {
            continue;
        };
        assert!(
            record.len() % 2 == 0 && record.len() >= 10,
            "Intel HEX line {} is truncated: {line:?}",
            number + 1
        );
        let bytes: Vec<u8> = (0..record.len() / 2)
            .map(|index| {
                let pair = &record[2 * index..2 * index + 2];
                u8::from_str_radix(pair, 16)
                    .unwrap_or_else(|_| panic!("Intel HEX line {}: bad byte {pair:?}", number + 1))
            })
            .collect();

        let length = bytes[0] as usize;
        let offset = ((bytes[1] as usize) << 8) | bytes[2] as usize;
        let kind = bytes[3];
        assert!(
            bytes.len() == length + 5,
            "Intel HEX line {}: length {length} disagrees with {} payload bytes",
            number + 1,
            bytes.len().saturating_sub(5)
        );
        let sum = bytes.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        assert!(
            sum == 0,
            "Intel HEX line {}: checksum mismatch (sum {sum:#04x})",
            number + 1
        );

        match kind {
            0x00 => {
                let start = base + offset;
                let end = start + length;
                assert!(
                    end <= flash.len(),
                    "Intel HEX line {}: data at {start:#06x}..{end:#06x} exceeds \
                     the {:#x}-byte flash image",
                    number + 1,
                    flash.len()
                );
                flash[start..end].copy_from_slice(&bytes[4..4 + length]);
            }
            0x01 => break,
            // Extended linear address: upper 16 bits of every following offset.
            0x04 => {
                assert!(
                    length == 2,
                    "Intel HEX line {}: type 04 record must carry 2 bytes",
                    number + 1
                );
                base = ((bytes[4] as usize) << 8 | bytes[5] as usize) << 16;
            }
            _ => {}
        }
    }
    flash
}
