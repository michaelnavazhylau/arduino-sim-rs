// SPDX-License-Identifier: MIT

//! Regression cases beyond the converted upstream inventory.
use avr_sim::{native_backend, runtime::Handle, scenario::*, Backend, Case, Runtime, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

fn execute(backend: Rc<dyn Backend>, body: Vec<Step>) {
    Runtime::new(backend).run(&Case {
        source: "native regression",
        name: "native regression",
        line: 1,
        assertions: assertion_count(&body),
        setup: vec![],
        body,
    });
}
fn equal(actual: Expr, expected: Expr) -> Step {
    at(1, check(actual, "toEqual", false, vec![expected]))
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
fn data(addr: f64) -> Expr {
    get(get(var("cpu"), text("data")), num(addr))
}
fn cpu_method(name: &'static str, args: Vec<Expr>) -> Step {
    at(1, eval(call(get(var("cpu"), text(name)), args)))
}

#[test]
fn adc_flags_use_original_operands_and_register_aliases() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                eval(call(
                    get(get(var("cpu"), text("progBytes")), text("set")),
                    vec![get(
                        call(var("assemble"), vec![text("ADC r0, r1\nADC r0, r0")]),
                        text("bytes"),
                    )],
                )),
            ),
            at(1, eval(assign(data(0.0), num(127.0), "="))),
            at(1, eval(assign(data(1.0), num(1.0), "="))),
            at(1, eval(call(var("avrInstruction"), vec![var("cpu")]))),
            equal(data(0.0), num(128.0)),
            equal(data(95.0), num(44.0)),
            at(1, eval(call(var("avrInstruction"), vec![var("cpu")]))),
            equal(data(0.0), num(0.0)),
            equal(data(95.0), num(27.0)),
        ],
    );
}

#[test]
fn gpio_listener_reenters_cpu_during_out_before_pin_and_pc_update() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "port",
                    new("AVRIOPort", vec![var("cpu"), var("portBConfig")]),
                ),
            ),
            cpu_method("writeData", vec![num(36.0), num(1.0)]),
            at(1, eval(assign(data(16.0), num(1.0), "="))),
            at(
                1,
                eval(call(
                    get(get(var("cpu"), text("progBytes")), text("set")),
                    vec![get(
                        call(var("assemble"), vec![text("OUT 5, r16")]),
                        text("bytes"),
                    )],
                )),
            ),
            at(
                1,
                bind(
                    "listener",
                    call(
                        get(var("vi"), text("fn")),
                        vec![function(
                            vec![],
                            vec![
                                equal(get(var("cpu"), text("pc")), num(0.0)),
                                equal(data(37.0), num(1.0)),
                                equal(data(35.0), num(0.0)),
                                equal(
                                    call(get(var("port"), text("pinState")), vec![num(0.0)]),
                                    num(1.0),
                                ),
                                cpu_method("writeData", vec![num(256.0), num(85.0)]),
                            ],
                        )],
                    ),
                ),
            ),
            at(
                1,
                eval(call(
                    get(var("port"), text("addListener")),
                    vec![var("listener")],
                )),
            ),
            at(1, eval(call(var("avrInstruction"), vec![var("cpu")]))),
            equal(data(256.0), num(85.0)),
            equal(data(35.0), num(1.0)),
            equal(get(var("cpu"), text("pc")), num(1.0)),
            at(
                1,
                check(
                    var("listener"),
                    "toHaveBeenCalledTimes",
                    false,
                    vec![num(1.0)],
                ),
            ),
        ],
    );
}

