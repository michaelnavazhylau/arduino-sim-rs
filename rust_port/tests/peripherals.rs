// SPDX-License-Identifier: MIT

//! Edge-case and reentry regressions beyond the pinned upstream scenarios.
use avr8rs::{native_backend, runtime::Handle, scenario::*, Backend, Case, Runtime, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
};

struct Fixture {
    backend: Rc<dyn Backend>,
    runtime: Runtime,
    cpu: Handle,
}
impl Fixture {
    fn new() -> Self {
        let backend = native_backend();
        let mut runtime = Runtime::new(backend.clone());
        let cpu =
            handle(backend.construct(&mut runtime, "CPU", vec![Value::buffer(vec![0; 2048], 2)]));
        Self {
            backend,
            runtime,
            cpu,
        }
    }
    fn construct(&mut self, kind: &str, args: Vec<Value>) -> Handle {
        handle(self.backend.construct(&mut self.runtime, kind, args))
    }
    fn peripheral(&mut self, kind: &str, config: &str) -> Handle {
        let cfg = self.backend.resolve(config);
        self.construct(
            kind,
            vec![Value::Handle(self.cpu), cfg, Value::Number(16e6)],
        )
    }
    fn call(&mut self, owner: Handle, name: &str, args: Vec<Value>) -> Value {
        self.backend
            .call(&mut self.runtime, Some(owner), name, args)
    }
    fn write(&mut self, addr: i64, value: i64) {
        self.call(self.cpu, "writeData", vec![n(addr), n(value)]);
    }
    fn read(&mut self, addr: i64) -> i64 {
        self.call(self.cpu, "readData", vec![n(addr)]).number() as i64
    }
    fn get(&self, owner: Handle, name: &str) -> Value {
        self.backend.get(owner, name)
    }
    fn set(&mut self, owner: Handle, name: &str, value: Value) {
        self.backend.set(&mut self.runtime, owner, name, value);
    }
    fn cycles(&mut self, cycles: i64) {
        self.set(self.cpu, "cycles", n(cycles));
    }
    fn tick(&mut self, cycles: i64) {
        self.cycles(cycles);
        self.call(self.cpu, "tick", vec![]);
    }
    fn watchdog(&mut self, frequency: f64) -> Handle {
        let clock = self.construct(
            "AVRClock",
            vec![Value::Handle(self.cpu), Value::Number(frequency)],
        );
        self.construct(
            "AVRWatchdog",
            vec![
                Value::Handle(self.cpu),
                self.backend.resolve("watchdogConfig"),
                Value::Handle(clock),
            ],
        )
    }
}
fn handle(value: Value) -> Handle {
    match value {
        Value::Handle(h) => h,
        _ => panic!("expected native handle"),
    }
}
fn n(value: i64) -> Value {
    Value::Number(value as f64)
}
fn execute(backend: Rc<dyn Backend>, body: Vec<Step>) {
    Runtime::new(backend).run(&Case {
        source: "peripheral regression",
        name: "peripheral regression",
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
fn property(owner: &'static str, name: &'static str) -> Expr {
    get(var(owner), text(name))
}
fn data(addr: f64) -> Expr {
    get(property("cpu", "data"), num(addr))
}
fn set(owner: &'static str, name: &'static str, value: Expr) -> Step {
    at(1, eval(assign(property(owner, name), value, "=")))
}
fn method(owner: &'static str, name: &'static str, args: Vec<Expr>) -> Step {
    at(1, eval(call(property(owner, name), args)))
}
fn write(addr: f64, value: f64) -> Step {
    method("cpu", "writeData", vec![num(addr), num(value)])
}
fn tick(cycles: f64) -> Vec<Step> {
    vec![
        set("cpu", "cycles", num(cycles)),
        method("cpu", "tick", vec![]),
    ]
}

#[test]
fn eeprom_backend_programming_only_clears_bits_and_erase_restores_them() {
    let mut f = Fixture::new();
    let memory = f.construct("EEPROMMemoryBackend", vec![n(2)]);
    f.call(memory, "writeMemory", vec![n(1), n(0x55)]);
    f.call(memory, "writeMemory", vec![n(1), n(0xf0)]);
    assert_eq!(
        f.call(memory, "readMemory", vec![n(1)]).number(),
        0x50 as f64
    );
    f.call(memory, "eraseMemory", vec![n(1)]);
    assert_eq!(f.call(memory, "readMemory", vec![n(1)]).number(), 255.0);
    f.call(memory, "writeMemory", vec![n(2), n(0)]);
    assert!(matches!(
        f.call(memory, "readMemory", vec![n(2)]),
        Value::Undefined
    ));
}

#[test]
fn eeprom_master_enable_expires_at_exactly_four_cycles() {
    for cycle in [3, 4] {
        let mut f = Fixture::new();
        let memory = f.construct("EEPROMMemoryBackend", vec![n(2)]);
        f.construct(
            "AVREEPROM",
            vec![Value::Handle(f.cpu), Value::Handle(memory)],
        );
        f.write(0x40, 0x55);
        f.write(0x41, 1);
        f.write(0x3f, 4);
        f.cycles(cycle);
        f.write(0x3f, 2);
        assert_eq!(
            f.call(memory, "readMemory", vec![n(1)]).number(),
            if cycle == 3 { 85.0 } else { 255.0 }
        );
        assert_eq!(
            f.get(f.cpu, "cycles").number(),
            if cycle == 3 { 5.0 } else { 4.0 }
        );
        assert_eq!(f.read(0x3f) & 2, if cycle == 3 { 2 } else { 0 });
    }
}

#[test]
fn adc_reference_selection_and_prescaler_cover_all_default_codes() {
    let mut f = Fixture::new();
    let adc = f.peripheral("AVRADC", "adcConfig");
    f.set(adc, "avcc", Value::Number(3.3));
    f.set(adc, "aref", Value::Number(2.0));
    for (code, voltage, kind) in [(0, 2.0, 1), (1, 3.3, 0), (2, 3.3, 4), (3, 1.1, 2)] {
        f.write(0x7c, code << 6);
        assert_eq!(f.get(adc, "referenceVoltage").number(), voltage);
        assert_eq!(f.get(adc, "referenceVoltageType").number(), kind as f64);
    }
    for code in 0..8 {
        f.write(0x7a, code);
        assert_eq!(f.get(adc, "prescaler").number(), (1 << code.max(1)) as f64);
    }
}

#[test]
fn adc_differential_left_adjust_clamping_and_subsequent_conversion_delay() {
    let mut f = Fixture::new();
    let cfg = f.backend.resolve("adcConfig");
    if let Value::Object(fields) = &cfg {
        fields.borrow_mut().insert(
            "muxChannels".into(),
            Value::object(std::collections::BTreeMap::from([(
                "0".into(),
                Value::object(std::collections::BTreeMap::from([
                    ("type".into(), n(1)),
                    ("positiveChannel".into(), n(1)),
                    ("negativeChannel".into(), n(2)),
                    ("gain".into(), n(2)),
                ])),
            )])),
        );
    }
    let adc = f.construct("AVRADC", vec![Value::Handle(f.cpu), cfg]);
    let Value::Array(channels) = f.get(adc, "channelValues") else {
        panic!("missing ADC channels")
    };
    channels.borrow_mut()[1] = Value::Number(2.0);
    channels.borrow_mut()[2] = Value::Number(0.5);
    f.write(0x7c, 0x60);
    f.write(0x7a, 0xc1);
    f.tick(49);
    assert_eq!(f.read(0x7a) & 0x40, 0x40);
    f.tick(50);
    assert_eq!(f.read(0x78), 128);
    assert_eq!(f.read(0x79), 153);
    assert_eq!(f.get(adc, "sampleCycles").number(), 26.0);
    channels.borrow_mut()[1] = n(0);
    channels.borrow_mut()[2] = n(4);
    f.write(0x7a, 0xc1);
    f.tick(75);
    assert_eq!(f.read(0x79), 153);
    f.tick(76);
    assert_eq!(f.read(0x78), 0);
    assert_eq!(f.read(0x79), 0);
    channels.borrow_mut()[1] = n(5);
    channels.borrow_mut()[2] = n(0);
    f.write(0x7a, 0xc1);
    f.tick(102);
    assert_eq!(f.read(0x78), 192);
    assert_eq!(f.read(0x79), 255);
}

#[test]
fn adc_callback_reenters_and_synchronously_completes_same_adc() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind("adc", new("AVRADC", vec![var("cpu"), var("adcConfig")])),
            ),
            set(
                "adc",
                "onADCRead",
                function(
                    vec![],
                    vec![
                        equal(property("adc", "converting"), boolean(true)),
                        equal(binary("&", data(122.0), num(64.0)), num(64.0)),
                        method("adc", "completeADCRead", vec![num(777.0)]),
                        equal(property("adc", "converting"), boolean(false)),
                        write(256.0, 42.0),
                    ],
                ),
            ),
            write(122.0, 193.0),
            equal(data(120.0), num(9.0)),
            equal(data(121.0), num(3.0)),
            equal(data(256.0), num(42.0)),
            equal(property("adc", "sampleCycles"), num(26.0)),
            equal(binary("&", data(122.0), num(64.0)), num(0.0)),
        ],
    );
}

