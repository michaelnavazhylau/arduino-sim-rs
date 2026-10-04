//! Native Rust AVR simulator used by the converted behavior scenarios.
//!
//! This is the real backend behind [`crate::native_backend`]: no JavaScript, no
//! bridge. Milestones 0–6 in `specs/backend-plan.md` implement the converted
//! compatibility baseline; unsupported objects and methods still fail loudly.
mod adc;
pub mod assembler;
pub mod clock;
pub mod cpu;
mod eeprom;
pub mod gpio;
mod gpio_adapter;
mod peripheral;
mod peripheral_adapter;
mod spi;
pub mod timer;
mod timer_adapter;
pub mod timer_attiny;
mod timer_attiny_adapter;
mod twi;
mod usart;
mod watchdog;

use crate::runtime::{Backend, Handle, Runtime, Value};
use clock::{Clock, DEFAULT_CLKPR};
use cpu::{Bus, Cpu};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::rc::Rc;

/// Native simulator and dynamic object registry.
#[derive(Default)]
pub struct Simulator {
    objects: RefCell<Vec<Object>>,
}

/// Backend-owned native objects. Memory views alias their owning CPU.
enum Object {
    Cpu(Box<Cpu>),
    Clock(Box<Clock>),
    Gpio(Rc<RefCell<gpio::GpioPort>>),
    Timer(Box<timer::Timer>),
    TinyTimer(Box<timer_attiny::TinyTimer>),
    Peripheral(Box<peripheral::Peripheral>),
    EepromMemory {
        memory: Value,
        props: BTreeMap<String, Value>,
    },
    Memory {
        cpu: Handle,
        kind: MemKind,
    },
    /// Temporary placeholder while a CPU is detached for instruction execution.
    Taken,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemKind {
    Data,
    DataView,
    ProgBytes,
    ProgMem,
}

impl Simulator {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, object: Object) -> Handle {
        let mut objects = self.objects.borrow_mut();
        objects.push(object);
        Handle(objects.len() - 1)
    }

    fn with_cpu<R>(&self, handle: Handle, f: impl FnOnce(&Cpu) -> R) -> R {
        let objects = self.objects.borrow();
        match &objects[handle.0] {
            Object::Cpu(cpu) => f(cpu),
            other => panic!("expected CPU at {handle:?}, found {}", other.name()),
        }
    }

    fn with_cpu_mut<R>(&self, handle: Handle, f: impl FnOnce(&mut Cpu) -> R) -> R {
        let mut objects = self.objects.borrow_mut();
        match &mut objects[handle.0] {
            Object::Cpu(cpu) => f(cpu),
            other => panic!("expected CPU at {handle:?}, found {}", other.name()),
        }
    }

    fn memory_view(&self, cpu: Handle, kind: MemKind) -> Value {
        let cached = match kind {
            MemKind::Data => self.with_cpu(cpu, |cpu| cpu.data_handle),
            MemKind::DataView => self.with_cpu(cpu, |cpu| cpu.data_view_handle),
            MemKind::ProgBytes => self.with_cpu(cpu, |cpu| cpu.prog_bytes_handle),
            MemKind::ProgMem => self.with_cpu(cpu, |cpu| cpu.prog_handle),
        };
        if let Some(index) = cached {
            return Value::Handle(Handle(index));
        }
        let handle = self.push(Object::Memory { cpu, kind });
        self.with_cpu_mut(cpu, |cpu| match kind {
            MemKind::Data => cpu.data_handle = Some(handle.0),
            MemKind::DataView => cpu.data_view_handle = Some(handle.0),
            MemKind::ProgBytes => cpu.prog_bytes_handle = Some(handle.0),
            MemKind::ProgMem => cpu.prog_handle = Some(handle.0),
        });
        Value::Handle(handle)
    }

    fn memory_owner(&self, handle: Handle) -> (Handle, MemKind) {
        let objects = self.objects.borrow();
        match &objects[handle.0] {
            Object::Memory { cpu, kind } => (*cpu, *kind),
            other => panic!("expected memory view at {handle:?}, found {}", other.name()),
        }
    }

