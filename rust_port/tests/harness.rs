// SPDX-License-Identifier: MIT

use avr_sim::{
    runtime::{deep_equal, Handle},
    scenario::*,
    Backend, Runtime, Value,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

/// Harness fixture only: deliberately not an AVR CPU or a passing simulator stub.
#[derive(Default)]
struct Fixture {
    fields: RefCell<BTreeMap<String, Value>>,
    events: RefCell<Vec<(Value, Value)>>,
    calls: RefCell<Vec<String>>,
}
impl Backend for Fixture {
    fn resolve(&self, name: &str) -> Value {
        match name {
            "fixture" => Value::Handle(Handle(0)),
            "avrInstruction" | "assemble" => Value::Native(name.into()),
            _ => panic!("unimplemented fixture global: {name}"),
        }
    }
    fn construct(&self, _: &mut Runtime, kind: &str, _: Vec<Value>) -> Value {
        panic!("fixture is not a simulator: {kind}")
    }
    fn get(&self, object: Handle, key: &str) -> Value {
        assert_eq!(object, Handle(0));
        if let Some(value) = self.fields.borrow().get(key) {
            return value.clone();
        }
        if ["emit", "echo", "schedule", "dispatch", "tick"].contains(&key) {
            Value::Method(Box::new(Value::Handle(object)), key.into())
        } else {
            panic!("unknown fixture field {key}")
        }
    }
    fn set(&self, _: &mut Runtime, object: Handle, key: &str, value: Value) {
        assert_eq!(object, Handle(0));
        self.fields.borrow_mut().insert(key.into(), value);
    }
    fn call(
        &self,
        runtime: &mut Runtime,
        receiver: Option<Handle>,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        self.calls.borrow_mut().push(name.into());
        match (receiver, name) {
            (Some(Handle(0)), "echo") => args[0].clone(),
            (Some(Handle(0)), "emit") => {
                let callback = self
                    .fields
                    .borrow()
                    .get("callback")
                    .expect("callback")
                    .clone();
                runtime.invoke(callback, args)
            }
            (Some(Handle(0)), "schedule") => {
                self.events
                    .borrow_mut()
                    .push((args[0].clone(), args[1].clone()));
                args[0].clone()
            }
            (Some(Handle(0)), "dispatch") => {
                let events = self.events.take();
                for (callback, arg) in events {
                    runtime.invoke(callback, vec![arg]);
                }
                Value::Undefined
            }
            (Some(Handle(0)), "tick") => Value::Undefined,
            (None, "avrInstruction") => {
                let pc = self.fields.borrow()["pc"].number();
                self.fields
                    .borrow_mut()
                    .insert("pc".into(), Value::Number(pc + 1.0));
                Value::Undefined
            }
            (None, "assemble") => {
                assert_eq!(args[0].string(), "NOP");
                Value::object(BTreeMap::from([
                    ("bytes".into(), Value::buffer(vec![0, 0], 1)),
                    ("errors".into(), Value::array(vec![])),
                    (
                        "lines".into(),
                        Value::array(vec![Value::object(BTreeMap::new())]),
                    ),
                    (
                        "labels".into(),
                        Value::object(BTreeMap::from([("start".into(), Value::Number(0.0))])),
                    ),
                ]))
            }
            _ => panic!("unimplemented fixture call {receiver:?}.{name}"),
        }
    }
    fn properties(&self, object: Handle) -> BTreeMap<String, Value> {
        assert_eq!(object, Handle(0));
        self.fields.borrow().clone()
    }
}
fn fixture() -> (Rc<Fixture>, Runtime) {
    let backend = Rc::new(Fixture::default());
    let runtime = Runtime::new(backend.clone());
    (backend, runtime)
}
fn execute(runtime: &mut Runtime, body: Vec<Step>) {
    runtime.run(&Case {
        source: "harness",
        name: "fixture",
        line: 1,
        assertions: assertion_count(&body),
        setup: vec![],
        body,
    });
}
fn property(name: &'static str) -> Expr {
    get(var("fixture"), text(name))
}
fn method(name: &'static str, args: Vec<Expr>) -> Expr {
    call(property(name), args)
}
fn vi(name: &'static str, args: Vec<Expr>) -> Expr {
    call(get(var("vi"), text(name)), args)
}
fn equal(actual: Expr, expected: Expr) -> Step {
    at(1, check(actual, "toEqual", false, vec![expected]))
}

#[test]
fn loops_capture_per_iteration_bindings_and_mutable_outer_state() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(1, bind("events", array(vec![]))),
            at(
                1,
                for_of(
                    "i",
                    array(vec![num(1.0), num(4.0), num(10.0)]),
                    vec![at(
                        1,
                        eval(method(
                            "schedule",
                            vec![
                                function(
                                    vec![param("cycle", false)],
                                    vec![at(
                                        1,
                                        eval(call(
                                            get(var("events"), text("push")),
                                            vec![array(vec![var("i"), var("cycle")])],
                                        )),
                                    )],
                                ),
                                var("i"),
                            ],
                        )),
                    )],
                ),
            ),
            at(1, eval(method("dispatch", vec![]))),
            equal(
                var("events"),
                array(vec![
                    array(vec![num(1.0), num(1.0)]),
                    array(vec![num(4.0), num(4.0)]),
                    array(vec![num(10.0), num(10.0)]),
                ]),
            ),
            at(1, bind("counter", num(0.0))),
            at(
                1,
                for_(
                    vec![at(1, bind("i", num(0.0)))],
                    binary("<", var("i"), num(4.0)),
                    update(var("i"), 1, false),
                    vec![at(1, eval(assign(var("counter"), num(2.0), "+=")))],
                ),
            ),
            equal(var("counter"), num(8.0)),
        ],
    );
}
#[test]
fn mock_callbacks_reenter_backend_and_count_nested_assertions() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(1, eval(assign(property("state"), num(0.0), "="))),
            at(
                1,
                bind(
                    "listener",
                    vi(
                        "fn",
                        vec![function(
                            vec![param("n", false)],
                            vec![
                                at(1, eval(assign(property("state"), var("n"), "="))),
                                equal(property("state"), num(7.0)),
                            ],
                        )],
                    ),
                ),
            ),
            at(1, eval(assign(property("callback"), var("listener"), "="))),
            at(1, eval(method("emit", vec![num(7.0)]))),
            at(
                1,
                check(
                    var("listener"),
                    "toHaveBeenCalledWith",
                    false,
                    vec![num(7.0)],
                ),
            ),
            at(
                1,
                check(
                    var("listener"),
                    "toHaveBeenCalledTimes",
                    false,
                    vec![num(1.0)],
                ),
            ),
            at(
                1,
                eval(call(get(var("listener"), text("mockClear")), vec![])),
            ),
            at(1, check(var("listener"), "toHaveBeenCalled", true, vec![])),
        ],
    );
    assert_eq!(runtime.assertions_executed, 4);
}
#[test]
fn spies_call_through_and_preserve_return_values() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(
                1,
                bind("spy", vi("spyOn", vec![var("fixture"), text("echo")])),
            ),
            equal(method("echo", vec![num(42.0)]), num(42.0)),
            at(
                1,
                check(var("spy"), "toHaveBeenCalledWith", false, vec![num(42.0)]),
            ),
        ],
    );
}
#[test]
fn mock_reset_removes_the_implementation_but_clear_does_not() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(1, bind("count", num(0.0))),
            at(
                1,
                bind(
                    "mock",
                    vi(
                        "fn",
                        vec![function(
                            vec![],
                            vec![at(1, eval(update(var("count"), 1, false)))],
                        )],
                    ),
                ),
            ),
            at(1, eval(call(var("mock"), vec![]))),
            at(1, eval(call(get(var("mock"), text("mockClear")), vec![]))),
            at(1, eval(call(var("mock"), vec![]))),
            equal(var("count"), num(2.0)),
            at(1, eval(call(get(var("mock"), text("mockReset")), vec![]))),
            at(1, eval(call(var("mock"), vec![]))),
            equal(var("count"), num(2.0)),
            at(
                1,
                check(var("mock"), "toHaveBeenCalledTimes", false, vec![num(1.0)]),
            ),
        ],
    );
}
#[test]
fn helpers_preserve_rest_parameters_destructuring_templates_and_spreads() {
    let (backend, mut runtime) = fixture();
    backend
        .fields
        .borrow_mut()
        .insert("a".into(), Value::Number(1.0));
    execute(
        &mut runtime,
        vec![
            at(
                1,
                bind(
                    "join",
                    function(
                        vec![param("items", true)],
                        vec![at(
                            1,
                            ret(call(get(var("items"), text("join")), vec![text("\n")])),
                        )],
                    ),
                ),
            ),
            equal(
                call(var("join"), vec![text("NOP"), text("BREAK")]),
                text("NOP\nBREAK"),
            ),
            at(
                1,
                bind(
                    "cfg",
                    object(vec![
                        spread(var("fixture")),
                        entry("a", num(2.0)),
                        entry("b", num(3.0)),
                    ]),
                ),
            ),
            at(
                1,
                destructure(vec![("a", "first"), ("b", "second")], var("cfg")),
            ),
            equal(
                template(vec![
                    text("value="),
                    binary("+", var("first"), var("second")),
                ]),
                text("value=5"),
            ),
            at(
                1,
                bind(
                    "filtered",
                    call(
                        get(array(vec![num(1.0), num(2.0), num(3.0)]), text("filter")),
                        vec![function(
                            vec![param("n", false)],
                            vec![at(1, ret(binary("<", var("n"), num(3.0))))],
                        )],
                    ),
                ),
            ),
            equal(var("filtered"), array(vec![num(1.0), num(2.0)])),
        ],
    );
}
#[test]
fn typed_arrays_narrow_and_alias_and_bitwise_operations_wrap() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(1, bind("bytes", new("Uint8Array", vec![num(2.0)]))),
            at(1, bind("alias", var("bytes"))),
            at(
                1,
                eval(assign(get(var("bytes"), num(0.0)), num(511.0), "=")),
            ),
            at(1, eval(assign(get(var("bytes"), num(1.0)), num(-1.0), "="))),
            equal(get(var("alias"), num(0.0)), num(255.0)),
            equal(get(var("alias"), num(1.0)), num(255.0)),
            equal(binary("<<", num(1.0), num(31.0)), num(-2147483648.0)),
            equal(binary("<<", num(1.0), num(32.0)), num(1.0)),
            equal(binary("/", num(5.0), num(2.0)), num(2.5)),
            equal(binary("&", unary("~", num(0.0)), num(255.0)), num(255.0)),
        ],
    );
    assert!(!deep_equal(&Value::Bool(true), &Value::Number(1.0)));
}
#[test]
fn asm_program_uses_little_endian_words_and_preserves_labels_and_lines() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![
            at(
                1,
                destructure(
                    vec![
                        ("program", "program"),
                        ("instructionCount", "count"),
                        ("labels", "labels"),
                    ],
                    call(var("asmProgram"), vec![text("NOP")]),
                ),
            ),
            equal(
                var("program"),
                new("Uint16Array", vec![array(vec![num(0.0)])]),
            ),
            equal(var("count"), num(1.0)),
            equal(get(var("labels"), text("start")), num(0.0)),
        ],
    );
}
fn program_fixture() -> (Rc<Fixture>, Runtime) {
    let (backend, runtime) = fixture();
    backend.fields.borrow_mut().extend([
        ("pc".into(), Value::Number(0.0)),
        ("progMem".into(), Value::buffer(vec![0, 0, 0x98, 0x95], 2)),
    ]);
    (backend, runtime)
}
#[test]
fn program_runner_executes_instructions_then_ticks_and_stops_before_break() {
    let (backend, mut runtime) = program_fixture();
    execute(
        &mut runtime,
        vec![
            at(
                1,
                bind(
                    "runner",
                    new(
                        "TestProgramRunner",
                        vec![var("fixture"), function(vec![], vec![at(1, ret(num(0.0)))])],
                    ),
                ),
            ),
            at(
                1,
                eval(call(get(var("runner"), text("runToBreak")), vec![])),
            ),
            equal(property("pc"), num(1.0)),
        ],
    );
    assert_eq!(*backend.calls.borrow(), vec!["avrInstruction", "tick"]);
}
#[test]
#[should_panic(expected = "BREAK instruction encountered")]
fn program_runner_default_break_callback_is_not_silently_ignored() {
    let (_, mut runtime) = program_fixture();
    execute(
        &mut runtime,
        vec![
            at(
                1,
                bind("runner", new("TestProgramRunner", vec![var("fixture")])),
            ),
            at(
                1,
                eval(call(get(var("runner"), text("runToBreak")), vec![])),
            ),
        ],
    );
}
#[test]
#[should_panic(expected = "Test program ran for too long")]
fn program_runner_enforces_iteration_limit() {
    let (_, mut runtime) = program_fixture();
    execute(
        &mut runtime,
        vec![
            at(
                1,
                bind(
                    "runner",
                    new(
                        "TestProgramRunner",
                        vec![var("fixture"), function(vec![], vec![])],
                    ),
                ),
            ),
            at(
                1,
                eval(call(
                    get(var("runner"), text("runUntil")),
                    vec![function(vec![], vec![at(1, ret(boolean(false)))]), num(2.0)],
                )),
            ),
        ],
    );
}
#[test]
#[should_panic(expected = "harness:19")]
fn failed_assertions_report_source_line() {
    let (_, mut runtime) = fixture();
    execute(
        &mut runtime,
        vec![at(19, check(num(1.0), "toEqual", false, vec![num(2.0)]))],
    );
}
#[test]
#[should_panic(expected = "unimplemented native constructor: NotAnAvrObject")]
fn native_backend_fails_on_unimplemented_operations() {
    // The native backend is real, but it must never fabricate default objects for
    // an operation it does not actually implement.
    let mut runtime = Runtime::new(avr_sim::native_backend());
    execute(
        &mut runtime,
        vec![at(1, bind("x", new("NotAnAvrObject", vec![])))],
    );
}
