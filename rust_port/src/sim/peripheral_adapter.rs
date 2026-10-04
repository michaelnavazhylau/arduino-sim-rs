//! Registry adapter for the remaining peripherals. Never hold a registry borrow
//! across host callbacks; publish actual live objects and restore on unwinding.
use super::{
    as_handle,
    cpu::Cpu,
    peripheral::{fields, number, Peripheral, PeripheralIo, State},
    Object, Simulator,
};
use crate::runtime::{Backend, Handle, Runtime, Value};
use std::{
    collections::BTreeMap,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
};

impl Simulator {
    fn take_peripheral(&self, handle: Handle) -> Box<Peripheral> {
        let mut objects = self.objects.borrow_mut();
        assert!(
            matches!(&objects[handle.0], Object::Peripheral(_)),
            "expected native peripheral, found {}",
            objects[handle.0].name()
        );
        match std::mem::replace(&mut objects[handle.0], Object::Taken) {
            Object::Peripheral(p) => p,
            _ => unreachable!("checked peripheral kind"),
        }
    }
    fn put_peripheral(&self, handle: Handle, peripheral: Box<Peripheral>) {
        let mut objects = self.objects.borrow_mut();
        assert!(matches!(&objects[handle.0], Object::Taken));
        objects[handle.0] = Object::Peripheral(peripheral);
    }
    fn with_peripheral<R>(&self, handle: Handle, f: impl FnOnce(&mut Peripheral) -> R) -> R {
        let mut p = self.take_peripheral(handle);
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut p)));
        self.put_peripheral(handle, p);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
    pub(super) fn construct_peripheral(&self, kind: &str, args: &[Value]) -> Value {
        if kind == "EEPROMMemoryBackend" {
            let size = args[0].number();
            assert!(
                size.is_finite() && size >= 0.0,
                "invalid EEPROM memory size"
            );
            return Value::Handle(self.push(Object::EepromMemory {
                memory: Value::buffer(vec![0xff; size as usize], 1),
                props: BTreeMap::new(),
            }));
        }
        if kind == "NoopTWIEventHandler" {
            return super::twi::noop_handler(as_handle(&args[0]));
        }
        let cpu = as_handle(&args[0]);
        let handle = Handle(self.objects.borrow().len());
        let (name, config, freq, state, hooks) = match kind {
            "AVREEPROM" => (
                "AVREEPROM",
                args.get(2)
                    .cloned()
                    .unwrap_or_else(super::eeprom::default_config),
                0.0,
                State::Eeprom {
                    backend: args[1].clone(),
                    enabled_until: 0,
                    complete_at: 0,
                },
                vec!["EECR"],
            ),
            "AVRADC" => {
                let channels = fields(&args[1])["numChannels"].number() as usize;
                (
                    "AVRADC",
                    args[1].clone(),
                    0.0,
                    State::Adc {
                        channels: Value::array(vec![Value::Undefined; channels]),
                        avcc: 5.0,
                        aref: 5.0,
                        converting: false,
                        conversion_cycles: 25,
                    },
                    vec!["ADCSRA"],
                )
            }
            "AVRSPI" => (
                "AVRSPI",
                args[1].clone(),
                args[2].number(),
                State::Spi { active: false },
                vec!["SPCR", "SPSR", "SPDR"],
            ),
            "AVRUSART" => (
                "AVRUSART",
                args[1].clone(),
                args[2].number(),
                State::Usart {
                    busy: false,
                    rx_byte: 0,
                    line: String::new(),
                },
                vec!["UCSRA", "UCSRB", "UCSRC", "UBRRH", "UBRRL", "UDR"],
            ),
            "AVRTWI" => (
                "AVRTWI",
                args[1].clone(),
                args[2].number(),
                State::Twi { busy: false },
                vec!["TWCR"],
            ),
            "AVRWatchdog" => (
                "AVRWatchdog",
                args[1].clone(),
                0.0,
                State::Watchdog {
                    clock: as_handle(&args[2]),
                    change_until: 0,
                    timeout: 0,
                    enabled: false,
                    scheduled: false,
                },
                vec!["WDTCSR"],
            ),
            _ => panic!("unimplemented native constructor: {kind}"),
        };
        let mut p = Peripheral::new(cpu, handle, name, config, freq, state);
        if name == "AVRTWI" {
            p.props
                .insert("eventHandler".into(), super::twi::noop_handler(handle));
        }
        self.with_cpu_mut(cpu, |cpu| {
            for hook in hooks {
                cpu.install_write_hook(p.reg(hook), handle);
            }
            if name == "AVRUSART" {
                p.usart_reset(cpu);
                cpu.install_read_hook(p.reg("UDR"), handle);
            }
            if name == "AVRTWI" {
                p.twi_update_status(cpu, 0xf8);
            }
            if name == "AVRWatchdog" {
                cpu.props
                    .insert("onWatchdogReset".into(), p.callback("onWatchdogReset"));
            }
        });
        Value::Handle(self.push(Object::Peripheral(Box::new(p))))
    }
    pub(super) fn peripheral_get(&self, handle: Handle, name: &str) -> Value {
        let objects = self.objects.borrow();
        match &objects[handle.0] {
            Object::Peripheral(p) => self.with_cpu(p.cpu, |cpu| p.get(cpu, name)),
            Object::EepromMemory { memory, props } => {
                if let Some(value) = props.get(name) {
                    return value.clone();
                }
                match name {
                    "memory" => memory.clone(),
                    "readMemory" | "writeMemory" | "eraseMemory" => {
                        Value::Method(Box::new(Value::Handle(handle)), name.into())
                    }
                    _ => panic!("unimplemented EEPROM backend property: {name}"),
                }
            }
            _ => panic!("expected native peripheral"),
        }
    }
    pub(super) fn peripheral_set(&self, handle: Handle, name: &str, value: Value) {
        let mut objects = self.objects.borrow_mut();
        match &mut objects[handle.0] {
            Object::Peripheral(p) => {
                if let State::Adc {
                    avcc,
                    aref,
                    channels,
                    ..
                } = &mut p.state
                {
                    match name {
                        "avcc" => {
                            *avcc = value.number();
                            return;
                        }
                        "aref" => {
                            *aref = value.number();
                            return;
                        }
                        "channelValues" => {
                            assert!(matches!(&value, Value::Array(_)));
                            *channels = value;
                            return;
                        }
                        _ => {}
                    }
                }
                p.props.insert(name.into(), value);
            }
            Object::EepromMemory { props, .. } => {
                props.insert(name.into(), value);
            }
            _ => panic!("expected native peripheral"),
        }
    }
    pub(super) fn peripheral_call(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        if matches!(
            &self.objects.borrow()[handle.0],
            Object::EepromMemory { .. }
        ) {
            return self.eeprom_memory_call(handle, name, args);
        }
        self.with_peripheral(handle, |p| {
            self.with_detached_cpu(p.cpu, |cpu| {
                p.call(
                    cpu,
                    &mut PeripheralBus {
                        sim: self,
                        runtime,
                        handle,
                    },
                    name,
                    &args,
                )
            })
        })
    }
    fn eeprom_memory_call(&self, handle: Handle, name: &str, args: Vec<Value>) -> Value {
        let memory = {
            let objects = self.objects.borrow();
            match &objects[handle.0] {
                Object::EepromMemory { memory, .. } => memory.clone(),
                _ => unreachable!(),
            }
        };
        let Value::Buffer { bytes, .. } = memory else {
            unreachable!()
        };
        let addr = number(&args[0]) as usize;
        match name {
            "readMemory" => {
                return bytes
                    .borrow()
                    .get(addr)
                    .map_or(Value::Undefined, |value| Value::Number(*value as f64))
            }
            "eraseMemory" => {
                if let Some(byte) = bytes.borrow_mut().get_mut(addr) {
                    *byte = 0xff;
                }
            }
            "writeMemory" => {
                if let Some(byte) = bytes.borrow_mut().get_mut(addr) {
                    *byte &= number(&args[1]) as u8;
                }
            }
            _ => panic!("unimplemented EEPROM backend method: {name}"),
        }
        Value::Undefined
    }
    pub(super) fn peripheral_read(&self, handle: Handle, cpu: &mut Cpu, addr: usize) -> u8 {
        self.with_peripheral(handle, |p| p.read(cpu, addr))
    }
    pub(super) fn peripheral_write(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
        value: i64,
        mask: i64,
    ) {
        let consumed = self.with_peripheral(handle, |p| {
            p.write(
                cpu,
                &mut PeripheralBus {
                    sim: self,
                    runtime,
                    handle,
                },
                addr,
                value as u8,
            )
        });
        if !consumed {
            cpu.write_data(addr, value, mask);
        }
    }
}
struct PeripheralBus<'a> {
    sim: &'a Simulator,
    runtime: &'a mut Runtime,
    handle: Handle,
}
impl PeripheralIo for PeripheralBus<'_> {
    fn invoke(
        &mut self,
        p: &mut Peripheral,
        cpu: &mut Cpu,
        target: Option<Value>,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        let owner = p.cpu;
        let live = std::mem::replace(p, Peripheral::vacant(owner, self.handle));
        self.sim.put_peripheral(self.handle, Box::new(live));
        let live_cpu = std::mem::replace(cpu, Cpu::new(Vec::new(), 0));
        self.sim.put_cpu(owner, Box::new(live_cpu));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let target = target.unwrap_or(Value::Handle(self.handle));
            let callback = match target {
                Value::Handle(handle) => self.sim.get(handle, name),
                Value::Object(fields) => fields
                    .borrow()
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| panic!("missing host callback: {name}")),
                other => panic!("invalid host callback target: {other:?}"),
            };
            self.runtime.invoke(callback, args)
        }));
        *cpu = *self.sim.take_cpu(owner);
        *p = *self.sim.take_peripheral(self.handle);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
    fn clock_frequency(&self, handle: Handle) -> f64 {
        let objects = self.sim.objects.borrow();
        match &objects[handle.0] {
            Object::Clock(clock) => clock.frequency(),
            _ => panic!("watchdog requires an AVRClock"),
        }
    }
}