    /// Reads one element of a memory view. Out-of-range reads are `undefined`.
    fn memory_get(&self, handle: Handle, index: i64) -> Value {
        let (cpu, kind) = self.memory_owner(handle);
        self.with_cpu(cpu, |cpu| match kind {
            MemKind::Data | MemKind::DataView => {
                if index < 0 || index as usize >= cpu.data.len() {
                    Value::Undefined
                } else {
                    Value::Number(cpu.data[index as usize] as f64)
                }
            }
            MemKind::ProgBytes => {
                if index < 0 || index as usize >= cpu.prog_bytes.len() {
                    Value::Undefined
                } else {
                    Value::Number(cpu.prog_bytes[index as usize] as f64)
                }
            }
            MemKind::ProgMem => {
                if index < 0 || index >= cpu.prog_len() as i64 {
                    Value::Undefined
                } else {
                    Value::Number(cpu.prog_word(index) as f64)
                }
            }
        })
    }

    fn memory_write(&self, handle: Handle, index: i64, value: i64) {
        let (cpu, kind) = self.memory_owner(handle);
        self.with_cpu_mut(cpu, |cpu| match kind {
            MemKind::Data | MemKind::DataView => {
                if index >= 0 && (index as usize) < cpu.data.len() {
                    cpu.data[index as usize] = cpu::to_u8(value);
                }
            }
            MemKind::ProgBytes => cpu.set_prog_byte(index, cpu::to_u8(value)),
            MemKind::ProgMem => {
                if index >= 0 && index < cpu.prog_len() as i64 {
                    cpu.prog_bytes[index as usize * 2] = value as u8;
                    cpu.prog_bytes[index as usize * 2 + 1] = (value >> 8) as u8;
                }
            }
        })
    }

    fn memory_length(&self, handle: Handle) -> f64 {
        let (cpu, kind) = self.memory_owner(handle);
        self.with_cpu(cpu, |cpu| match kind {
            MemKind::Data | MemKind::DataView => cpu.data.len() as f64,
            MemKind::ProgBytes => cpu.prog_bytes.len() as f64,
            MemKind::ProgMem => cpu.prog_len() as f64,
        })
    }