#[test]
fn disabled_adc_completion_calls_through_completion_spy() {
    let mut body = vec![
        cpu_setup(),
        at(
            1,
            bind("adc", new("AVRADC", vec![var("cpu"), var("adcConfig")])),
        ),
        at(
            1,
            bind(
                "completion",
                call(
                    get(var("vi"), text("spyOn")),
                    vec![var("adc"), text("completeADCRead")],
                ),
            ),
        ),
        write(122.0, 64.0),
    ];
    body.extend(tick(50.0));
    body.extend([
        at(
            1,
            check(
                var("completion"),
                "toHaveBeenCalledWith",
                false,
                vec![num(0.0)],
            ),
        ),
        equal(data(120.0), num(0.0)),
        equal(data(121.0), num(0.0)),
    ]);
    execute(native_backend(), body);
}

#[test]
fn spi_transfer_callback_reenters_cpu_and_updates_transfer_timing() {
    let mut body = vec![
        cpu_setup(),
        at(
            1,
            bind(
                "spi",
                new("AVRSPI", vec![var("cpu"), var("spiConfig"), num(16e6)]),
            ),
        ),
        set(
            "spi",
            "onTransfer",
            function(
                vec![param("value", false)],
                vec![
                    equal(var("value"), num(85.0)),
                    equal(property("spi", "transmissionActive"), boolean(true)),
                    write(77.0, 1.0),
                    equal(property("spi", "transferCycles"), num(16.0)),
                    write(256.0, 7.0),
                    at(1, ret(num(123.0))),
                ],
            ),
        ),
        write(76.0, 80.0),
        write(78.0, 85.0),
        equal(data(78.0), num(0.0)),
    ];
    body.extend(tick(15.0));
    body.push(equal(data(78.0), num(0.0)));
    body.extend(tick(16.0));
    body.extend([
        equal(data(78.0), num(123.0)),
        equal(property("spi", "transmissionActive"), boolean(false)),
        equal(data(256.0), num(7.0)),
    ]);
    execute(native_backend(), body);
}

