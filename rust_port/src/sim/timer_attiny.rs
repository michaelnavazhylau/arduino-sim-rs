// SPDX-License-Identifier: MIT
//! ATtiny25/45/85 Timer1 compatibility model, derived from AVR8js.
use super::cpu::{Cpu, InterruptConfig};
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct TinyConfig {
    pub tccr: usize,
    pub gtccr: usize,
    pub tcnt: usize,
    pub ocr: [usize; 3],
    pub tifr: usize,
    pub timsk: usize,
    pub vectors: [u8; 3],
    pub flags: [u8; 3],
    pub enables: [u8; 3],
    pub port: u16,
    pub pins: [u8; 2],
    pub dividers: [i64; 16],
}
#[derive(Clone)]
pub enum PreviousHook {
    Native(Handle),
    Host(Value),
}
pub trait TinyIo {
    fn output(&mut self, timer: &mut TinyTimer, cpu: &mut Cpu, channel: usize, mode: i64);
    fn previous(
        &mut self,
        timer: &mut TinyTimer,
        cpu: &mut Cpu,
        hook: PreviousHook,
        addr: usize,
        value: u8,
        mask: i64,
    );
}
pub struct TinyTimer {
    pub cpu: Handle,
    pub config: TinyConfig,
    pub callback: Value,
    pub props: BTreeMap<String, Value>,
    pub previous: BTreeMap<usize, PreviousHook>,
    last_cycle: i64,
    tcnt: i64,
    tcnt_next: i64,
    tcnt_updated: bool,
    ocr: [i64; 3],
    divider: i64,
    update_divider: bool,
    counting_up: bool,
    interrupts: [InterruptConfig; 3],
}
impl TinyTimer {
    pub fn new(cpu: Handle, config: TinyConfig, handle: Handle) -> Self {
        let interrupts = std::array::from_fn(|i| InterruptConfig {
            address: config.vectors[i],
            flag_register: config.tifr as u16,
            flag_mask: config.flags[i],
            enable_register: config.timsk as u16,
            enable_mask: config.enables[i],
            constant: false,
            inverse_flag: false,
        });
        Self {
            cpu,
            config,
            callback: Value::Native(format!("tiny-count:{}", handle.0)),
            props: BTreeMap::new(),
            previous: BTreeMap::new(),
            last_cycle: 0,
            tcnt: 0,
            tcnt_next: 0,
            tcnt_updated: false,
            ocr: [0; 3],
            divider: 0,
            update_divider: false,
            counting_up: true,
            interrupts,
        }
    }
    pub fn hooks(&mut self, cpu: &mut Cpu, handle: Handle) {
        let c = &self.config;
        // Preserve prior shared-register hooks, as upstream Timer1 does.
        for addr in [c.gtccr, c.tifr, c.timsk] {
            let host = match &cpu.write_hooks {
                Value::Array(a) => a.borrow().get(addr).cloned(),
                _ => None,
            };
            if let Some(host) = host.filter(Value::truthy) {
                self.previous.insert(addr, PreviousHook::Host(host));
            } else if let Some(owner) = cpu.native_write_hooks.get(&addr) {
                self.previous.insert(addr, PreviousHook::Native(*owner));
            }
        }
        cpu.install_read_hook(c.tcnt, handle);
        for addr in [
            c.tccr, c.gtccr, c.tcnt, c.ocr[0], c.ocr[1], c.ocr[2], c.tifr, c.timsk,
        ] {
            cpu.install_write_hook(addr, handle);
            // Native installation replaces the source hook at this address.
            if let Value::Array(hooks) = &cpu.write_hooks {
                if let Some(hook) = hooks.borrow_mut().get_mut(addr) {
                    *hook = Value::Undefined;
                }
            }
        }
    }
    fn ctc(&self, cpu: &Cpu) -> bool {
        cpu.data[self.config.tccr] & 0x80 != 0
    }
    fn pwm(&self, cpu: &Cpu, channel: usize) -> bool {
        cpu.data[if channel == 0 {
            self.config.tccr
        } else {
            self.config.gtccr
        }] & 0x40
            != 0
    }
    fn com(&self, cpu: &Cpu, channel: usize) -> u8 {
        (cpu.data[if channel == 0 {
            self.config.tccr
        } else {
            self.config.gtccr
        }] >> 4)
            & 3
    }
    fn top(&self, cpu: &Cpu) -> i64 {
        if self.ctc(cpu) || self.pwm(cpu, 0) || self.pwm(cpu, 1) {
            self.ocr[2]
        } else {
            255
        }
    }
    pub fn get(&self, cpu: &Cpu, key: &str) -> Value {
        if let Some(value) = self.props.get(key) {
            return value.clone();
        }
        match key {
            "tccr1" => Value::Number(cpu.data[self.config.tccr] as f64),
            "gtccr" => Value::Number(cpu.data[self.config.gtccr] as f64),
            "CS" => Value::Number((cpu.data[self.config.tccr] & 15) as f64),
            "ctcMode" => Value::Bool(self.ctc(cpu)),
            "pwmA" => Value::Bool(self.pwm(cpu, 0)),
            "pwmB" => Value::Bool(self.pwm(cpu, 1)),
            "comA" => Value::Number(self.com(cpu, 0) as f64),
            "comB" => Value::Number(self.com(cpu, 1) as f64),
            "TOP" => Value::Number(self.top(cpu) as f64),
            _ => panic!("unknown ATtinyTimer1 property: {key}"),
        }
    }
    pub fn read(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo, addr: usize) -> u8 {
        assert_eq!(addr, self.config.tcnt);
        self.count(cpu, io, false);
        cpu.data[addr] = self.tcnt as u8;
        cpu.data[addr]
    }
    pub fn write(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn TinyIo,
        addr: usize,
        mut value: u8,
        mask: i64,
    ) {
        let c = self.config.clone();
        if addr == c.tcnt {
            self.tcnt_next = value as i64;
            self.counting_up = true;
            self.tcnt_updated = true;
            cpu.update_clock_event(self.callback.clone(), 0);
            if self.divider != 0 {
                self.updated(cpu, io, self.tcnt_next, self.tcnt_next);
            }
        } else if let Some(channel) = c.ocr.iter().position(|&reg| reg == addr) {
            self.ocr[channel] = value as i64;
        } else if addr == c.tccr {
            cpu.data[addr] = value;
            self.update_divider = true;
            cpu.clear_clock_event(&self.callback);
            cpu.add_clock_event(self.callback.clone(), 0);
            self.comp_config(cpu, io);
            return;
        } else if addr == c.gtccr {
            for channel in 0..2 {
                if value & (4 << channel) != 0
                    && !self.pwm(cpu, channel)
                    && self.com(cpu, channel) != 0
                {
                    self.non_pwm_output(cpu, io, channel);
                }
            }
            if value & 2 != 0 {
                self.last_cycle = cpu.cycles;
            }
            value &= !14;
            if let Some(hook) = self.previous.get(&addr).cloned() {
                io.previous(self, cpu, hook, addr, value, mask);
            } else {
                cpu.data[addr] = value;
            }
            self.comp_config(cpu, io);
            return;
        } else if addr == c.tifr {
            if let Some(hook) = self.previous.get(&addr).cloned() {
                io.previous(self, cpu, hook, addr, value, mask);
            } else {
                cpu.data[addr] = value;
            }
            for interrupt in &self.interrupts {
                cpu.clear_interrupt_by_flag(interrupt, value);
            }
            return;
        } else if addr == c.timsk {
            if let Some(hook) = self.previous.get(&addr).cloned() {
                io.previous(self, cpu, hook, addr, value, mask);
            }
            for interrupt in &self.interrupts {
                cpu.update_interrupt_enable(interrupt, value);
            }
        } else {
            panic!("unknown ATtinyTimer1 hook {addr:#x}");
        }
        cpu.data[addr] = value;
    }
    pub fn count(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo, reschedule: bool) {
        let divider = self.divider;
        let delta = cpu.cycles - self.last_cycle;
        if divider != 0 && delta >= divider {
            let count_delta = delta / divider;
            self.last_cycle += count_delta * divider;
            let value = self.tcnt;
            let top = self.top(cpu);
            let phase = (self.pwm(cpu, 0) || self.pwm(cpu, 1)) && !self.ctc(cpu);
            let next = if phase {
                self.phase_count(cpu, io, value, count_delta)
            } else {
                (value + count_delta) % (top + 1)
            };
            if !self.tcnt_updated {
                self.tcnt = next;
                if !phase {
                    self.updated(cpu, io, next, value);
                }
            }
            if !phase && value + count_delta > top {
                cpu.set_interrupt_flag(&self.interrupts[2]);
            }
        }
        if self.tcnt_updated {
            self.tcnt = self.tcnt_next;
            self.tcnt_updated = false;
        }
        if self.update_divider {
            let cs = cpu.data[self.config.tccr] & 15;
            let new_divider = self.config.dividers[cs as usize];
            self.last_cycle = if new_divider != 0 { cpu.cycles } else { 0 };
            self.update_divider = false;
            self.divider = new_divider;
            if new_divider != 0 {
                cpu.add_clock_event(
                    self.callback.clone(),
                    self.last_cycle + new_divider - cpu.cycles,
                );
            }
            return;
        }
        if reschedule && divider != 0 {
            cpu.add_clock_event(
                self.callback.clone(),
                self.last_cycle + divider - cpu.cycles,
            );
        }
    }
    fn phase_count(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn TinyIo,
        mut value: i64,
        mut delta: i64,
    ) -> i64 {
        let top = self.top(cpu);
        while delta > 0 {
            if self.counting_up {
                value += 1;
                if value >= top {
                    value = top;
                    self.counting_up = false;
                }
            } else {
                value -= 1;
                if value <= 0 {
                    value = 0;
                    self.counting_up = true;
                    cpu.set_interrupt_flag(&self.interrupts[2]);
                }
            }
            if !self.tcnt_updated {
                for channel in 0..2 {
                    if value == self.ocr[channel] {
                        cpu.set_interrupt_flag(&self.interrupts[channel]);
                        self.pwm_output(cpu, io, channel);
                    }
                }
            }
            delta -= 1;
        }
        value & 255
    }
    fn updated(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo, value: i64, previous: i64) {
        let ocr = self.ocr;
        let overflow = previous > value;
        for (channel, &compare) in ocr.iter().enumerate().take(2) {
            if ((previous < compare || overflow) && value >= compare)
                || (previous < compare && overflow)
            {
                cpu.set_interrupt_flag(&self.interrupts[channel]);
                if self.com(cpu, channel) != 0 && !self.pwm(cpu, channel) {
                    self.non_pwm_output(cpu, io, channel);
                }
            }
        }
    }
    fn non_pwm_output(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo, channel: usize) {
        let mode = match self.com(cpu, channel) {
            1 => 4,
            2 => 3,
            3 => 2,
            _ => return,
        };
        io.output(self, cpu, channel, mode);
    }
    fn pwm_output(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo, channel: usize) {
        let com = self.com(cpu, channel);
        let mode = match com {
            1 => 4,
            2 | 3 => {
                if self.counting_up == (com == 3) {
                    2
                } else {
                    3
                }
            }
            _ => return,
        };
        io.output(self, cpu, channel, mode);
    }
    fn comp_config(&mut self, cpu: &mut Cpu, io: &mut dyn TinyIo) {
        for channel in 0..2 {
            io.output(
                self,
                cpu,
                channel,
                if self.com(cpu, channel) != 0 { 1 } else { 0 },
            );
        }
    }
}
pub fn parse_config(value: &Value) -> TinyConfig {
    let Value::Object(object) = value else {
        panic!("invalid ATtiny timer config");
    };
    let object = object.borrow();
    let get = |key: &str| {
        object
            .get(key)
            .unwrap_or_else(|| panic!("missing ATtiny timer config {key}"))
            .number() as i64
    };
    let Value::Object(dividers) = &object["dividers"] else {
        panic!("invalid ATtiny timer dividers");
    };
    let dividers = dividers.borrow();
    TinyConfig {
        tccr: get("TCCR1") as usize,
        gtccr: get("GTCCR") as usize,
        tcnt: get("TCNT1") as usize,
        ocr: [
            get("OCR1A") as usize,
            get("OCR1B") as usize,
            get("OCR1C") as usize,
        ],
        tifr: get("TIFR") as usize,
        timsk: get("TIMSK") as usize,
        vectors: [
            get("compAInterrupt") as u8,
            get("compBInterrupt") as u8,
            get("ovfInterrupt") as u8,
        ],
        flags: [get("OCF1A") as u8, get("OCF1B") as u8, get("TOV1") as u8],
        enables: [get("OCIE1A") as u8, get("OCIE1B") as u8, get("TOIE1") as u8],
        port: get("compPortB") as u16,
        pins: [get("compPinA") as u8, get("compPinB") as u8],
        dividers: std::array::from_fn(|i| {
            dividers
                .get(&i.to_string())
                .map_or(0, |v| v.number() as i64)
        }),
    }
}
pub fn config() -> Value {
    let mut object: BTreeMap<_, _> = [
        ("TCCR1", 0x50),
        ("GTCCR", 0x4c),
        ("TCNT1", 0x4f),
        ("OCR1A", 0x4e),
        ("OCR1B", 0x4b),
        ("OCR1C", 0x4d),
        ("TIFR", 0x58),
        ("TIMSK", 0x59),
        ("ovfInterrupt", 4),
        ("compAInterrupt", 3),
        ("compBInterrupt", 9),
        ("TOV1", 4),
        ("OCF1A", 64),
        ("OCF1B", 32),
        ("TOIE1", 4),
        ("OCIE1A", 64),
        ("OCIE1B", 32),
        ("compPortB", 0x38),
        ("compPinA", 1),
        ("compPinB", 4),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::Number(v as f64)))
    .collect();
    object.insert(
        "dividers".into(),
        Value::object(
            (0..16)
                .map(|i| {
                    (
                        i.to_string(),
                        Value::Number(if i == 0 { 0.0 } else { (1 << (i - 1)) as f64 }),
                    )
                })
                .collect(),
        ),
    );
    Value::object(object)
}