    fn read_integer(
        &self,
        handle: Handle,
        addr: i64,
        width: usize,
        signed: bool,
        little: bool,
    ) -> Value {
        let (cpu, kind) = self.memory_owner(handle);
        let bytes: Vec<u8> = self.with_cpu(cpu, |cpu| {
            (0..width)
                .map(|i| match kind {
                    MemKind::Data | MemKind::DataView => cpu
                        .data
                        .get((addr + i as i64) as usize)
                        .copied()
                        .unwrap_or(0),
                    MemKind::ProgBytes => cpu.prog_byte(addr + i as i64),
                    MemKind::ProgMem => cpu.prog_byte(addr * 2 + i as i64),
                })
                .collect()
        });
        let raw = if little {
            bytes
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, b)| acc | ((*b as u32) << (8 * i)))
        } else {
            bytes.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32)
        };
        let value = if signed && width == 2 {
            raw as u16 as i16 as i64
        } else if signed && width == 1 {
            raw as u8 as i8 as i64
        } else {
            raw as i64
        };
        Value::Number(value as f64)
    }

    fn write_integer(&self, handle: Handle, addr: i64, width: usize, value: i64, little: bool) {
        let bytes: Vec<u8> = (0..width)
            .map(|i| {
                let shift = if little { 8 * i } else { 8 * (width - 1 - i) };
                ((value >> shift) & 0xff) as u8
            })
            .collect();
        let (cpu, kind) = self.memory_owner(handle);
        self.with_cpu_mut(cpu, |cpu| {
            for (i, byte) in bytes.iter().enumerate() {
                let index = addr + i as i64;
                match kind {
                    MemKind::Data | MemKind::DataView => {
                        if index >= 0 && (index as usize) < cpu.data.len() {
                            cpu.data[index as usize] = *byte;
                        }
                    }
                    MemKind::ProgBytes => cpu.set_prog_byte(index, *byte),
                    MemKind::ProgMem => cpu.set_prog_byte(addr * 2 + i as i64, *byte),
                }
            }
        });
    }

    fn clock_get(&self, handle: Handle, key: &str) -> Value {
        let (cpu_handle, frequency, prescaler, delta) = {
            let objects = self.objects.borrow();
            match &objects[handle.0] {
                Object::Clock(clock) => (
                    clock.cpu,
                    clock.frequency(),
                    clock.prescaler(),
                    clock.cycles_delta,
                ),
                other => panic!("expected AVRClock at {handle:?}, found {}", other.name()),
            }
        };
        let cycles = self.with_cpu(cpu_handle, |cpu| cpu.cycles as f64);
        match key {
            "frequency" => Value::Number(frequency),
            "prescaler" => Value::Number(prescaler),
            "cyclesDelta" => Value::Number(delta),
            "timeNanos" => Value::Number(((cycles + delta) / frequency) * 1e9),
            "timeMicros" => Value::Number(((cycles + delta) / frequency) * 1e6),
            "timeMillis" => Value::Number(((cycles + delta) / frequency) * 1e3),
            other => panic!("unimplemented AVRClock property: {other}"),
        }
    }

    fn cpu_get(&self, handle: Handle, key: &str) -> Value {
        if let Some(value) = self.with_cpu(handle, |cpu| cpu.props.get(key).cloned()) {
            return value;
        }
        match key {
            "SP" => Value::Number(self.with_cpu(handle, |cpu| cpu.sp()) as f64),
            "SREG" => Value::Number(self.with_cpu(handle, |cpu| cpu.sreg()) as f64),
            "interruptsEnabled" => {
                Value::Bool(self.with_cpu(handle, |cpu| cpu.interrupts_enabled()))
            }
            "pc" => Value::Number(self.with_cpu(handle, |cpu| cpu.pc as f64)),
            "cycles" => Value::Number(self.with_cpu(handle, |cpu| cpu.cycles as f64)),
            "pc22Bits" => Value::Bool(self.with_cpu(handle, |cpu| cpu.pc22_bits)),
            "nextInterrupt" => {
                Value::Number(self.with_cpu(handle, |cpu| cpu.next_interrupt as f64))
            }
            "maxInterrupt" => Value::Number(self.with_cpu(handle, |cpu| cpu.max_interrupt as f64)),
            "data" => self.memory_view(handle, MemKind::Data),
            "dataView" => self.memory_view(handle, MemKind::DataView),
            "progMem" => self.memory_view(handle, MemKind::ProgMem),
            "progBytes" => self.memory_view(handle, MemKind::ProgBytes),
            "readHooks" => self.with_cpu(handle, |cpu| cpu.read_hooks.clone()),
            "writeHooks" => self.with_cpu(handle, |cpu| cpu.write_hooks.clone()),
            _ => Value::Method(Box::new(Value::Handle(handle)), key.into()),
        }
    }

    fn cpu_set(&self, handle: Handle, key: &str, value: Value) {
        match key {
            "cycles" => self.with_cpu_mut(handle, |cpu| cpu.cycles = value.number() as i64),
            "pc" => self.with_cpu_mut(handle, |cpu| cpu.pc = value.number() as i64),
            "SP" => {
                let sp = value.number() as u16;
                self.with_cpu_mut(handle, |cpu| cpu.set_sp(sp));
            }
            _ => self.with_cpu_mut(handle, |cpu| {
                cpu.props.insert(key.into(), value);
            }),
        }
    }

    fn cpu_call(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        match name {
            "reset" => {
                self.with_cpu_mut(handle, |cpu| cpu.reset());
                Value::Undefined
            }
            "readData" => {
                let addr = args[0].number() as usize;
                Value::Number(self.read_data(runtime, handle, addr) as f64)
            }
            "writeData" => {
                let addr = args[0].number() as usize;
                let value = args[1].number() as i64;
                let mask = args.get(2).map_or(0xff, |v| v.number() as i64);
                self.write_data(runtime, handle, addr, value, mask);
                Value::Undefined
            }
            "addClockEvent" => {
                let callback = args[0].clone();
                let cycles = args[1].number() as i64;
                self.with_cpu_mut(handle, |cpu| cpu.add_clock_event(callback, cycles))
            }
            "updateClockEvent" => {
                let callback = args[0].clone();
                let cycles = args[1].number() as i64;
                Value::Bool(
                    self.with_cpu_mut(handle, |cpu| cpu.update_clock_event(callback, cycles)),
                )
            }
            "clearClockEvent" => {
                let callback = args[0].clone();
                Value::Bool(self.with_cpu_mut(handle, |cpu| cpu.clear_clock_event(&callback)))
            }
            "tick" => {
                self.tick(runtime, handle);
                Value::Undefined
            }
            other => panic!("unimplemented CPU method: {other}"),
        }
    }

    fn tick(&self, runtime: &mut Runtime, handle: Handle) {
        let due = self.with_cpu(handle, |cpu| {
            cpu.next_clock_event.map(|head| {
                (
                    head,
                    cpu.clock_events[head].cycles,
                    cpu.clock_events[head].callback.clone(),
                )
            })
        });
        if let Some((head, cycles, callback)) = due {
            if cycles <= self.with_cpu(handle, |cpu| cpu.cycles) {
                // The callback runs while the queue still contains its own entry,
                // matching upstream's ordering-sensitive linked-list behavior.
                runtime.invoke(callback, Vec::new());
                self.with_cpu_mut(handle, |cpu| {
                    let next = cpu.clock_events[head].next;
                    cpu.next_clock_event = next;
                    if cpu.free_events.len() < 10 {
                        cpu.free_events.push(head);
                    }
                });
            }
        }
        let (enabled, next) =
            self.with_cpu(handle, |cpu| (cpu.interrupts_enabled(), cpu.next_interrupt));
        if enabled && next >= 0 {
            let interrupt =
                self.with_cpu(handle, |cpu| cpu.pending_interrupts[next as usize].clone());
            if let Some(interrupt) = interrupt {
                self.with_cpu_mut(handle, |cpu| {
                    cpu::avr_interrupt(cpu, interrupt.address as i64)
                });
                if !interrupt.constant {
                    self.with_cpu_mut(handle, |cpu| cpu.clear_interrupt(&interrupt, true));
                }
            }
        }
    }

    fn avr_instruction(&self, runtime: &mut Runtime, handle: Handle) {
        self.with_detached_cpu(handle, |cpu| {
            let opcode = cpu.prog_word(cpu.pc) as i64;
            if opcode == 0x95a8 {
                let callback = cpu
                    .props
                    .get("onWatchdogReset")
                    .cloned()
                    .unwrap_or(Value::Native("noop".into()));
                self.invoke_from_cpu(runtime, handle, cpu, callback, Vec::new());
            }
            let mut bus = NativeBus {
                sim: self,
                runtime,
                owner: handle,
            };
            cpu::execute(cpu, &mut bus, opcode);
        });
    }
}