#[test]
fn usart_receive_interrupt_persists_until_masked_udr_read() {
    for (size, mask) in [(0, 31), (1, 63), (2, 127), (3, 255)] {
        let mut f = Fixture::new();
        let uart = f.peripheral("AVRUSART", "usart0Config");
        f.write(0xc2, size << 1);
        f.write(0xc1, 0x90);
        f.write(95, 0x80);
        assert!(matches!(
            f.call(uart, "writeByte", vec![n(511), Value::Bool(true)]),
            Value::Undefined
        ));
        f.tick(0);
        assert_eq!(f.get(f.cpu, "pc").number(), 36.0);
        assert_eq!(f.read(0xc0) & 0x80, 0x80);
        assert_eq!(f.read(0xc6), mask);
        assert_eq!(f.read(0xc6), 0);
        assert_eq!(f.read(0xc0) & 0x80, 0);
        assert_eq!(f.get(f.cpu, "nextInterrupt").number(), -1.0);
    }
}

#[test]
fn usart_disabling_receiver_drops_inflight_byte_at_completion() {
    let mut f = Fixture::new();
    let uart = f.peripheral("AVRUSART", "usart0Config");
    f.write(0xc1, 0x10);
    assert!(f.call(uart, "writeByte", vec![n(42)]).truthy());
    f.write(0xc1, 0);
    f.tick(159);
    assert!(f.get(uart, "rxBusy").truthy());
    f.tick(160);
    assert!(!f.get(uart, "rxBusy").truthy());
    assert_eq!(f.read(0xc0) & 0x80, 0);
    f.write(0xc1, 0x10);
    assert_eq!(f.read(0xc6), 0);
}

