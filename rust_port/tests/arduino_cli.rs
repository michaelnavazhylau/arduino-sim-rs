// SPDX-License-Identifier: MIT

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

use avr_port_tests::board::{parse_hex, Board, PinState, LED_BUILTIN_PIN, PORTB};
use avr_port_tests::{native_backend, Backend, Runtime};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

const BOARD: &str = "arduino:avr:uno";
const SKETCH: &str = "uno_probe";
const CLOCK_HZ: f64 = 16_000_000.0;

// Data-space observation points written by the sketch. See the sketch header.
const GPIOR0: usize = 0x3e; // millis() / 100
const GPIOR1: usize = 0x4a; // 0x77 once the UART TX ring drained
const GPIOR2: usize = 0x4b; // 0x5a set from the INT0 handler

const TX_DRAINED: i64 = 0x77;
const INT0_RAN: i64 = 0x5a;
const PB5: i64 = (1u8 << LED_BUILTIN_PIN) as i64;
const EEPROM_SEED: u8 = 42;

/// Total clock cycles a run must exceed before the LED-timing assertion is
/// meaningful (roughly 1.6 s, enough for the third 500 ms boundary).
const TIMING_MIN_CYCLES: f64 = 25_000_000.0;
/// Step interval between callback-budget refills.
///
/// One instruction dispatches at most a clock event and an interrupt, so a
/// 250k-instruction slice can consume at most 750k of the
/// `DEFAULT_BUDGET`-sized budget.
const BUDGET_REFILL_STEPS: u64 = 250_000;

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
    let board = Board::uno(backend.as_ref(), &mut runtime, firmware);

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
        // A run this long dispatches more callbacks than one Runtime budget
        // allows. Peripheral state lives in the backend and every scheduled
        // callback is a stateless Native, so refilling the budget is
        // behaviorally equivalent to building a fresh Runtime, and cheaper.
        if step > 0 && step % BUDGET_REFILL_STEPS == 0 {
            runtime.reset_budget();
        }
        board.step(backend.as_ref(), &mut runtime);
        if step % 100_000 == 0 {
            let pc = backend.get(board.cpu_handle, "pc").number() as i64;
            stalled_samples = if pc == last_pc {
                stalled_samples + 1
            } else {
                0
            };
            last_pc = pc;
            pb5_samples.push(board.read_data(backend.as_ref(), PORTB) as i64 & PB5);
        }
    }

    let cycles = board.cycles(backend.as_ref());
    let millis_coarse = board.read_data(backend.as_ref(), GPIOR0) as i64;
    let tx_drained = board.read_data(backend.as_ref(), GPIOR1) as i64;
    let int0_before = board.read_data(backend.as_ref(), GPIOR2) as i64;

    // Fresh budget for the interrupt phase, so budget accounting cannot be
    // confused with the INT0 assertion below.
    runtime.reset_budget();

    // Drive PD2 low then high so the CHANGE-mode INT0 handler must fire.
    for level in [false, true] {
        board.set_input(backend.as_ref(), &mut runtime, board.portd, 2, level);
    }
    board.run_cycles(backend.as_ref(), &mut runtime, 200_000);
    let int0_after = board.read_data(backend.as_ref(), GPIOR2) as i64;

    let eeprom = board.eeprom_bytes(backend.as_ref())[0];

    // The board helpers must agree with direct register observation.
    let expected_led = if board.read_bit(backend.as_ref(), PORTB, LED_BUILTIN_PIN) {
        PinState::High
    } else {
        PinState::Low
    };
    assert_eq!(
        board.led_builtin(backend.as_ref(), &mut runtime),
        expected_led,
        "led_builtin disagrees with the PORTB5 output latch"
    );
    assert_eq!(
        board.pin_state(backend.as_ref(), &mut runtime, board.portd, 2),
        PinState::InputPullUp,
        "PD2 is configured INPUT_PULLUP but does not report as pulled up"
    );

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
