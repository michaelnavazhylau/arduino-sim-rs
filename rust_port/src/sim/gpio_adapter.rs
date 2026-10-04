//! GPIO object dispatch, register hooks and synchronous listener boundaries.
use super::{as_handle, cpu::Cpu, gpio::*, Object, Simulator};
use crate::runtime::{deep_equal, Handle, Runtime, Value};
use std::{cell::RefCell, rc::Rc};

impl Simulator {
    pub(super) fn gpio_rc(&self, handle: Handle) -> Rc<RefCell<GpioPort>> {
        match &self.objects.borrow()[handle.0] {
            Object::Gpio(port) => port.clone(),
            other => panic!("expected GPIO, found {}", other.name()),
        }
    }

    pub(super) fn gpio_get(&self, handle: Handle, key: &str) -> Value {
        let port = self.gpio_rc(handle);
        let port = port.borrow();
        if let Some(value) = port.props.get(key) {
            return value.clone();
        }
        match key {
            "openCollector" => Value::Number(port.open_collector as f64),
            "externalClockListeners" => port.external_clock_listeners.clone(),
            "addListener" | "removeListener" | "pinState" | "setPin" | "timerOverridePin" => {
                Value::Method(Box::new(Value::Handle(handle)), key.into())
            }
            _ => panic!("unknown GPIO property: {key}"),
        }
    }

    pub(super) fn gpio_set(&self, handle: Handle, key: &str, value: Value) {
        let port = self.gpio_rc(handle);
        let mut port = port.borrow_mut();
        match key {
            "openCollector" => port.open_collector = value.number() as u8,
            "externalClockListeners" => port.external_clock_listeners = value,
            "addListener" | "removeListener" | "pinState" | "setPin" | "timerOverridePin" => {
                port.props.insert(key.into(), value);
            }
            _ => panic!("unknown GPIO assignment: {key}"),
        }
    }

    pub(super) fn construct_gpio(&self, args: &[Value]) -> Value {
        let cpu = as_handle(&args[0]);
        let port = Rc::new(RefCell::new(GpioPort::new(
            cpu,
            parse_port_config(&args[1]),
        )));
        let handle = self.push(Object::Gpio(port.clone()));
        self.with_cpu_mut(cpu, |cpu| port.borrow().hooks(cpu, handle));
        Value::Handle(handle)
    }

    pub(super) fn gpio_call(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        let port = self.gpio_rc(handle);
        match name {
            "addListener" => {
                port.borrow_mut().listeners.push(args[0].clone());
                Value::Undefined
            }
            "removeListener" => {
                port.borrow_mut()
                    .listeners
                    .retain(|value| !deep_equal(value, &args[0]));
                Value::Undefined
            }
            "pinState" => {
                let owner = port.borrow().cpu;
                Value::Number(self.with_cpu(owner, |cpu| {
                    port.borrow().pin_state(cpu, args[0].number() as u8)
                }) as f64)
            }
            "setPin" => {
                let owner = port.borrow().cpu;
                self.with_detached_cpu(owner, |cpu| {
                    let mut pending = Vec::new();
                    port.borrow_mut().set_pin(
                        cpu,
                        args[0].number() as u8,
                        args[1].truthy(),
                        &mut pending,
                    );
                    self.invoke_pending(runtime, owner, cpu, pending);
                });
                Value::Undefined
            }
            "timerOverridePin" => {
                let owner = port.borrow().cpu;
                self.with_detached_cpu(owner, |cpu| {
                    let pin = args[0].number() as u8;
                    let mode = args[1].number() as i64;
                    let mask = 1u8 << pin;
                    {
                        let mut port = port.borrow_mut();
                        if mode == PIN_OVERRIDE_NONE {
                            port.override_mask |= mask;
                            port.override_value &= !mask;
                        } else {
                            port.override_mask &= !mask;
                            match mode {
                                PIN_OVERRIDE_ENABLE => {
                                    port.override_value = (port.override_value & !mask)
                                        | (cpu.data[port.config.port] & mask);
                                }
                                PIN_OVERRIDE_SET => port.override_value |= mask,
                                PIN_OVERRIDE_CLEAR => port.override_value &= !mask,
                                PIN_OVERRIDE_TOGGLE => port.override_value ^= mask,
                                _ => panic!("unknown PinOverrideMode: {mode}"),
                            }
                        }
                    }
                    let config = port.borrow().config.clone();
                    self.gpio_output(
                        runtime,
                        owner,
                        cpu,
                        &port,
                        cpu.data[config.port],
                        cpu.data[config.ddr],
                    );
                });
                Value::Undefined
            }
            _ => panic!("unknown GPIO method: {name}"),
        }
    }