impl Object {
    fn name(&self) -> &'static str {
        match self {
            Object::Cpu(_) => "CPU",
            Object::Clock(_) => "AVRClock",
            Object::Gpio(_) => "AVRIOPort",
            Object::Timer(_) => "AVRTimer",
            Object::TinyTimer(_) => "ATtinyTimer1",
            Object::Peripheral(p) => p.kind,
            Object::EepromMemory { .. } => "EEPROMMemoryBackend",
            Object::Memory { .. } => "memory view",
            Object::Taken => "detached object",
        }
    }
}

/// Detaches a CPU so native hooks can drive `execute` while the object registry
/// stays available for peripheral dispatch. At synchronous callback boundaries
/// the actual CPU is temporarily restored to the registry for safe reentry.
impl Simulator {
    fn take_cpu(&self, handle: Handle) -> Box<Cpu> {
        let mut objects = self.objects.borrow_mut();
        assert!(
            matches!(&objects[handle.0], Object::Cpu(_)),
            "expected CPU at {handle:?}, found {}",
            objects[handle.0].name()
        );
        match std::mem::replace(&mut objects[handle.0], Object::Taken) {
            Object::Cpu(cpu) => cpu,
            _ => unreachable!("CPU kind checked before detaching"),
        }
    }

    fn put_cpu(&self, handle: Handle, cpu: Box<Cpu>) {
        let mut objects = self.objects.borrow_mut();
        assert!(matches!(&objects[handle.0], Object::Taken));
        objects[handle.0] = Object::Cpu(cpu);
    }

    fn native_read(&self, runtime: &mut Runtime, owner: Handle, cpu: &mut Cpu, addr: usize) -> u8 {
        if matches!(&self.objects.borrow()[owner.0], Object::Timer(_)) {
            return self.timer_read(runtime, owner, cpu, addr);
        }
        if matches!(&self.objects.borrow()[owner.0], Object::TinyTimer(_)) {
            return self.tiny_read(runtime, owner, cpu, addr);
        }
        if matches!(&self.objects.borrow()[owner.0], Object::Peripheral(_)) {
            return self.peripheral_read(owner, cpu, addr);
        }
        panic!("no native read hook at {addr:#x}");
    }

