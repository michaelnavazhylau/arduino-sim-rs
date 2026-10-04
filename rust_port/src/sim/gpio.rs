//! Native port of `avr8js/src/peripherals/gpio.ts`.
//!
//! Output changes collect listener notifications; the adapter invokes them with
//! no registry borrows held, before updating PIN, preserving synchronous reentry.
use super::cpu::{Cpu, InterruptConfig};
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;

pub const PIN_STATE_LOW: i64 = 0;
pub const PIN_STATE_HIGH: i64 = 1;
pub const PIN_STATE_INPUT: i64 = 2;
pub const PIN_STATE_INPUT_PULLUP: i64 = 3;

pub const PIN_OVERRIDE_NONE: i64 = 0;
pub const PIN_OVERRIDE_ENABLE: i64 = 1;
pub const PIN_OVERRIDE_SET: i64 = 2;
pub const PIN_OVERRIDE_CLEAR: i64 = 3;
pub const PIN_OVERRIDE_TOGGLE: i64 = 4;

pub const MODE_LOW_LEVEL: u8 = 0;
pub const MODE_CHANGE: u8 = 1;
pub const MODE_FALLING_EDGE: u8 = 2;
pub const MODE_RISING_EDGE: u8 = 3;

/// A deferred host callback: listener or external-clock notification.
pub type PendingCall = (Value, Vec<Value>);

#[derive(Clone)]
pub struct ExternalInterruptConfig {
    pub eicr: usize,
    pub eimsk: usize,
    pub eifr: usize,
    pub isc_offset: u8,
    pub index: u8,
    pub interrupt: u8,
}

#[derive(Clone)]
pub struct PinChangeConfig {
    pub pcie: u8,
    pub pcicr: usize,
    pub pcifr: usize,
    pub pcmsk: usize,
    pub pin_change_interrupt: u8,
    pub mask: u8,
    pub offset: u8,
}

#[derive(Clone)]
pub struct PortConfig {
    pub pin: usize,
    pub ddr: usize,
    pub port: usize,
    pub pin_change: Option<PinChangeConfig>,
    pub external_interrupts: Vec<Option<ExternalInterruptConfig>>,
}

pub struct GpioPort {
    pub cpu: Handle,
    pub config: PortConfig,
    pub listeners: Vec<Value>,
    pub props: BTreeMap<String, Value>,
    /// Shared array so scenarios can install callbacks by index.
    pub external_clock_listeners: Value,
    pub external_ints: Vec<Option<InterruptConfig>>,
    pub pcint: Option<InterruptConfig>,
    pub pin_value: u8,
    pub override_mask: u8,
    pub override_value: u8,
    pub last_value: u8,
    pub last_ddr: u8,
    pub last_pin: u8,
    pub open_collector: u8,
}

impl GpioPort {
    pub fn new(cpu: Handle, config: PortConfig) -> Self {
        let external_ints = config
            .external_interrupts
            .iter()
            .map(|external| {
                external.as_ref().map(|external| InterruptConfig {
                    address: external.interrupt,
                    flag_register: external.eifr as u16,
                    flag_mask: 1 << external.index,
                    enable_register: external.eimsk as u16,
                    enable_mask: 1 << external.index,
                    constant: false,
                    inverse_flag: false,
                })
            })
            .collect();
        let pcint = config
            .pin_change
            .as_ref()
            .map(|pin_change| InterruptConfig {
                address: pin_change.pin_change_interrupt,
                flag_register: pin_change.pcifr as u16,
                flag_mask: 1 << pin_change.pcie,
                enable_register: pin_change.pcicr as u16,
                enable_mask: 1 << pin_change.pcie,
                constant: false,
                inverse_flag: false,
            });
        Self {
            cpu,
            config,
            listeners: Vec::new(),
            props: BTreeMap::new(),
            external_clock_listeners: Value::array(Vec::new()),
            external_ints,
            pcint,
            pin_value: 0,
            override_mask: 0xff,
            override_value: 0,
            last_value: 0,
            last_ddr: 0,
            last_pin: 0,
            open_collector: 0,
        }
    }

    /// Register this port on its CPU and install the register write hooks.
    pub fn hooks(&self, cpu: &mut Cpu, handle: Handle) {
        cpu.install_write_hook(self.config.ddr, handle);
        cpu.install_write_hook(self.config.port, handle);
        cpu.install_write_hook(self.config.pin, handle);
        if let Some(pin_change) = &self.config.pin_change {
            cpu.install_write_hook(pin_change.pcifr, handle);
            cpu.install_write_hook(pin_change.pcmsk, handle);
        }
        for external in self.config.external_interrupts.iter().flatten() {
            cpu.install_write_hook(external.eicr, handle);
            cpu.install_write_hook(external.eimsk, handle);
            cpu.install_write_hook(external.eifr, handle);
        }
        cpu.gpio_ports.push(handle);
        cpu.gpio_by_port.insert(self.config.port as u16, handle);
    }

