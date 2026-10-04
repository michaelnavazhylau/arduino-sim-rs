//! Shared native dispatch contract for EEPROM, ADC, serial peripherals and watchdog.
//! Source-compatible callbacks execute synchronously through the adapter, not JS.
use super::cpu::{Cpu, InterruptConfig};
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;

pub(super) enum State {
    Eeprom {
        backend: Value,
        enabled_until: i64,
        complete_at: i64,
    },
    Adc {
        channels: Value,
        avcc: f64,
        aref: f64,
        converting: bool,
        conversion_cycles: i64,
    },
    Spi {
        active: bool,
    },
    Usart {
        busy: bool,
        rx_byte: i64,
        line: String,
    },
    Twi {
        busy: bool,
    },
    Watchdog {
        clock: Handle,
        change_until: i64,
        timeout: i64,
        enabled: bool,
        scheduled: bool,
    },
    Vacant,
}

pub(super) struct Peripheral {
    pub cpu: Handle,
    pub handle: Handle,
    pub kind: &'static str,
    pub config: BTreeMap<String, Value>,
    pub props: BTreeMap<String, Value>,
    pub freq: f64,
    pub state: State,
    event_serial: u64,
}

pub(super) trait PeripheralIo {
    /// Publish both peripheral and CPU before resolving and invoking the callback.
    fn invoke(
        &mut self,
        peripheral: &mut Peripheral,
        cpu: &mut Cpu,
        target: Option<Value>,
        name: &str,
        args: Vec<Value>,
    ) -> Value;
    fn clock_frequency(&self, clock: Handle) -> f64;
}

pub(super) fn fields(value: &Value) -> BTreeMap<String, Value> {
    match value {
        Value::Object(map) => map.borrow().clone(),
        other => panic!("expected native peripheral config, got {other:?}"),
    }
}
pub(super) fn config(entries: &[(&str, i64)]) -> Value {
    Value::object(
        entries
            .iter()
            .map(|(name, value)| (name.to_string(), Value::Number(*value as f64)))
            .collect(),
    )
}
pub(super) fn number(value: &Value) -> i64 {
    match value {
        Value::Number(n) => *n as i64,
        Value::Undefined | Value::Null => 0,
        Value::Bool(v) => i64::from(*v),
        other => panic!("expected numeric peripheral value, got {other:?}"),
    }
}
impl Peripheral {
    pub fn new(
        cpu: Handle,
        handle: Handle,
        kind: &'static str,
        config: Value,
        freq: f64,
        state: State,
    ) -> Self {
        Self {
            cpu,
            handle,
            kind,
            config: fields(&config),
            props: BTreeMap::new(),
            freq,
            state,
            event_serial: 0,
        }
    }
    pub fn vacant(cpu: Handle, handle: Handle) -> Self {
        Self {
            cpu,
            handle,
            kind: "detached peripheral",
            config: BTreeMap::new(),
            props: BTreeMap::new(),
            freq: 0.0,
            state: State::Vacant,
            event_serial: 0,
        }
    }
    pub fn reg(&self, name: &str) -> usize {
        self.config[name].number() as usize
    }
    pub fn int(&self, name: &str) -> i64 {
        self.config[name].number() as i64
    }
    pub fn interrupt(
        &self,
        address: &str,
        flag_register: &str,
        flag_mask: u8,
        enable_register: &str,
        enable_mask: u8,
    ) -> InterruptConfig {
        InterruptConfig {
            address: self.reg(address) as u8,
            flag_register: self.reg(flag_register) as u16,
            flag_mask,
            enable_register: self.reg(enable_register) as u16,
            enable_mask,
            constant: false,
            inverse_flag: false,
        }
    }
    pub fn callback(&self, name: &str) -> Value {
        self.props
            .get(name)
            .cloned()
            .unwrap_or_else(|| Value::Native(format!("peripheral:{}:{name}", self.handle.0)))
    }
    pub fn schedule(&mut self, cpu: &mut Cpu, name: &str, args: &[i64], delay: i64) {
        // Each scheduled closure has distinct identity; named arrow callbacks such
        // as watchdog.checkWatchdog use callback() directly instead.
        self.event_serial += 1;
        let payload = args
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        cpu.add_clock_event(
            Value::Native(format!(
                "peripheral:{}:{name}:{}:{payload}",
                self.handle.0, self.event_serial
            )),
            delay,
        );
    }
    pub fn invoke(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        io.invoke(self, cpu, None, name, args)
    }
    pub fn get(&self, cpu: &Cpu, name: &str) -> Value {
        if let Some(value) = self.props.get(name) {
            return value.clone();
        }
        match self.kind {
            "AVREEPROM" => match name {
                "backend" => match &self.state {
                    State::Eeprom { backend, .. } => backend.clone(),
                    _ => unreachable!(),
                },
                _ => panic!("unimplemented AVREEPROM property: {name}"),
            },
            "AVRADC" => self.adc_get(cpu, name),
            "AVRSPI" => self.spi_get(cpu, name),
            "AVRUSART" => self.usart_get(cpu, name),
            "AVRTWI" => self.twi_get(cpu, name),
            "AVRWatchdog" => self.watchdog_get(cpu, name),
            _ => panic!("invalid peripheral kind: {}", self.kind),
        }
    }
    pub fn write(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        addr: usize,
        value: u8,
    ) -> bool {
        match self.kind {
            "AVREEPROM" => self.eeprom_write(cpu, io, value),
            "AVRADC" => self.adc_write(cpu, io, value),
            "AVRSPI" => self.spi_write(cpu, io, addr, value),
            "AVRUSART" => self.usart_write(cpu, io, addr, value),
            "AVRTWI" => self.twi_write(cpu, value),
            "AVRWatchdog" => self.watchdog_write(cpu, io, value),
            _ => panic!("invalid peripheral kind: {}", self.kind),
        }
    }
    pub fn read(&mut self, cpu: &mut Cpu, addr: usize) -> u8 {
        assert_eq!(self.kind, "AVRUSART", "unexpected peripheral read hook");
        assert_eq!(addr, self.reg("UDR"));
        self.usart_read(cpu)
    }
    pub fn call(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: &[Value],
    ) -> Value {
        match self.kind {
            "AVREEPROM" => self.eeprom_call(cpu, name),
            "AVRADC" => self.adc_call(cpu, io, name, args),
            "AVRSPI" => self.spi_call(cpu, io, name, args),
            "AVRUSART" => self.usart_call(cpu, io, name, args),
            "AVRTWI" => self.twi_call(cpu, io, name, args),
            "AVRWatchdog" => self.watchdog_call(cpu, io, name),
            _ => panic!("invalid peripheral kind: {}", self.kind),
        }
    }
}