    #[allow(clippy::too_many_arguments)]
    fn native_write(
        &self,
        runtime: &mut Runtime,
        owner: Handle,
        hook: Handle,
        cpu: &mut Cpu,
        addr: usize,
        value: i64,
        mask: i64,
    ) {
        if matches!(&self.objects.borrow()[hook.0], Object::Timer(_)) {
            self.timer_write(runtime, hook, cpu, addr, value);
            return;
        }
        if matches!(&self.objects.borrow()[hook.0], Object::TinyTimer(_)) {
            self.tiny_write(runtime, hook, cpu, addr, value, mask);
            return;
        }
        if matches!(&self.objects.borrow()[hook.0], Object::Peripheral(_)) {
            self.peripheral_write(runtime, hook, cpu, addr, value, mask);
            return;
        }
        let is_gpio = matches!(&self.objects.borrow()[hook.0], Object::Gpio(_));
        if is_gpio {
            self.gpio_write(runtime, owner, hook, cpu, addr, value, mask);
            return;
        }
        let mut objects = self.objects.borrow_mut();
        match &mut objects[hook.0] {
            Object::Clock(clock) => clock.write_hook(cpu, value),
            other => panic!("no write hook on {}", other.name()),
        }
    }

    fn with_detached_cpu<R>(&self, handle: Handle, f: impl FnOnce(&mut Cpu) -> R) -> R {
        let mut cpu = self.take_cpu(handle);
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut cpu)));
        self.put_cpu(handle, cpu);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }

    fn invoke_from_cpu(
        &self,
        runtime: &mut Runtime,
        owner: Handle,
        cpu: &mut Cpu,
        callback: Value,
        args: Vec<Value>,
    ) -> Value {
        // Move, never clone, state into the registry. The local CPU is inert
        // until synchronous reentry returns; no borrowed guards cross callbacks.
        let live = std::mem::replace(cpu, Cpu::new(Vec::new(), 0));
        self.put_cpu(owner, Box::new(live));
        let result = catch_unwind(AssertUnwindSafe(|| runtime.invoke(callback, args)));
        *cpu = *self.take_cpu(owner);
        match result {
            Ok(value) => value,
            Err(error) => resume_unwind(error),
        }
    }

    fn invoke_pending(
        &self,
        runtime: &mut Runtime,
        owner: Handle,
        cpu: &mut Cpu,
        pending: Vec<gpio::PendingCall>,
    ) {
        for (callback, args) in pending {
            self.invoke_from_cpu(runtime, owner, cpu, callback, args);
        }
    }

    fn write_data(
        &self,
        runtime: &mut Runtime,
        handle: Handle,
        addr: usize,
        value: i64,
        mask: i64,
    ) {
        self.with_detached_cpu(handle, |cpu| {
            let mut bus = NativeBus {
                sim: self,
                runtime,
                owner: handle,
            };
            bus.write_data(cpu, addr, value, mask);
        });
    }

    fn read_data(&self, runtime: &mut Runtime, handle: Handle, addr: usize) -> u8 {
        self.with_detached_cpu(handle, |cpu| {
            let mut bus = NativeBus {
                sim: self,
                runtime,
                owner: handle,
            };
            bus.read_data(cpu, addr)
        })
    }
}

/// Adapter between instruction execution and native peripheral hooks.
struct NativeBus<'a> {
    sim: &'a Simulator,
    runtime: &'a mut Runtime,
    owner: Handle,
}