#[test]
fn cpu_host_hooks_reenter_and_control_raw_write_fallback() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(1, eval(assign(data(64.0), num(10.0), "="))),
            at(
                1,
                eval(assign(
                    get(get(var("cpu"), text("writeHooks")), num(64.0)),
                    function(
                        vec![
                            param("value", false),
                            param("old", false),
                            param("addr", false),
                            param("mask", false),
                        ],
                        vec![
                            equal(var("value"), num(20.0)),
                            equal(var("old"), num(10.0)),
                            equal(var("addr"), num(64.0)),
                            equal(var("mask"), num(15.0)),
                            cpu_method("writeData", vec![num(256.0), num(7.0)]),
                            at(1, ret(boolean(true))),
                        ],
                    ),
                    "=",
                )),
            ),
            cpu_method("writeData", vec![num(64.0), num(20.0), num(15.0)]),
            equal(data(64.0), num(10.0)),
            equal(data(256.0), num(7.0)),
            at(
                1,
                eval(assign(
                    get(get(var("cpu"), text("writeHooks")), num(64.0)),
                    function(vec![], vec![at(1, ret(boolean(false)))]),
                    "=",
                )),
            ),
            cpu_method("writeData", vec![num(64.0), num(30.0)]),
            equal(data(64.0), num(30.0)),
            at(
                1,
                eval(assign(
                    get(get(var("cpu"), text("readHooks")), num(64.0)),
                    function(
                        vec![param("addr", false)],
                        vec![
                            equal(var("addr"), num(64.0)),
                            equal(data(64.0), num(30.0)),
                            at(1, ret(num(99.0))),
                        ],
                    ),
                    "=",
                )),
            ),
            equal(
                call(get(var("cpu"), text("readData")), vec![num(64.0)]),
                num(99.0),
            ),
            equal(data(64.0), num(30.0)),
        ],
    );
}

#[test]
fn timer_gpio_listener_reenters_same_timer_and_cpu() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "port",
                    new("AVRIOPort", vec![var("cpu"), var("portBConfig")]),
                ),
            ),
            at(
                1,
                bind(
                    "timer",
                    new("AVRTimer", vec![var("cpu"), var("timer1Config")]),
                ),
            ),
            cpu_method("writeData", vec![num(36.0), num(2.0)]),
            cpu_method("writeData", vec![num(128.0), num(64.0)]),
            cpu_method("writeData", vec![num(136.0), num(1.0)]),
            cpu_method("writeData", vec![num(129.0), num(1.0)]),
            at(
                1,
                bind(
                    "listener",
                    call(
                        get(var("vi"), text("fn")),
                        vec![function(
                            vec![],
                            vec![
                                equal(get(var("timer"), text("debugTCNT")), num(1.0)),
                                equal(
                                    call(get(var("cpu"), text("readData")), vec![num(132.0)]),
                                    num(1.0),
                                ),
                                equal(
                                    call(get(var("port"), text("pinState")), vec![num(1.0)]),
                                    num(1.0),
                                ),
                            ],
                        )],
                    ),
                ),
            ),
            at(
                1,
                eval(call(
                    get(var("port"), text("addListener")),
                    vec![var("listener")],
                )),
            ),
            at(
                1,
                eval(assign(get(var("cpu"), text("cycles")), num(1.0), "=")),
            ),
            cpu_method("tick", vec![]),
            at(
                1,
                eval(assign(get(var("cpu"), text("cycles")), num(2.0), "=")),
            ),
            cpu_method("tick", vec![]),
            equal(get(var("timer"), text("debugTCNT")), num(1.0)),
            at(
                1,
                check(
                    var("listener"),
                    "toHaveBeenCalledTimes",
                    false,
                    vec![num(1.0)],
                ),
            ),
        ],
    );
}

#[test]
fn tiny_chains_shared_interrupt_register_hooks_with_timer0() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                eval(new(
                    "AVRTimer",
                    vec![
                        var("cpu"),
                        object(vec![
                            spread(var("timer0Config")),
                            entry("TIFR", num(88.0)),
                            entry("TIMSK", num(89.0)),
                        ]),
                    ],
                )),
            ),
            at(
                1,
                eval(new(
                    "ATtinyTimer1",
                    vec![var("cpu"), var("attinyTimer1Config")],
                )),
            ),
            at(1, eval(assign(data(88.0), num(5.0), "="))),
            cpu_method("writeData", vec![num(89.0), num(5.0)]),
            equal(get(var("cpu"), text("nextInterrupt")), num(4.0)),
            cpu_method("writeData", vec![num(88.0), num(5.0)]),
            equal(data(88.0), num(0.0)),
            equal(get(var("cpu"), text("nextInterrupt")), num(-1.0)),
        ],
    );
}

