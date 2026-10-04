//! Timer registry dispatch and synchronous, reentrant GPIO boundaries.
use super::{
    as_handle,
    cpu::Cpu,
    timer::{Timer, TimerIo},
    Object, Simulator,
};
use crate::runtime::{Handle, Runtime, Value};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

impl Simulator {
    fn take_timer(&self, handle: Handle) -> Box<Timer> {
        let mut objects = self.objects.borrow_mut();
        assert!(
            matches!(&objects[handle.0], Object::Timer(_)),
            "expected AVRTimer, found {}",
            objects[handle.0].name()
        );
        match std::mem::replace(&mut objects[handle.0], Object::Taken) {
            Object::Timer(timer) => timer,
            _ => unreachable!("timer kind checked before detaching"),
        }
    }
    fn put_timer(&self, handle: Handle, timer: Box<Timer>) {
        let mut objects = self.objects.borrow_mut();
        assert!(matches!(objects[handle.0], Object::Taken));
        objects[handle.0] = Object::Timer(timer);
    }
    fn with_timer_cpu<R>(&self, handle: Handle, f: impl FnOnce(&mut Timer, &mut Cpu) -> R) -> R {
        let mut timer = self.take_timer(handle);
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.with_detached_cpu(timer.cpu, |cpu| f(&mut timer, cpu))
        }));
        self.put_timer(handle, timer);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
    pub(super) fn construct_timer(&self, runtime: &mut Runtime, args: &[Value]) -> Value {
        let owner = as_handle(&args[0]);
        let handle = Handle(self.objects.borrow().len());
        let timer = Timer::new(owner, super::timer::parse_config(&args[1]), handle);
        self.with_cpu_mut(owner, |cpu| timer.hooks(cpu, handle));
        // Hook installation below does not invoke callbacks.
        self.push(Object::Timer(Box::new(timer)));
        self.with_timer_cpu(handle, |timer, cpu| {
            timer.update_wgm(
                cpu,
                &mut TimerBus {
                    sim: self,
                    runtime,
                    handle,
                },
            );
        });
        Value::Handle(handle)
    }
    pub(super) fn timer_get(&self, handle: Handle, key: &str) -> Value {
        if matches!(key, "reset" | "count") {
            let objects = self.objects.borrow();
            if let Object::Timer(timer) = &objects[handle.0] {
                if let Some(value) = timer.props.get(key) {
                    return value.clone();
                }
            }
            return Value::Method(Box::new(Value::Handle(handle)), key.into());
        }
        let objects = self.objects.borrow();
        match &objects[handle.0] {
            Object::Timer(timer) => match &objects[timer.cpu.0] {
                Object::Cpu(cpu) => timer.get(cpu, key),
                _ => panic!("timer property requested while CPU detached"),
            },
            _ => panic!("timer property requested while timer detached"),
        }
    }
    pub(super) fn timer_set(&self, handle: Handle, key: &str, value: Value) {
        assert!(
            matches!(key, "reset" | "count"),
            "unknown timer assignment: {key}"
        );
        match &mut self.objects.borrow_mut()[handle.0] {
            Object::Timer(timer) => {
                timer.props.insert(key.into(), value);
            }
            _ => panic!("timer assignment while detached"),
        }
    }
    pub(super) fn timer_call(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        self.with_timer_cpu(handle, |timer, cpu| match name {
            "reset" => timer.reset(),
            "count" => timer.count(
                cpu,
                &mut TimerBus {
                    sim: self,
                    runtime,
                    handle,
                },
                args.first().is_none_or(Value::truthy),
                args.get(1).is_some_and(Value::truthy),
            ),
            "external" => {
                if args[0].truthy() == timer.external_rising {
                    timer.count(
                        cpu,
                        &mut TimerBus {
                            sim: self,
                            runtime,
                            handle,
                        },
                        false,
                        true,
                    );
                }
            }
            _ => panic!("unknown timer method: {name}"),
        });
        Value::Undefined
    }
    pub(super) fn timer_read(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
    ) -> u8 {
        self.with_timer_detached(handle, |timer| {
            timer.read(
                cpu,
                &mut TimerBus {
                    sim: self,
                    runtime,
                    handle,
                },
                addr,
            )
        })
    }
    pub(super) fn timer_write(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
        value: i64,
    ) {
        self.with_timer_detached(handle, |timer| {
            timer.write(
                cpu,
                &mut TimerBus {
                    sim: self,
                    runtime,
                    handle,
                },
                addr,
                value as u8,
            )
        });
    }
    fn with_timer_detached<R>(&self, handle: Handle, f: impl FnOnce(&mut Timer) -> R) -> R {
        let mut timer = self.take_timer(handle);
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut timer)));
        self.put_timer(handle, timer);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
}

struct TimerBus<'a> {
    sim: &'a Simulator,
    runtime: &'a mut Runtime,
    handle: Handle,
}
impl TimerIo for TimerBus<'_> {
    fn output(&mut self, timer: &mut Timer, cpu: &mut Cpu, channel: usize, mode: i64) {
        let Some(port) = cpu.gpio_by_port.get(&timer.config.ports[channel]).copied() else {
            return;
        };
        let pin = timer.config.pins[channel];
        let owner = timer.cpu;
        // Publish both live objects while the port invokes listeners or spies.
        let inert = Timer::new(owner, timer.config.clone(), self.handle);
        let live = std::mem::replace(timer, inert);
        self.sim.put_timer(self.handle, Box::new(live));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let callback = self.sim.gpio_get(port, "timerOverridePin");
            self.sim.invoke_from_cpu(
                self.runtime,
                owner,
                cpu,
                callback,
                vec![Value::Number(pin as f64), Value::Number(mode as f64)],
            )
        }));
        *timer = *self.sim.take_timer(self.handle);
        if let Err(error) = result {
            resume_unwind(error);
        }
    }
    fn external_listener(&mut self, port: Handle, pin: u8, callback: Option<Value>) {
        let port = self.sim.gpio_rc(port);
        let listeners = port.borrow().external_clock_listeners.clone();
        let Value::Array(listeners) = listeners else {
            panic!("invalid external clock listener array");
        };
        let mut listeners = listeners.borrow_mut();
        let pin = pin as usize;
        if listeners.len() <= pin {
            listeners.resize(pin + 1, Value::Undefined);
        }
        listeners[pin] = callback.unwrap_or(Value::Null);
    }
}