impl Bus for NativeBus<'_> {
    fn read_data(&mut self, cpu: &mut Cpu, addr: usize) -> u8 {
        let hook = match &cpu.read_hooks {
            Value::Array(hooks) => hooks.borrow().get(addr).cloned(),
            _ => panic!("CPU readHooks must be an array"),
        };
        if let Some(hook) = hook.filter(|hook| addr >= 32 && hook.truthy()) {
            return self
                .sim
                .invoke_from_cpu(
                    self.runtime,
                    self.owner,
                    cpu,
                    hook,
                    vec![Value::Number(addr as f64)],
                )
                .number() as u8;
        }
        if let Some(owner) = cpu.native_read_hooks.get(&addr).copied() {
            return self.sim.native_read(self.runtime, owner, cpu, addr);
        }
        cpu.read_data(addr)
    }

    fn write_data(&mut self, cpu: &mut Cpu, addr: usize, value: i64, mask: i64) {
        let hook = match &cpu.write_hooks {
            Value::Array(hooks) => hooks.borrow().get(addr).cloned(),
            _ => panic!("CPU writeHooks must be an array"),
        };
        if let Some(hook) = hook.filter(Value::truthy) {
            let old = cpu.read_data(addr);
            let consumed = self
                .sim
                .invoke_from_cpu(
                    self.runtime,
                    self.owner,
                    cpu,
                    hook,
                    vec![
                        Value::Number(value as f64),
                        Value::Number(old as f64),
                        Value::Number(addr as f64),
                        Value::Number(mask as f64),
                    ],
                )
                .truthy();
            if !consumed {
                cpu.write_data(addr, value, mask);
            }
            return;
        }
        if let Some(owner) = cpu.native_write_hooks.get(&addr).copied() {
            self.sim
                .native_write(self.runtime, self.owner, owner, cpu, addr, value, mask);
            return;
        }
        cpu.write_data(addr, value, mask);
    }
}

fn enum_value(names: &[&str]) -> Value {
    Value::object(
        names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.to_string(), Value::Number(i as f64)))
            .collect(),
    )
}

fn as_handle(value: &Value) -> Handle {
    match value {
        Value::Handle(handle) => *handle,
        other => panic!("expected native handle, got {other:?}"),
    }
}

fn indexed_key(key: &str) -> Option<i64> {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse().ok()
}

impl Backend for Simulator {
    fn resolve(&self, name: &str) -> Value {
        match name {
            "assemble" | "avrInstruction" | "avrInterrupt" | "noop" => Value::Native(name.into()),
            "clockConfig" => Value::object(BTreeMap::from([(
                "CLKPR".into(),
                Value::Number(DEFAULT_CLKPR as f64),
            )])),
            "adcConfig" => adc::default_config(),
            "atmega328Channels" => adc::channels(),
            "ADCReference" => {
                enum_value(&["AVCC", "AREF", "Internal1V1", "Internal2V56", "Reserved"])
            }
            "ADCMuxInputType" => {
                enum_value(&["SingleEnded", "Differential", "Constant", "Temperature"])
            }
            "eepromConfig" => eeprom::default_config(),
            "spiConfig" => spi::default_config(),
            "usart0Config" => usart::default_config(),
            "twiConfig" => twi::default_config(),
            "watchdogConfig" => watchdog::default_config(),
            "attinyTimer1Config" => timer_attiny::config(),
            "timer0Config" => timer::config(0),
            "timer1Config" => timer::config(1),
            "timer2Config" => timer::config(2),
            "PinState" => enum_value(&["Low", "High", "Input", "InputPullUp"]),
            "PinOverrideMode" => enum_value(&["None", "Enable", "Set", "Clear", "Toggle"]),
            name if name.starts_with("port") && name.ends_with("Config") => gpio::port_configs()
                .remove(name)
                .unwrap_or_else(|| panic!("unknown port config: {name}")),
            _ => panic!("unimplemented native global: {name}"),
        }
    }