    pub fn pin_state(&self, cpu: &Cpu, index: u8) -> i64 {
        let ddr = cpu.data[self.config.ddr];
        let port = cpu.data[self.config.port];
        let bit_mask = 1u8 << index;
        let open_state = if port & bit_mask != 0 {
            PIN_STATE_INPUT_PULLUP
        } else {
            PIN_STATE_INPUT
        };
        let high_value = if self.open_collector & bit_mask != 0 {
            open_state
        } else {
            PIN_STATE_HIGH
        };
        if ddr & bit_mask != 0 {
            if self.last_value & bit_mask != 0 {
                high_value
            } else {
                PIN_STATE_LOW
            }
        } else {
            open_state
        }
    }

    pub fn set_pin(
        &mut self,
        cpu: &mut Cpu,
        index: u8,
        value: bool,
        pending: &mut Vec<PendingCall>,
    ) {
        let bit_mask = 1u8 << index;
        self.pin_value &= !bit_mask;
        if value {
            self.pin_value |= bit_mask;
        }
        let ddr = cpu.data[self.config.ddr];
        self.update_pin_register(ddr, cpu, pending);
    }

    pub fn timer_override_pin(
        &mut self,
        cpu: &mut Cpu,
        pin: u8,
        mode: i64,
        pending: &mut Vec<PendingCall>,
    ) {
        let pin_mask = 1u8 << pin;
        if mode == PIN_OVERRIDE_NONE {
            self.override_mask |= pin_mask;
            self.override_value &= !pin_mask;
        } else {
            self.override_mask &= !pin_mask;
            match mode {
                PIN_OVERRIDE_ENABLE => {
                    self.override_value &= !pin_mask;
                    self.override_value |= cpu.data[self.config.port] & pin_mask;
                }
                PIN_OVERRIDE_SET => self.override_value |= pin_mask,
                PIN_OVERRIDE_CLEAR => self.override_value &= !pin_mask,
                PIN_OVERRIDE_TOGGLE => self.override_value ^= pin_mask,
                other => panic!("unknown PinOverrideMode: {other}"),
            }
        }
        let ddr = cpu.data[self.config.ddr];
        let value = cpu.data[self.config.port];
        self.write_gpio(value, ddr, pending);
        self.update_pin_register(ddr, cpu, pending);
    }

    pub fn ddr_write(&mut self, cpu: &mut Cpu, value: i64, pending: &mut Vec<PendingCall>) {
        let port_value = cpu.data[self.config.port];
        cpu.data[self.config.ddr] = value as u8;
        let ddr = value as u8;
        self.write_gpio(port_value, ddr, pending);
        self.update_pin_register(ddr, cpu, pending);
    }

    pub fn port_write(&mut self, cpu: &mut Cpu, value: i64, pending: &mut Vec<PendingCall>) {
        let ddr = cpu.data[self.config.ddr];
        cpu.data[self.config.port] = value as u8;
        self.write_gpio(value as u8, ddr, pending);
        self.update_pin_register(ddr, cpu, pending);
    }

    pub fn pin_write(
        &mut self,
        cpu: &mut Cpu,
        value: i64,
        mask: i64,
        pending: &mut Vec<PendingCall>,
    ) {
        let old_port = cpu.data[self.config.port];
        let ddr = cpu.data[self.config.ddr];
        let port_value = old_port ^ ((value as u8) & (mask as u8));
        cpu.data[self.config.port] = port_value;
        self.write_gpio(port_value, ddr, pending);
        self.update_pin_register(ddr, cpu, pending);
    }

    pub fn write_gpio(&mut self, value: u8, ddr: u8, pending: &mut Vec<PendingCall>) {
        let new_value =
            (((value & self.override_mask) | self.override_value) & ddr) | (value & !ddr);
        let previous = self.last_value;
        if new_value != previous || ddr != self.last_ddr {
            self.last_value = new_value;
            self.last_ddr = ddr;
            for listener in &self.listeners {
                pending.push((
                    listener.clone(),
                    vec![
                        Value::Number(new_value as f64),
                        Value::Number(previous as f64),
                    ],
                ));
            }
        }
    }

