// SPDX-License-Identifier: MIT

//! Ownership restoration and source ordering at peripheral host boundaries.
use avr_sim::{native_backend, runtime::Handle, scenario::*, Backend, Case, Runtime, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};
fn execute(backend: Rc<dyn Backend>, body: Vec<Step>) {
    Runtime::new(backend).run(&Case {
        source: "peripheral recovery",
        name: "peripheral recovery",
        line: 1,
        assertions: assertion_count(&body),
        setup: vec![],
        body,
    });
}
fn equal(a: Expr, b: Expr) -> Step {
    at(1, check(a, "toEqual", false, vec![b]))
}
fn prop(owner: &'static str, name: &'static str) -> Expr {
    get(var(owner), text(name))
}
fn method(owner: &'static str, name: &'static str, args: Vec<Expr>) -> Step {
    at(1, eval(call(prop(owner, name), args)))
}
fn write(addr: f64, value: f64) -> Step {
    method("cpu", "writeData", vec![num(addr), num(value)])
}
fn cpu_setup() -> Step {
    at(
        1,
        bind(
            "cpu",
            new("CPU", vec![new("Uint16Array", vec![num(1024.0)])]),
        ),
    )
}
fn cycles(value: f64) -> Step {
    at(1, eval(assign(prop("cpu", "cycles"), num(value), "=")))
}
fn data(addr: f64) -> Expr {
    get(prop("cpu", "data"), num(addr))
}
fn assert_cpu_usable(backend: Rc<dyn Backend>) {
    assert_eq!(backend.get(Handle(0), "pc").number(), 0.0);
    let mut runtime = Runtime::new(backend.clone());
    backend.call(
        &mut runtime,
        Some(Handle(0)),
        "writeData",
        vec![Value::Number(256.0), Value::Number(42.0)],
    );
    assert_eq!(
        backend
            .call(
                &mut runtime,
                Some(Handle(0)),
                "readData",
                vec![Value::Number(256.0)]
            )
            .number(),
        42.0
    );
}

#[test]
fn eeprom_read_callback_runs_before_halt_cycles_and_can_reenter_cpu() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "backend",
                    object(vec![entry(
                        "readMemory",
                        function(
                            vec![param("address", false)],
                            vec![
                                equal(var("address"), num(1.0)),
                                equal(prop("cpu", "cycles"), num(0.0)),
                                equal(data(65.0), num(1.0)),
                                write(256.0, 42.0),
                                at(1, ret(num(85.0))),
                            ],
                        ),
                    )]),
                ),
            ),
            at(1, eval(new("AVREEPROM", vec![var("cpu"), var("backend")]))),
            write(65.0, 1.0),
            write(63.0, 1.0),
            equal(data(64.0), num(85.0)),
            equal(data(256.0), num(42.0)),
            equal(prop("cpu", "cycles"), num(4.0)),
        ],
    );
}

#[test]
fn eeprom_backend_panic_preserves_live_cpu_and_eeprom_state() {
    let backend = native_backend();
    let body = vec![
        cpu_setup(),
        at(
            1,
            bind(
                "memory",
                object(vec![entry(
                    "readMemory",
                    function(vec![], vec![at(1, throw(text("EEPROM backend failure")))]),
                )]),
            ),
        ),
        at(1, eval(new("AVREEPROM", vec![var("cpu"), var("memory")]))),
        write(63.0, 1.0),
    ];
    assert!(catch_unwind(AssertUnwindSafe(|| execute(backend.clone(), body))).is_err());
    assert!(matches!(
        backend.get(Handle(1), "backend"),
        Value::Object(_)
    ));
    assert_eq!(backend.get(Handle(0), "cycles").number(), 0.0);
    assert_cpu_usable(backend);
}

#[test]
fn twi_handler_panic_preserves_busy_state_and_allows_explicit_completion() {
    let backend = native_backend();
    let body = vec![
        cpu_setup(),
        at(
            1,
            bind(
                "twi",
                new("AVRTWI", vec![var("cpu"), var("twiConfig"), num(16e6)]),
            ),
        ),
        at(
            1,
            eval(assign(
                prop("twi", "eventHandler"),
                object(vec![entry(
                    "start",
                    function(vec![], vec![at(1, throw(text("TWI handler failure")))]),
                )]),
                "=",
            )),
        ),
        write(188.0, 164.0),
        cycles(1.0),
        method("cpu", "tick", vec![]),
    ];
    assert!(catch_unwind(AssertUnwindSafe(|| execute(backend.clone(), body))).is_err());
    assert!(backend.get(Handle(1), "busy").truthy());
    let mut runtime = Runtime::new(backend.clone());
    backend.call(&mut runtime, Some(Handle(1)), "completeStop", vec![]);
    assert!(!backend.get(Handle(1), "busy").truthy());
    assert_eq!(backend.get(Handle(1), "status").number(), 248.0);
    assert_cpu_usable(backend);
}

#[test]
fn watchdog_reset_calls_through_cpu_reset_spy_at_timeout() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind("clock", new("AVRClock", vec![var("cpu"), num(128000.0)])),
            ),
            at(
                1,
                eval(new(
                    "AVRWatchdog",
                    vec![var("cpu"), var("watchdogConfig"), var("clock")],
                )),
            ),
            at(
                1,
                bind(
                    "reset",
                    call(
                        get(var("vi"), text("spyOn")),
                        vec![var("cpu"), text("reset")],
                    ),
                ),
            ),
            write(96.0, 24.0),
            write(96.0, 8.0),
            cycles(2048.0),
            method("cpu", "tick", vec![]),
            at(
                1,
                check(var("reset"), "toHaveBeenCalledTimes", false, vec![num(1.0)]),
            ),
            equal(data(84.0), num(8.0)),
            equal(prop("cpu", "SP"), num(8447.0)),
        ],
    );
}

#[test]
fn watchdog_reset_callback_panic_restores_both_objects() {
    let backend = native_backend();
    let body = vec![
        cpu_setup(),
        at(
            1,
            bind("clock", new("AVRClock", vec![var("cpu"), num(128000.0)])),
        ),
        at(
            1,
            bind(
                "watchdog",
                new(
                    "AVRWatchdog",
                    vec![var("cpu"), var("watchdogConfig"), var("clock")],
                ),
            ),
        ),
        at(
            1,
            eval(assign(
                prop("watchdog", "resetWatchdog"),
                function(
                    vec![],
                    vec![at(1, throw(text("watchdog callback failure")))],
                ),
                "=",
            )),
        ),
        write(96.0, 24.0),
        write(96.0, 8.0),
    ];
    assert!(catch_unwind(AssertUnwindSafe(|| execute(backend.clone(), body))).is_err());
    assert!(backend.get(Handle(2), "enabled").truthy());
    assert_cpu_usable(backend);
}