    fn construct(&self, runtime: &mut Runtime, kind: &str, args: Vec<Value>) -> Value {
        match kind {
            "CPU" => {
                let prog_bytes = match &args[0] {
                    Value::Buffer { bytes, width } => {
                        assert_eq!(*width, 2, "CPU program memory must be a Uint16Array");
                        bytes.borrow().clone()
                    }
                    other => panic!("CPU expects a Uint16Array program, got {other:?}"),
                };
                let sram = args.get(1).map_or(8192, |v| v.number() as usize);
                let handle = self.push(Object::Cpu(Box::new(Cpu::new(prog_bytes, sram))));
                self.with_cpu_mut(handle, |cpu| {
                    cpu.props
                        .insert("onWatchdogReset".into(), Value::Native("noop".into()));
                });
                Value::Handle(handle)
            }
            "AVRClock" => {
                let cpu = as_handle(&args[0]);
                let base_freq = args[1].number();
                let clkpr = match args.get(2) {
                    Some(Value::Object(fields)) => fields
                        .borrow()
                        .get("CLKPR")
                        .map_or(DEFAULT_CLKPR, |v| v.number() as usize),
                    _ => DEFAULT_CLKPR,
                };
                let handle = self.push(Object::Clock(Box::new(Clock::new(cpu, base_freq, clkpr))));
                self.with_cpu_mut(cpu, |state| {
                    state.install_write_hook(clkpr, handle);
                });
                Value::Handle(handle)
            }
            "AVRIOPort" => self.construct_gpio(&args),
            "AVRTimer" => self.construct_timer(runtime, &args),
            "ATtinyTimer1" => self.construct_tiny(&args),
            "AVREEPROM"
            | "EEPROMMemoryBackend"
            | "AVRADC"
            | "AVRSPI"
            | "AVRUSART"
            | "AVRTWI"
            | "NoopTWIEventHandler"
            | "AVRWatchdog" => self.construct_peripheral(kind, &args),
            other => panic!("unimplemented native constructor: {other}"),
        }
    }

    fn get(&self, object: Handle, key: &str) -> Value {
        let objects = self.objects.borrow();
        let kind = match &objects[object.0] {
            Object::Cpu(_) => "cpu",
            Object::Clock(_) => "clock",
            Object::Gpio(_) => "gpio",
            Object::Timer(_) => "timer",
            Object::TinyTimer(_) => "tiny",
            Object::Peripheral(_) | Object::EepromMemory { .. } => "peripheral",
            Object::Memory { .. } => "memory",
            Object::Taken => panic!("native object {object:?} is detached"),
        };
        drop(objects);
        match kind {
            "cpu" => self.cpu_get(object, key),
            "clock" => self.clock_get(object, key),
            "gpio" => self.gpio_get(object, key),
            "timer" => self.timer_get(object, key),
            "tiny" => self.tiny_get(object, key),
            "peripheral" => self.peripheral_get(object, key),
            _ => {
                if key == "length" {
                    return Value::Number(self.memory_length(object));
                }
                if let Some(index) = indexed_key(key) {
                    return self.memory_get(object, index);
                }
                Value::Method(Box::new(Value::Handle(object)), key.into())
            }
        }
    }

    fn set(&self, _runtime: &mut Runtime, object: Handle, key: &str, value: Value) {
        let objects = self.objects.borrow();
        let kind = match &objects[object.0] {
            Object::Cpu(_) => "cpu",
            Object::Clock(_) => "clock",
            Object::Gpio(_) => "gpio",
            Object::Timer(_) => "timer",
            Object::TinyTimer(_) => "tiny",
            Object::Peripheral(_) | Object::EepromMemory { .. } => "peripheral",
            Object::Memory { .. } => "memory",
            Object::Taken => panic!("native object {object:?} is detached"),
        };
        drop(objects);
        match kind {
            "cpu" => self.cpu_set(object, key, value),
            "clock" => panic!("AVRClock properties are read-only: {key}"),
            "gpio" => self.gpio_set(object, key, value),
            "timer" => self.timer_set(object, key, value),
            "tiny" => self.tiny_set(object, key, value),
            "peripheral" => self.peripheral_set(object, key, value),
            _ => match indexed_key(key) {
                Some(index) => self.memory_write(object, index, value.number() as i64),
                None => panic!("cannot set native memory key {key}"),
            },
        }
    }

