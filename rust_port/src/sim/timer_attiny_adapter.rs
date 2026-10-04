// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! ATtiny Timer1 native dispatch, including shared-register hook chaining.
use super::{
    as_handle,
    cpu::Cpu,
    timer_attiny::{PreviousHook, TinyIo, TinyTimer},
    Object, Simulator,
};
use crate::runtime::{Handle, Runtime, Value};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

impl Simulator {
    fn take_tiny(&self, handle: Handle) -> Box<TinyTimer> {
        let mut objects = self.objects.borrow_mut();
        assert!(
            matches!(&objects[handle.0], Object::TinyTimer(_)),
            "expected ATtinyTimer1, found {}",
            objects[handle.0].name()
        );
        match std::mem::replace(&mut objects[handle.0], Object::Taken) {
            Object::TinyTimer(timer) => timer,
            _ => unreachable!("Tiny timer kind checked before detaching"),
        }
    }
    fn put_tiny(&self, handle: Handle, timer: Box<TinyTimer>) {
        let mut objects = self.objects.borrow_mut();
        assert!(matches!(objects[handle.0], Object::Taken));
        objects[handle.0] = Object::TinyTimer(timer);
    }
    fn with_tiny<R>(&self, handle: Handle, f: impl FnOnce(&mut TinyTimer) -> R) -> R {
        let mut timer = self.take_tiny(handle);
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut timer)));
        self.put_tiny(handle, timer);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
    pub(super) fn construct_tiny(&self, args: &[Value]) -> Value {
        let owner = as_handle(&args[0]);
        let handle = Handle(self.objects.borrow().len());
        let mut timer = TinyTimer::new(owner, super::timer_attiny::parse_config(&args[1]), handle);
        self.with_cpu_mut(owner, |cpu| timer.hooks(cpu, handle));
        self.push(Object::TinyTimer(Box::new(timer)));
        Value::Handle(handle)
    }
    pub(super) fn tiny_get(&self, handle: Handle, key: &str) -> Value {
        let objects = self.objects.borrow();
        let Object::TinyTimer(timer) = &objects[handle.0] else {
            panic!("ATtiny timer detached");
        };
        if key == "count" {
            return timer
                .props
                .get(key)
                .cloned()
                .unwrap_or_else(|| Value::Method(Box::new(Value::Handle(handle)), key.into()));
        }
        let Object::Cpu(cpu) = &objects[timer.cpu.0] else {
            panic!("ATtiny timer CPU detached");
        };
        timer.get(cpu, key)
    }
    pub(super) fn tiny_set(&self, handle: Handle, key: &str, value: Value) {
        assert_eq!(key, "count", "unknown ATtinyTimer1 assignment");
        let mut objects = self.objects.borrow_mut();
        let Object::TinyTimer(timer) = &mut objects[handle.0] else {
            panic!("ATtiny timer detached");
        };
        timer.props.insert(key.into(), value);
    }
    pub(super) fn tiny_call(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        assert_eq!(name, "count", "unknown ATtinyTimer1 method");
        self.with_tiny(handle, |timer| {
            self.with_detached_cpu(timer.cpu, |cpu| {
                timer.count(
                    cpu,
                    &mut TinyBus {
                        sim: self,
                        runtime,
                        handle,
                    },
                    args.first().is_none_or(Value::truthy),
                );
            })
        });
        Value::Undefined
    }
    pub(super) fn tiny_read(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
    ) -> u8 {
        self.with_tiny(handle, |timer| {
            timer.read(
                cpu,
                &mut TinyBus {
                    sim: self,
                    runtime,
                    handle,
                },
                addr,
            )
        })
    }
    pub(super) fn tiny_write(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        cpu: &mut Cpu,
        addr: usize,
        value: i64,
        mask: i64,
    ) {
        self.with_tiny(handle, |timer| {
            timer.write(
                cpu,
                &mut TinyBus {
                    sim: self,
                    runtime,
                    handle,
                },
                addr,
                value as u8,
                mask,
            )
        });
    }
}
struct TinyBus<'a> {
    sim: &'a Simulator,
    runtime: &'a mut Runtime,
    handle: Handle,
}
impl TinyBus<'_> {
    fn with_published<R>(&mut self, timer: &mut TinyTimer, f: impl FnOnce(&mut Self) -> R) -> R {
        let inert = TinyTimer::new(timer.cpu, timer.config.clone(), self.handle);
        let live = std::mem::replace(timer, inert);
        self.sim.put_tiny(self.handle, Box::new(live));
        let result = catch_unwind(AssertUnwindSafe(|| f(self)));
        *timer = *self.sim.take_tiny(self.handle);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }
}
impl TinyIo for TinyBus<'_> {
    fn output(&mut self, timer: &mut TinyTimer, cpu: &mut Cpu, channel: usize, mode: i64) {
        let Some(port) = cpu.gpio_by_port.get(&timer.config.port).copied() else {
            return;
        };
        let pin = timer.config.pins[channel];
        let owner = timer.cpu;
        self.with_published(timer, |this| {
            let callback = this.sim.gpio_get(port, "timerOverridePin");
            this.sim.invoke_from_cpu(
                this.runtime,
                owner,
                cpu,
                callback,
                vec![Value::Number(pin as f64), Value::Number(mode as f64)],
            );
        });
    }
    fn previous(
        &mut self,
        timer: &mut TinyTimer,
        cpu: &mut Cpu,
        hook: PreviousHook,
        addr: usize,
        value: u8,
        mask: i64,
    ) {
        let owner = timer.cpu;
        self.with_published(timer, |this| match hook {
            PreviousHook::Native(hook) => {
                this.sim
                    .native_write(this.runtime, owner, hook, cpu, addr, value as i64, mask)
            }
            PreviousHook::Host(hook) => {
                let old = cpu.data[addr];
                this.sim.invoke_from_cpu(
                    this.runtime,
                    owner,
                    cpu,
                    hook,
                    vec![
                        Value::Number(value as f64),
                        Value::Number(old as f64),
                        Value::Number(addr as f64),
                        Value::Number(mask as f64),
                    ],
                );
            }
        });
    }
}