#[test]
fn usart_line_callback_reenters_before_line_clear_and_raw_udr_write() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "uart",
                    new("AVRUSART", vec![var("cpu"), var("usart0Config"), num(16e6)]),
                ),
            ),
            set(
                "uart",
                "onLineTransmit",
                function(
                    vec![param("line", false)],
                    vec![
                        equal(var("line"), text("A")),
                        equal(property("uart", "lineBuffer"), text("A")),
                        equal(data(198.0), num(65.0)),
                        write(198.0, 66.0),
                        equal(property("uart", "lineBuffer"), text("AB")),
                    ],
                ),
            ),
            write(198.0, 65.0),
            write(198.0, 10.0),
            equal(property("uart", "lineBuffer"), text("")),
            equal(data(198.0), num(10.0)),
        ],
    );
}

#[test]
fn usart_configuration_callback_observes_committed_configuration() {
    execute(
        native_backend(),
        vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "uart",
                    new("AVRUSART", vec![var("cpu"), var("usart0Config"), num(16e6)]),
                ),
            ),
            set(
                "uart",
                "onConfigurationChange",
                function(
                    vec![],
                    vec![
                        equal(property("uart", "bitsPerChar"), num(7.0)),
                        equal(data(194.0), num(4.0)),
                        write(256.0, 42.0),
                    ],
                ),
            ),
            write(194.0, 4.0),
            equal(data(256.0), num(42.0)),
        ],
    );
}

#[test]
fn twi_host_handler_can_synchronously_complete_each_master_phase() {
    let mut body = vec![
        cpu_setup(),
        at(
            1,
            bind(
                "twi",
                new("AVRTWI", vec![var("cpu"), var("twiConfig"), num(16e6)]),
            ),
        ),
        set(
            "twi",
            "eventHandler",
            object(vec![
                entry(
                    "start",
                    function(
                        vec![param("repeated", false)],
                        vec![
                            equal(var("repeated"), boolean(false)),
                            equal(property("twi", "busy"), boolean(true)),
                            method("twi", "completeStart", vec![]),
                        ],
                    ),
                ),
                entry(
                    "connectToSlave",
                    function(
                        vec![param("address", false), param("write", false)],
                        vec![
                            equal(var("address"), num(32.0)),
                            equal(var("write"), boolean(true)),
                            method("twi", "completeConnect", vec![boolean(true)]),
                        ],
                    ),
                ),
            ]),
        ),
        write(188.0, 164.0),
    ];
    body.extend(tick(1.0));
    body.extend([
        equal(property("twi", "status"), num(8.0)),
        equal(property("twi", "busy"), boolean(false)),
        write(187.0, 64.0),
        write(188.0, 132.0),
    ]);
    body.extend(tick(2.0));
    body.extend([
        equal(property("twi", "status"), num(24.0)),
        equal(property("twi", "busy"), boolean(false)),
    ]);
    execute(native_backend(), body);
}