    pub fn update_pin_register(&mut self, ddr: u8, cpu: &mut Cpu, pending: &mut Vec<PendingCall>) {
        let new_pin = (self.pin_value & !ddr) | (self.last_value & ddr);
        cpu.data[self.config.pin] = new_pin;
        if self.last_pin != new_pin {
            for index in 0..8u8 {
                let before = self.last_pin & (1 << index) != 0;
                let after = new_pin & (1 << index) != 0;
                if before != after {
                    self.toggle_interrupt(index, after, cpu);
                    if let Some(callback) =
                        listener_at(&self.external_clock_listeners, index as usize)
                    {
                        pending.push((callback, vec![Value::Bool(after)]));
                    }
                }
            }
            self.last_pin = new_pin;
        }
    }

    fn toggle_interrupt(&mut self, pin: u8, rising_edge: bool, cpu: &mut Cpu) {
        let external_config = self
            .config
            .external_interrupts
            .get(pin as usize)
            .and_then(|external| external.clone());
        let external = self
            .external_ints
            .get(pin as usize)
            .and_then(|external| external.clone());
        if let (Some(config), Some(mut external)) = (external_config, external) {
            if cpu.data[config.eimsk] & (1 << config.index) != 0 {
                let configuration = (cpu.data[config.eicr] >> config.isc_offset) & 0x3;
                external.constant = configuration == MODE_LOW_LEVEL;
                let generate_interrupt = match configuration {
                    MODE_LOW_LEVEL | MODE_FALLING_EDGE => !rising_edge,
                    MODE_CHANGE => true,
                    _ => rising_edge,
                };
                self.external_ints[pin as usize] = Some(external.clone());
                if generate_interrupt {
                    cpu.set_interrupt_flag(&external);
                } else if external.constant {
                    cpu.clear_interrupt(&external, true);
                }
            }
        }
        if let Some(pin_change) = self.config.pin_change.clone() {
            if let Some(pcint) = self.pcint.clone() {
                if pin_change.mask & (1 << pin) != 0
                    && cpu.data[pin_change.pcmsk] & (1 << (pin + pin_change.offset)) != 0
                {
                    cpu.set_interrupt_flag(&pcint);
                }
            }
        }
    }

    /// Re-arm low-level external interrupts while their pin stays low.
    pub fn check_external_interrupts(&self, cpu: &mut Cpu) {
        for pin in 0..8u8 {
            let Some(external) = self
                .config
                .external_interrupts
                .get(pin as usize)
                .and_then(|external| external.as_ref())
            else {
                continue;
            };
            let pin_value = self.last_pin & (1 << pin) != 0;
            if cpu.data[external.eimsk] & (1 << external.index) == 0 || pin_value {
                continue;
            }
            let configuration = (cpu.data[external.eicr] >> external.isc_offset) & 0x3;
            if configuration == MODE_LOW_LEVEL {
                cpu.queue_interrupt(InterruptConfig {
                    address: external.interrupt,
                    flag_register: external.eifr as u16,
                    flag_mask: 1 << external.index,
                    enable_register: external.eimsk as u16,
                    enable_mask: 1 << external.index,
                    constant: true,
                    inverse_flag: false,
                });
            }
        }
    }
}

fn listener_at(array: &Value, index: usize) -> Option<Value> {
    match array {
        Value::Array(values) => values
            .borrow()
            .get(index)
            .cloned()
            .filter(|value| !matches!(value, Value::Undefined | Value::Null)),
        _ => None,
    }
}

/// Registered GPIO port constants, matching upstream's exported configs.
pub fn pin_change(pcie: u8, pcicr: usize, pcifr: usize, pcmsk: usize, interrupt: u8) -> Value {
    Value::object(BTreeMap::from([
        ("PCIE".into(), Value::Number(pcie as f64)),
        ("PCICR".into(), Value::Number(pcicr as f64)),
        ("PCIFR".into(), Value::Number(pcifr as f64)),
        ("PCMSK".into(), Value::Number(pcmsk as f64)),
        ("pinChangeInterrupt".into(), Value::Number(interrupt as f64)),
        ("mask".into(), Value::Number(255.0)),
        ("offset".into(), Value::Number(0.0)),
    ]))
}

pub fn external_interrupt(eicr: usize, index: u8, isc_offset: u8, interrupt: u8) -> Value {
    Value::object(BTreeMap::from([
        ("EICR".into(), Value::Number(eicr as f64)),
        ("EIMSK".into(), Value::Number(0x3d as f64)),
        ("EIFR".into(), Value::Number(0x3c as f64)),
        ("index".into(), Value::Number(index as f64)),
        ("iscOffset".into(), Value::Number(isc_offset as f64)),
        ("interrupt".into(), Value::Number(interrupt as f64)),
    ]))
}