#[test]
fn tiny_gtccr_chains_existing_host_hook_after_self_clearing_bits() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                eval(assign(
                    get(get(var("cpu"), text("writeHooks")), num(76.0)),
                    function(
                        vec![
                            param("value", false),
                            param("old", false),
                            param("addr", false),
                            param("mask", false),
                        ],
                        vec![
                            equal(var("value"), num(1.0)),
                            equal(var("old"), num(0.0)),
                            equal(var("addr"), num(76.0)),
                            equal(var("mask"), num(255.0)),
                            at(1, eval(assign(data(76.0), var("value"), "="))),
                        ],
                    ),
                    "=",
                )),
            ),
            at(
                1,
                eval(new(
                    "ATtinyTimer1",
                    vec![var("cpu"), var("attinyTimer1Config")],
                )),
            ),
            cpu_method("writeData", vec![num(76.0), num(3.0)]),
            equal(data(76.0), num(1.0)),
        ],
    );
}

#[test]
fn tiny_phase_pwm_changes_direction_and_sets_output_on_down_compare() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "port",
                    new(
                        "AVRIOPort",
                        vec![
                            var("cpu"),
                            object(vec![
                                entry("PIN", num(54.0)),
                                entry("DDR", num(55.0)),
                                entry("PORT", num(56.0)),
                                entry("externalInterrupts", array(vec![])),
                            ]),
                        ],
                    ),
                ),
            ),
            at(
                1,
                eval(new(
                    "ATtinyTimer1",
                    vec![var("cpu"), var("attinyTimer1Config")],
                )),
            ),
            cpu_method("writeData", vec![num(55.0), num(2.0)]),
            cpu_method("writeData", vec![num(77.0), num(2.0)]),
            cpu_method("writeData", vec![num(78.0), num(1.0)]),
            cpu_method("writeData", vec![num(80.0), num(97.0)]),
            at(
                1,
                eval(assign(get(var("cpu"), text("cycles")), num(1.0), "=")),
            ),
            cpu_method("tick", vec![]),
            at(
                1,
                eval(assign(get(var("cpu"), text("cycles")), num(4.0), "=")),
            ),
            cpu_method("tick", vec![]),
            equal(
                call(get(var("cpu"), text("readData")), vec![num(79.0)]),
                num(1.0),
            ),
            equal(data(54.0), num(2.0)),
            at(
                1,
                eval(assign(get(var("cpu"), text("cycles")), num(5.0), "=")),
            ),
            cpu_method("tick", vec![]),
            equal(
                call(get(var("cpu"), text("readData")), vec![num(79.0)]),
                num(0.0),
            ),
            equal(binary("&", data(88.0), num(4.0)), num(4.0)),
        ],
    );
}

#[test]
fn callback_panic_restores_cpu_registry_ownership() {
    let backend = native_backend();
    let result = catch_unwind(AssertUnwindSafe(|| {
        execute(
            backend.clone(),
            vec![
                cpu_setup(),
                at(
                    1,
                    bind(
                        "port",
                        new("AVRIOPort", vec![var("cpu"), var("portBConfig")]),
                    ),
                ),
                at(
                    1,
                    eval(call(
                        get(var("port"), text("addListener")),
                        vec![function(
                            vec![],
                            vec![at(1, throw(text("listener failure")))],
                        )],
                    )),
                ),
                cpu_method("writeData", vec![num(37.0), num(1.0)]),
            ],
        )
    }));
    assert!(result.is_err());
    // First construction in this private test registry is the CPU.
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