#[test]
fn watchdog_combined_mode_interrupts_first_then_resets_and_preserves_ram() {
    let mut f = Fixture::new();
    let watchdog = f.watchdog(128000.0);
    f.write(256, 85);
    f.write(95, 128);
    f.set(f.cpu, "pc", n(100));
    f.write(96, 24);
    f.write(96, 72);
    f.tick(2048);
    assert_eq!(f.get(f.cpu, "pc").number(), 12.0);
    assert_eq!(f.read(96) & 64, 0);
    assert_eq!(f.get(watchdog, "watchdogTimeout").number(), 4096.0);
    f.tick(4096);
    assert_eq!(f.get(f.cpu, "pc").number(), 0.0);
    assert_eq!(f.read(84) & 8, 8);
    assert_eq!(f.read(256), 85);
    assert_eq!(f.get(f.cpu, "cycles").number(), 4096.0);
    assert_eq!(f.get(f.cpu, "SP").number(), 8447.0);
    assert!(!f.get(watchdog, "scheduled").truthy());
}

#[test]
fn watchdog_wdr_uses_current_clock_prescaler_and_protected_boundary() {
    let mut f = Fixture::new();
    let watchdog = f.watchdog(16e6);
    f.write(96, 24);
    f.cycles(4);
    f.write(96, 8);
    assert!(!f.get(watchdog, "enabled").truthy());
    f.write(96, 24);
    f.write(96, 8);
    assert_eq!(f.get(watchdog, "watchdogTimeout").number(), 256004.0);
    f.write(97, 128);
    f.write(97, 1); // CLKPR /2 through protected window
    let program = handle(f.get(f.cpu, "progBytes"));
    f.set(program, "0", n(0xa8));
    f.set(program, "1", n(0x95));
    f.cycles(200);
    f.backend.call(
        &mut f.runtime,
        None,
        "avrInstruction",
        vec![Value::Handle(f.cpu)],
    );
    assert_eq!(f.get(watchdog, "watchdogTimeout").number(), 128200.0);
    assert_eq!(f.get(f.cpu, "pc").number(), 1.0);
    assert_eq!(f.get(f.cpu, "cycles").number(), 201.0);
}

#[test]
fn peripheral_callback_panics_restore_cpu_and_peripheral_ownership() {
    for (kind, config, callback, writes, property, expected) in [
        (
            "AVRSPI",
            "spiConfig",
            "onByte",
            vec![write(76.0, 80.0), write(78.0, 1.0)],
            "transmissionActive",
            true,
        ),
        (
            "AVRADC",
            "adcConfig",
            "onADCRead",
            vec![write(122.0, 193.0)],
            "converting",
            true,
        ),
        (
            "AVRUSART",
            "usart0Config",
            "onConfigurationChange",
            vec![write(196.0, 10.0)],
            "rxBusy",
            false,
        ),
    ] {
        let backend = native_backend();
        let mut body = vec![
            cpu_setup(),
            at(
                1,
                bind(
                    "peripheral",
                    new(kind, vec![var("cpu"), var(config), num(16e6)]),
                ),
            ),
            set(
                "peripheral",
                callback,
                function(vec![], vec![at(1, throw(text("callback failure")))]),
            ),
        ];
        body.extend(writes);
        assert!(catch_unwind(AssertUnwindSafe(|| execute(backend.clone(), body))).is_err());
        assert_eq!(backend.get(Handle(0), "pc").number(), 0.0);
        assert_eq!(backend.get(Handle(1), property).truthy(), expected);
        let mut runtime = Runtime::new(backend.clone());
        backend.call(
            &mut runtime,
            Some(Handle(0)),
            "writeData",
            vec![n(256), n(42)],
        );
        assert_eq!(
            backend
                .call(&mut runtime, Some(Handle(0)), "readData", vec![n(256)])
                .number(),
            42.0
        );
    }
}