    /// Commit output state, invoke listeners, then calculate PIN from the state
    /// they left behind. This matches writeGpio -> updatePinRegister in upstream.
    #[allow(clippy::too_many_arguments)]
    fn gpio_output(
        &self,
        runtime: &mut Runtime,
        owner: Handle,
        cpu: &mut Cpu,
        port: &Rc<RefCell<GpioPort>>,
        value: u8,
        ddr: u8,
    ) {
        let mut pending = Vec::new();
        port.borrow_mut().write_gpio(value, ddr, &mut pending);
        self.invoke_pending(runtime, owner, cpu, pending);
        let mut pending = Vec::new();
        port.borrow_mut()
            .update_pin_register(ddr, cpu, &mut pending);
        self.invoke_pending(runtime, owner, cpu, pending);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn gpio_write(
        &self,
        runtime: &mut Runtime,
        owner: Handle,
        port_handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
        value: i64,
        mask: i64,
    ) {
        let port = self.gpio_rc(port_handle);
        let config = port.borrow().config.clone();
        let value = value as u8;
        if addr == config.ddr || addr == config.port || addr == config.pin {
            let (value, ddr) = if addr == config.ddr {
                cpu.data[config.ddr] = value;
                (cpu.data[config.port], value)
            } else {
                let value = if addr == config.pin {
                    cpu.data[config.port] ^ (value & mask as u8)
                } else {
                    value
                };
                cpu.data[config.port] = value;
                (value, cpu.data[config.ddr])
            };
            self.gpio_output(runtime, owner, cpu, &port, value, ddr);
            return;
        }
        let ports: Vec<_> = cpu
            .gpio_ports
            .iter()
            .map(|handle| self.gpio_rc(*handle))
            .collect();
        if let Some(change) = &config.pin_change {
            if addr == change.pcifr {
                for port in &ports {
                    if let Some(interrupt) = &port.borrow().pcint {
                        cpu.clear_interrupt_by_flag(interrupt, value);
                    }
                }
                return;
            }
            if addr == change.pcmsk {
                cpu.data[addr] = value;
                for port in &ports {
                    if let Some(interrupt) = &port.borrow().pcint {
                        cpu.update_interrupt_enable(interrupt, value);
                    }
                }
                return;
            }
        }
        let is_mask = config
            .external_interrupts
            .iter()
            .flatten()
            .any(|ext| addr == ext.eimsk);
        let is_flag = config
            .external_interrupts
            .iter()
            .flatten()
            .any(|ext| addr == ext.eifr);
        let is_control = config
            .external_interrupts
            .iter()
            .flatten()
            .any(|ext| addr == ext.eicr);
        assert!(
            is_mask || is_flag || is_control,
            "unknown GPIO hook {addr:#x}"
        );
        if !is_flag {
            cpu.data[addr] = value;
        }
        for port in ports {
            let port = port.borrow();
            for interrupt in port.external_ints.iter().flatten() {
                if is_mask {
                    cpu.update_interrupt_enable(interrupt, value);
                }
                if is_flag && !interrupt.constant {
                    cpu.clear_interrupt_by_flag(interrupt, value);
                }
            }
            port.check_external_interrupts(cpu);
        }
    }
}
