//! End-to-end check: compile a sketch with `arduino-cli` and execute the
//! resulting ATmega328P firmware on the native backend.
//!
//! This is the only test that runs genuine toolchain output instead of converted
//! AVR8js scenarios. It requires `arduino-cli` with the `arduino:avr` core; when
//! that is unavailable the test reports a skip so the default suite stays
//! hermetic. Set `AVR_SIM_REQUIRE_ARDUINO_CLI=1` to turn the missing toolchain
//! into a hard failure.
//!
//! The timing-sensitive 500 ms GPIO assertion needs a multi-second simulation,
//! so it only runs in release builds. `tools/verify-native.sh` runs both
//! profiles and therefore exercises it.

use avr_port_tests::runtime::Handle;
use avr_port_tests::{native_backend, Backend, Runtime, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

const BOARD: &str = "arduino:avr:uno";
const SKETCH: &str = "uno_probe";
const CLOCK_HZ: f64 = 16_000_000.0;
/// SRAM bytes on an ATmega328P. A real sketch sets SP from its own startup code.
const SRAM_BYTES: f64 = 2048.0;
/// Flash bytes on an ATmega328P; loaded images are padded to the full device.
const FLASH_BYTES: usize = 0x8000;

// Data-space observation points written by the sketch. See the sketch header.
const GPIOR0: usize = 0x3e; // millis() / 100
const GPIOR1: usize = 0x4a; // 0x77 once the UART TX ring drained
const GPIOR2: usize = 0x4b; // 0x5a set from the INT0 handler
const PORTB: usize = 0x25; // bit 5 mirrors digitalWrite(13)

const TX_DRAINED: i64 = 0x77;
const INT0_RAN: i64 = 0x5a;
const PB5: i64 = 0x20;
const EEPROM_SEED: u8 = 42;

/// Total clock cycles a run must exceed before the LED-timing assertion is
/// meaningful (roughly 1.6 s, enough for the third 500 ms boundary).
const TIMING_MIN_CYCLES: f64 = 25_000_000.0;
/// Step interval after which the scenario Runtime is recreated to reset its
/// callback budget. Kept below the 1e6 budget so a slice never exhausts it.
const RECYCLE_STEPS: u64 = 4_000_000;

#[test]
fn compiles_and_runs_uno_sketch() {
    if !arduino_cli_available() {
        assert!(
            std::env::var_os("AVR_SIM_REQUIRE_ARDUINO_CLI").is_none(),
            "AVR_SIM_REQUIRE_ARDUINO_CLI is set, but arduino-cli with the arduino:avr \
             core was not found on PATH"
        );
        eprintln!("skipping: arduino-cli / arduino:avr core is not installed");
        return;
    }

    let firmware = parse_hex(&compile_sketch());

    let backend: Rc<dyn Backend> = native_backend();
    let mut runtime = Runtime::new(backend.clone());
    let board = build_uno(backend.as_ref(), &mut runtime, firmware);

    // Debug builds step a shorter window; release builds reach the LED boundary.
    let steps: u64 = if cfg!(debug_assertions) {
        6_000_000
    } else {
        24_000_000
    };

    let mut pb5_samples = Vec::new();
    let mut last_pc = i64::MIN;
    let mut stalled_samples = 0u32;

    for step in 0..steps {
        // The scenario Runtime enforces a 1e6-callback budget, which a long
        // simulation of real firmware outgrows. Peripheral state lives in the
        // backend and every scheduled callback is a stateless Native, so
        // recycling the Runtime is behaviorally equivalent.
        if step > 0 && step % RECYCLE_STEPS == 0 {
            runtime = Runtime::new(backend.clone());
        }
        backend.call(
            &mut runtime,
            None,
            "avrInstruction",
            vec![board.cpu.clone()],
        );
        backend.call(&mut runtime, Some(board.cpu_handle), "tick", vec![]);
        if step % 100_000 == 0 {
            let pc = number(backend.get(board.cpu_handle, "pc")) as i64;
            stalled_samples = if pc == last_pc {
                stalled_samples + 1
            } else {
                0
            };
            last_pc = pc;
            pb5_samples.push(read_byte(backend.as_ref(), board.data, PORTB) & PB5);
        }
    }

    let cycles = number(backend.get(board.cpu_handle, "cycles"));
    let millis_coarse = read_byte(backend.as_ref(), board.data, GPIOR0);
    let tx_drained = read_byte(backend.as_ref(), board.data, GPIOR1);
    let int0_before = read_byte(backend.as_ref(), board.data, GPIOR2);

    // Drive PD2 low then high so the CHANGE-mode INT0 handler must fire.
    for level in [false, true] {
        backend.call(
            &mut runtime,
            Some(board.portd),
            "setPin",
            vec![Value::Number(2.0), Value::Bool(level)],
        );
    }
    for _ in 0..200_000 {
        backend.call(
            &mut runtime,
            None,
            "avrInstruction",
            vec![board.cpu.clone()],
        );
        backend.call(&mut runtime, Some(board.cpu_handle), "tick", vec![]);
    }
    let int0_after = read_byte(backend.as_ref(), board.data, GPIOR2);

    let eeprom = match backend.get(board.eeprom, "memory") {
        Value::Buffer { bytes, .. } => bytes.borrow()[0],
        other => panic!("expected an EEPROM buffer, got {other:?}"),
    };

    eprintln!(
        "cycles={cycles} ms={millis_coarse}00 tx=0x{tx_drained:x} int0=0x{int0_after:x} \
         eeprom={eeprom} pb5={pb5_samples:?}"
    );

    assert!(cycles > 1_000_000.0, "clock did not advance: {cycles}");
    assert_eq!(stalled_samples, 0, "program counter stopped advancing");
    assert_eq!(
        tx_drained, TX_DRAINED,
        "Serial.flush() never completed; the interrupt-driven USART TX path is not draining"
    );
    assert_ne!(
        int0_before, INT0_RAN,
        "the INT0 marker was already set before any pin change was driven"
    );
    assert_eq!(
        int0_after, INT0_RAN,
        "driving PD2 did not run the attached INT0 handler (marker 0x{int0_after:x})"
    );
    assert_eq!(eeprom, EEPROM_SEED, "EEPROM.write did not persist");
    assert!(
        millis_coarse >= 3,
        "millis() did not advance (reported {}00 ms)",
        millis_coarse
    );

    // millis()/100 must agree with the simulated clock: cycles / 16000 / 100.
    let expected_coarse = (cycles / (CLOCK_HZ / 10.0)) as i64;
    assert!(
        (millis_coarse - expected_coarse).abs() <= 4,
        "millis()/100={millis_coarse} disagrees with cycles={cycles} \
         (expected about {expected_coarse})"
    );

    if cycles > TIMING_MIN_CYCLES {
        let transitions = pb5_samples
            .windows(2)
            .filter(|pair| (pair[0] ^ pair[1]) & PB5 != 0)
            .count();
        assert!(
            transitions >= 3,
            "PB5 did not follow the 500 ms boundary: {pb5_samples:?}"
        );
    } else {
        eprintln!(
            "note: LED/millis timing assertion skipped at {cycles} cycles; \
             run the release profile to exercise it"
        );
    }
}

/// Everything a bare ATmega328P board needs, wired to the real device profile.
struct Board {
    cpu: Value,
    cpu_handle: Handle,
    data: Handle,
    eeprom: Handle,
    portd: Handle,
}

fn build_uno(backend: &dyn Backend, runtime: &mut Runtime, flash: Vec<u8>) -> Board {
    let cpu = backend.construct(
        runtime,
        "CPU",
        vec![Value::buffer(flash, 2), Value::Number(SRAM_BYTES)],
    );
    let cpu_handle = as_handle(&cpu);
    let frequency = Value::Number(CLOCK_HZ);

    let clock = backend.construct(
        runtime,
        "AVRClock",
        vec![
            cpu.clone(),
            frequency.clone(),
            backend.resolve("clockConfig"),
        ],
    );

    let mut portd = None;
    for name in ["portBConfig", "portCConfig", "portDConfig"] {
        let port = backend.construct(
            runtime,
            "AVRIOPort",
            vec![cpu.clone(), backend.resolve(name)],
        );
        if name == "portDConfig" {
            portd = Some(as_handle(&port));
        }
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
    backend.construct(
        runtime,
        "AVRADC",
        vec![cpu.clone(), backend.resolve("adcConfig")],
    );
    backend.construct(
        runtime,
        "AVRSPI",
        vec![cpu.clone(), backend.resolve("spiConfig"), frequency.clone()],
    );
    backend.construct(
        runtime,
        "AVRTWI",
        vec![cpu.clone(), backend.resolve("twiConfig"), frequency],
    );

    let eeprom_backend =
        backend.construct(runtime, "EEPROMMemoryBackend", vec![Value::Number(1024.0)]);
    backend.construct(
        runtime,
        "AVREEPROM",
        vec![
            cpu.clone(),
            eeprom_backend.clone(),
            backend.resolve("eepromConfig"),
        ],
    );
    backend.construct(
        runtime,
        "AVRWatchdog",
        vec![cpu.clone(), backend.resolve("watchdogConfig"), clock],
    );

    Board {
        data: as_handle(&backend.get(cpu_handle, "data")),
        eeprom: as_handle(&eeprom_backend),
        portd: portd.expect("portDConfig must be resolvable"),
        cpu,
        cpu_handle,
    }
}

fn arduino_cli_available() -> bool {
    let version = Command::new("arduino-cli").arg("version").output();
    if !matches!(&version, Ok(output) if output.status.success()) {
        return false;
    }
    let cores = Command::new("arduino-cli").args(["core", "list"]).output();
    matches!(&cores, Ok(output) if output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("arduino:avr"))
}

fn compile_sketch() -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sketch = manifest.join("tests").join("arduino-cli").join(SKETCH);
    let build = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{SKETCH}-build"));
    std::fs::create_dir_all(&build).expect("create arduino-cli build directory");

    let output = Command::new("arduino-cli")
        .args(["compile", "-b", BOARD, "--build-path"])
        .arg(&build)
        .arg(&sketch)
        .output()
        .expect("run arduino-cli");
    assert!(
        output.status.success(),
        "arduino-cli compile failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let hex = build.join(format!("{SKETCH}.ino.hex"));
    std::fs::read_to_string(&hex).unwrap_or_else(|error| panic!("read {}: {error}", hex.display()))
}

fn as_handle(value: &Value) -> Handle {
    match value {
        Value::Handle(handle) => *handle,
        other => panic!("expected a native handle, got {other:?}"),
    }
}

fn number(value: Value) -> f64 {
    match value {
        Value::Number(number) => number,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn read_byte(backend: &dyn Backend, data: Handle, address: usize) -> i64 {
    match backend.get(data, &address.to_string()) {
        Value::Number(value) => value as i64,
        other => panic!("expected a number at {address:#x}, got {other:?}"),
    }
}

/// Minimal Intel HEX reader. Unwritten flash is left at `0xff`, matching an
/// erased ATmega328P.
fn parse_hex(text: &str) -> Vec<u8> {
    let mut flash = vec![0xffu8; FLASH_BYTES];
    for line in text.lines() {
        let line = line.trim();
        let Some(record) = line.strip_prefix(':') else {
            continue;
        };
        let bytes: Vec<u8> = (0..record.len() / 2)
            .map(|index| {
                let pair = &record[2 * index..2 * index + 2];
                u8::from_str_radix(pair, 16).unwrap_or_else(|_| panic!("bad hex pair {pair:?}"))
            })
            .collect();
        let length = bytes[0] as usize;
        let address = ((bytes[1] as usize) << 8) | bytes[2] as usize;
        if bytes[3] == 0x00 {
            flash[address..address + length].copy_from_slice(&bytes[4..4 + length]);
        }
    }
    flash
}