fn port_config(
    pin: usize,
    ddr: usize,
    port: usize,
    pin_change: Option<Value>,
    external_interrupts: Vec<Value>,
) -> Value {
    Value::object(BTreeMap::from([
        ("PIN".into(), Value::Number(pin as f64)),
        ("DDR".into(), Value::Number(ddr as f64)),
        ("PORT".into(), Value::Number(port as f64)),
        ("pinChange".into(), pin_change.unwrap_or(Value::Undefined)),
        (
            "externalInterrupts".into(),
            Value::array(external_interrupts),
        ),
    ]))
}

/// The exported port configs referenced by scenarios.
pub fn port_configs() -> BTreeMap<&'static str, Value> {
    let int0 = external_interrupt(0x69, 0, 0, 2);
    let int1 = external_interrupt(0x69, 1, 2, 4);
    let pcint0 = pin_change(0, 0x68, 0x3b, 0x6b, 6);
    let pcint1 = pin_change(1, 0x68, 0x3b, 0x6c, 8);
    let pcint2 = pin_change(2, 0x68, 0x3b, 0x6d, 10);
    BTreeMap::from([
        ("portAConfig", port_config(0x20, 0x21, 0x22, None, vec![])),
        (
            "portBConfig",
            port_config(0x23, 0x24, 0x25, Some(pcint0), vec![]),
        ),
        (
            "portCConfig",
            port_config(0x26, 0x27, 0x28, Some(pcint1), vec![]),
        ),
        (
            "portDConfig",
            port_config(
                0x29,
                0x2a,
                0x2b,
                Some(pcint2),
                vec![Value::Null, Value::Null, int0, int1],
            ),
        ),
        ("portEConfig", port_config(0x2c, 0x2d, 0x2e, None, vec![])),
        ("portFConfig", port_config(0x2f, 0x30, 0x31, None, vec![])),
        ("portGConfig", port_config(0x32, 0x33, 0x34, None, vec![])),
        (
            "portHConfig",
            port_config(0x100, 0x101, 0x102, None, vec![]),
        ),
        (
            "portJConfig",
            port_config(0x103, 0x104, 0x105, None, vec![]),
        ),
        (
            "portKConfig",
            port_config(0x106, 0x107, 0x108, None, vec![]),
        ),
        (
            "portLConfig",
            port_config(0x109, 0x10a, 0x10b, None, vec![]),
        ),
    ])
}

fn field(config: &Value, key: &str) -> Value {
    match config {
        Value::Object(fields) => fields
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or(Value::Undefined),
        other => panic!("expected port config object, got {other:?}"),
    }
}

fn number(config: &Value, key: &str) -> usize {
    field(config, key).number() as usize
}

/// Convert an exported/overridden config object into native form.
pub fn parse_port_config(config: &Value) -> PortConfig {
    let pin_change = match field(config, "pinChange") {
        Value::Undefined | Value::Null => None,
        value => Some(PinChangeConfig {
            pcie: value.number_field("PCIE") as u8,
            pcicr: value.number_field("PCICR") as usize,
            pcifr: value.number_field("PCIFR") as usize,
            pcmsk: value.number_field("PCMSK") as usize,
            pin_change_interrupt: value.number_field("pinChangeInterrupt") as u8,
            mask: value.number_field("mask") as u8,
            offset: value.number_field("offset") as u8,
        }),
    };
    let external_interrupts = match field(config, "externalInterrupts") {
        Value::Array(values) => values
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Undefined | Value::Null => None,
                value => Some(ExternalInterruptConfig {
                    eicr: value.number_field("EICR") as usize,
                    eimsk: value.number_field("EIMSK") as usize,
                    eifr: value.number_field("EIFR") as usize,
                    isc_offset: value.number_field("iscOffset") as u8,
                    index: value.number_field("index") as u8,
                    interrupt: value.number_field("interrupt") as u8,
                }),
            })
            .collect(),
        other => panic!("expected externalInterrupts array, got {other:?}"),
    };
    PortConfig {
        pin: number(config, "PIN"),
        ddr: number(config, "DDR"),
        port: number(config, "PORT"),
        pin_change,
        external_interrupts,
    }
}

trait NumberField {
    fn number_field(&self, key: &str) -> f64;
}
impl NumberField for Value {
    fn number_field(&self, key: &str) -> f64 {
        match self {
            Value::Object(fields) => fields
                .borrow()
                .get(key)
                .map(|value| value.number())
                .unwrap_or_else(|| panic!("missing config field {key}")),
            other => panic!("expected object field {key}, got {other:?}"),
        }
    }
}