    fn call(
        &self,
        runtime: &mut Runtime,
        receiver: Option<Handle>,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        match (receiver, name) {
            (None, "assemble") => assembler::to_value(assembler::assemble(&args[0].string())),
            (None, "avrInstruction") => {
                let handle = as_handle(&args[0]);
                self.avr_instruction(runtime, handle);
                Value::Undefined
            }
            (None, "avrInterrupt") => {
                let handle = as_handle(&args[0]);
                let addr = args[1].number() as i64;
                self.with_cpu_mut(handle, |cpu| cpu::avr_interrupt(cpu, addr));
                Value::Undefined
            }
            (None, "noop") => Value::Undefined,
            (None, name) if name.starts_with("peripheral:") => {
                let parts: Vec<_> = name.split(':').collect();
                assert!(
                    parts.len() == 3 || parts.len() == 5,
                    "invalid peripheral callback identity"
                );
                let handle = Handle(parts[1].parse().expect("invalid peripheral handle"));
                let args = if parts.len() == 5 {
                    if parts[4].is_empty() {
                        Vec::new()
                    } else {
                        parts[4]
                            .split(',')
                            .map(|n| Value::Number(n.parse().expect("invalid event argument")))
                            .collect()
                    }
                } else {
                    args
                };
                self.peripheral_call(runtime, handle, parts[2], args)
            }
            (None, name) if name.starts_with("tiny-count:") => {
                let id = name.strip_prefix("tiny-count:").unwrap();
                self.tiny_call(
                    runtime,
                    Handle(id.parse().expect("invalid tiny callback identity")),
                    "count",
                    args,
                )
            }
            (None, name)
                if name.starts_with("timer-count:") || name.starts_with("timer-external:") =>
            {
                let (kind, id) = name.split_once(':').unwrap();
                let handle = Handle(id.parse().expect("invalid timer callback identity"));
                self.timer_call(
                    runtime,
                    handle,
                    if kind == "timer-count" {
                        "count"
                    } else {
                        "external"
                    },
                    args,
                )
            }
            (Some(handle), _) => {
                let objects = self.objects.borrow();
                let kind = match &objects[handle.0] {
                    Object::Cpu(_) => "cpu",
                    Object::Clock(_) => "clock",
                    Object::Gpio(_) => "gpio",
                    Object::Timer(_) => "timer",
                    Object::TinyTimer(_) => "tiny",
                    Object::Peripheral(_) | Object::EepromMemory { .. } => "peripheral",
                    Object::Memory { .. } => "memory",
                    Object::Taken => panic!("native object {handle:?} is detached"),
                };
                drop(objects);
                match kind {
                    "cpu" => self.cpu_call(runtime, handle, name, args),
                    "clock" => panic!("unimplemented AVRClock method: {name}"),
                    "gpio" => self.gpio_call(runtime, handle, name, args),
                    "timer" => self.timer_call(runtime, handle, name, args),
                    "tiny" => self.tiny_call(runtime, handle, name, args),
                    "peripheral" => self.peripheral_call(runtime, handle, name, args),
                    _ => self.memory_call(handle, name, args),
                }
            }
            (None, other) => panic!("unimplemented native function: {other}"),
        }
    }

    fn properties(&self, object: Handle) -> BTreeMap<String, Value> {
        panic!("unimplemented native properties: {object:?}")
    }
}

impl Simulator {
    fn memory_call(&self, handle: Handle, name: &str, args: Vec<Value>) -> Value {
        match name {
            "getUint8" => self.read_integer(handle, args[0].number() as i64, 1, false, true),
            "getInt8" => self.read_integer(handle, args[0].number() as i64, 1, true, true),
            "getUint16" => {
                let little = args.get(1).is_none_or(Value::truthy);
                self.read_integer(handle, args[0].number() as i64, 2, false, little)
            }
            "getInt16" => {
                let little = args.get(1).is_none_or(Value::truthy);
                self.read_integer(handle, args[0].number() as i64, 2, true, little)
            }
            "setUint8" => {
                self.write_integer(
                    handle,
                    args[0].number() as i64,
                    1,
                    args[1].number() as i64,
                    true,
                );
                Value::Undefined
            }
            "setInt8" => {
                self.write_integer(
                    handle,
                    args[0].number() as i64,
                    1,
                    args[1].number() as i64,
                    true,
                );
                Value::Undefined
            }
            "setUint16" => {
                let little = args.get(2).is_none_or(Value::truthy);
                self.write_integer(
                    handle,
                    args[0].number() as i64,
                    2,
                    args[1].number() as i64,
                    little,
                );
                Value::Undefined
            }
            "setInt16" => {
                let little = args.get(2).is_none_or(Value::truthy);
                self.write_integer(
                    handle,
                    args[0].number() as i64,
                    2,
                    args[1].number() as i64,
                    little,
                );
                Value::Undefined
            }
            "set" => {
                let offset = args.get(1).map_or(0, |v| v.number() as i64);
                for (i, value) in args[0].values().into_iter().enumerate() {
                    self.memory_write(handle, offset + i as i64, value.number() as i64);
                }
                Value::Undefined
            }
            other => panic!("unimplemented memory method: {other}"),
        }
    }
}
