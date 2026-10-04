// SPDX-License-Identifier: MIT
//! AVR8js-compatible megaAVR timers: WGM tables, buffered OCR, events and GPIO.
//! Derived from avr8js/src/peripherals/timer.ts (Uri Shaked and contributors).
use super::cpu::{Cpu, InterruptConfig};
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct TimerConfig {
    pub bits: u8,
    pub dividers: [i64; 8],
    pub tifr: usize,
    pub timsk: usize,
    pub tcnt: usize,
    pub tccra: usize,
    pub tccrb: usize,
    pub tccrc: usize,
    pub icr: usize,
    pub ocr: [usize; 3],
    pub vectors: [i64; 4],
    pub flags: [u8; 4],
    pub enables: [u8; 4],
    pub ports: [u16; 3],
    pub pins: [u8; 3],
    pub external_port: u16,
    pub external_pin: u8,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Phase,
    Ctc,
    Fast,
    PhaseFrequency,
    Reserved,
}
#[derive(Clone, Copy, PartialEq)]
enum Update {
    Immediate,
    Top,
    Bottom,
}
#[derive(Clone, Copy)]
enum Top {
    Fixed(i64),
    Ocra,
    Icr,
}
#[derive(Clone, Copy, PartialEq)]
enum Overflow {
    Max,
    Top,
    Bottom,
}
use Mode::*;
use Top::*;
const WGM8: [(Mode, Top, Update, Overflow, bool); 8] = [
    (Normal, Fixed(255), Update::Immediate, Overflow::Max, false),
    (Phase, Fixed(255), Update::Top, Overflow::Bottom, false),
    (Ctc, Ocra, Update::Immediate, Overflow::Max, false),
    (Fast, Fixed(255), Update::Bottom, Overflow::Max, false),
    (
        Reserved,
        Fixed(255),
        Update::Immediate,
        Overflow::Max,
        false,
    ),
    (Phase, Ocra, Update::Top, Overflow::Bottom, true),
    (
        Reserved,
        Fixed(255),
        Update::Immediate,
        Overflow::Max,
        false,
    ),
    (Fast, Ocra, Update::Bottom, Overflow::Top, true),
];
const WGM16: [(Mode, Top, Update, Overflow, bool); 16] = [
    (
        Normal,
        Fixed(65535),
        Update::Immediate,
        Overflow::Max,
        false,
    ),
    (Phase, Fixed(255), Update::Top, Overflow::Bottom, false),
    (Phase, Fixed(511), Update::Top, Overflow::Bottom, false),
    (Phase, Fixed(1023), Update::Top, Overflow::Bottom, false),
    (Ctc, Ocra, Update::Immediate, Overflow::Max, false),
    (Fast, Fixed(255), Update::Bottom, Overflow::Top, false),
    (Fast, Fixed(511), Update::Bottom, Overflow::Top, false),
    (Fast, Fixed(1023), Update::Bottom, Overflow::Top, false),
    (PhaseFrequency, Icr, Update::Bottom, Overflow::Bottom, false),
    (PhaseFrequency, Ocra, Update::Bottom, Overflow::Bottom, true),
    (Phase, Icr, Update::Top, Overflow::Bottom, false),
    (Phase, Ocra, Update::Top, Overflow::Bottom, true),
    (Ctc, Icr, Update::Immediate, Overflow::Max, false),
    (
        Reserved,
        Fixed(65535),
        Update::Immediate,
        Overflow::Max,
        false,
    ),
    (Fast, Icr, Update::Bottom, Overflow::Top, true),
    (Fast, Ocra, Update::Bottom, Overflow::Top, true),
];

/// IO calls occur synchronously, with timer and CPU published for host reentry.
pub trait TimerIo {
    fn output(&mut self, timer: &mut Timer, cpu: &mut Cpu, channel: usize, mode: i64);
    fn external_listener(&mut self, port: Handle, pin: u8, callback: Option<Value>);
}

pub struct Timer {
    pub cpu: Handle,
    pub config: TimerConfig,
    pub props: BTreeMap<String, Value>,
    pub callback: Value,
    pub external_callback: Value,
    max: i64,
    last_cycle: i64,
    ocr: [i64; 3],
    next_ocr: [i64; 3],
    ocr_update: Update,
    tov_update: Overflow,
    icr: i64,
    mode: Mode,
    top_value: Top,
    tcnt: i64,
    tcnt_next: i64,
    comp: [u8; 3],
    tcnt_updated: bool,
    update_divider: bool,
    counting_up: bool,
    divider: i64,
    external_port: Option<Handle>,
    pub external_rising: bool,
    high_byte: u8,
    interrupts: [InterruptConfig; 4],
}

impl Timer {
    pub fn new(cpu: Handle, config: TimerConfig, handle: Handle) -> Self {
        let interrupts = std::array::from_fn(|i| InterruptConfig {
            address: config.vectors[i] as u8,
            flag_register: config.tifr as u16,
            flag_mask: config.flags[i],
            enable_register: config.timsk as u16,
            enable_mask: config.enables[i],
            constant: false,
            inverse_flag: false,
        });
        Self {
            cpu,
            max: if config.bits == 16 { 65535 } else { 255 },
            config,
            props: BTreeMap::new(),
            callback: Value::Native(format!("timer-count:{}", handle.0)),
            external_callback: Value::Native(format!("timer-external:{}", handle.0)),
            last_cycle: 0,
            ocr: [0; 3],
            next_ocr: [0; 3],
            ocr_update: Update::Immediate,
            tov_update: Overflow::Max,
            icr: 0,
            mode: Normal,
            top_value: Fixed(255),
            tcnt: 0,
            tcnt_next: 0,
            comp: [0; 3],
            tcnt_updated: false,
            update_divider: false,
            counting_up: true,
            divider: 0,
            external_port: None,
            external_rising: false,
            high_byte: 0,
            interrupts,
        }
    }
    pub fn hooks(&self, cpu: &mut Cpu, handle: Handle) {
        let c = &self.config;
        cpu.install_read_hook(c.tcnt, handle);
        let mut addresses = vec![
            c.tcnt, c.ocr[0], c.ocr[1], c.tccra, c.tccrb, c.tifr, c.timsk,
        ];
        if c.ocr[2] != 0 {
            addresses.push(c.ocr[2]);
        }
        if c.tccrc != 0 {
            addresses.push(c.tccrc);
        }
        if c.bits == 16 {
            addresses.extend([c.icr, c.icr + 1, c.tcnt + 1, c.ocr[0] + 1, c.ocr[1] + 1]);
            if c.ocr[2] != 0 {
                addresses.push(c.ocr[2] + 1);
            }
        }
        for address in addresses {
            cpu.install_write_hook(address, handle);
        }
    }
    pub fn top(&self) -> i64 {
        match self.top_value {
            Fixed(value) => value,
            Ocra => self.ocr[0],
            Icr => self.icr,
        }
    }
    fn ocr_mask(&self) -> u16 {
        match self.top_value {
            Fixed(value) => value as u16,
            _ => 65535,
        }
    }
    fn wgm(&self, cpu: &Cpu) -> usize {
        let mask = if self.config.bits == 16 { 0x18 } else { 8 };
        (((cpu.data[self.config.tccrb] & mask) >> 1) | (cpu.data[self.config.tccra] & 3)) as usize
    }
    fn phase(&self) -> bool {
        matches!(self.mode, Phase | PhaseFrequency)
    }
    fn pwm(&self) -> bool {
        self.phase() || self.mode == Fast
    }
    pub fn update_wgm(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo) {
        let wgm = self.wgm(cpu);
        let (mode, top, ocr_update, tov_update, toggle) = if self.config.bits == 16 {
            WGM16[wgm]
        } else {
            WGM8[wgm]
        };
        self.mode = mode;
        self.top_value = top;
        self.ocr_update = ocr_update;
        self.tov_update = tov_update;
        let tccra = cpu.data[self.config.tccra];
        for channel in 0..if self.config.ocr[2] != 0 { 3 } else { 2 } {
            let previous = self.comp[channel];
            let mut comp = (tccra >> (6 - channel * 2)) & 3;
            if comp == 1 && self.pwm() && (channel != 0 || !toggle) {
                comp = 0;
            }
            self.comp[channel] = comp;
            if (previous != 0) != (comp != 0) {
                io.output(self, cpu, channel, if comp != 0 { 1 } else { 0 });
            }
        }
    }
    pub fn reset(&mut self) {
        self.divider = 0;
        self.last_cycle = 0;
        self.ocr = [0; 3];
        self.next_ocr = [0; 3];
        self.icr = 0;
        self.tcnt = 0;
        self.tcnt_next = 0;
        self.tcnt_updated = false;
        self.counting_up = false;
        self.update_divider = true;
    }
    pub fn get(&self, cpu: &Cpu, key: &str) -> Value {
        if let Some(value) = self.props.get(key) {
            return value.clone();
        }
        let value = match key {
            "debugTCNT" => self.tcnt,
            "TCCRA" => cpu.data[self.config.tccra] as i64,
            "TCCRB" => cpu.data[self.config.tccrb] as i64,
            "TIMSK" => cpu.data[self.config.timsk] as i64,
            "CS" => (cpu.data[self.config.tccrb] & 7) as i64,
            "WGM" => self.wgm(cpu) as i64,
            "TOP" => self.top(),
            "ocrMask" => self.ocr_mask() as i64,
            _ => panic!("unknown AVRTimer property: {key}"),
        };
        Value::Number(value as f64)
    }
    pub fn read(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo, addr: usize) -> u8 {
        assert_eq!(addr, self.config.tcnt, "unknown timer read hook");
        self.count(cpu, io, false, false);
        if self.config.bits == 16 {
            cpu.data[addr + 1] = (self.tcnt >> 8) as u8;
        }
        cpu.data[addr] = self.tcnt as u8;
        cpu.data[addr]
    }
    pub fn write(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo, addr: usize, value: u8) {
        let c = self.config.clone();
        if addr == c.tcnt {
            self.tcnt_next = ((self.high_byte as i64) << 8) | value as i64;
            self.counting_up = true;
            self.tcnt_updated = true;
            cpu.update_clock_event(self.callback.clone(), 0);
            if self.divider != 0 {
                self.timer_updated(cpu, io, self.tcnt_next, self.tcnt_next);
            }
        } else if let Some(channel) = c.ocr.iter().position(|&reg| reg != 0 && reg == addr) {
            self.next_ocr[channel] = ((self.high_byte as i64) << 8) | value as i64;
            if self.ocr_update == Update::Immediate {
                self.ocr[channel] = self.next_ocr[channel];
            }
        } else if c.bits == 16 && addr == c.icr {
            self.icr = ((self.high_byte as i64) << 8) | value as i64;
        } else if c.bits == 16 && (addr == c.icr + 1 || addr == c.tcnt + 1) {
            self.high_byte = value;
        } else if c.bits == 16 && c.ocr.iter().any(|&reg| reg != 0 && addr == reg + 1) {
            self.high_byte = value & (self.ocr_mask() >> 8) as u8;
            cpu.data[addr] = self.high_byte;
            return;
        } else if addr == c.tccra {
            cpu.data[addr] = value;
            self.update_wgm(cpu, io);
            return;
        } else if addr == c.tccrb {
            let mut value = value;
            if c.tccrc == 0 {
                self.force_compare(cpu, io, value);
                value &= !0xc0;
            }
            cpu.data[addr] = value;
            self.update_divider = true;
            cpu.clear_clock_event(&self.callback);
            cpu.add_clock_event(self.callback.clone(), 0);
            self.update_wgm(cpu, io);
            return;
        } else if c.tccrc != 0 && addr == c.tccrc {
            self.force_compare(cpu, io, value);
        } else if addr == c.tifr {
            cpu.data[addr] = value;
            // Upstream deliberately handles OVF/A/B, not C, here.
            for i in [3, 0, 1] {
                cpu.clear_interrupt_by_flag(&self.interrupts[i], value);
            }
            return;
        } else if addr == c.timsk {
            for i in [3, 0, 1] {
                cpu.update_interrupt_enable(&self.interrupts[i], value);
            }
        } else {
            panic!("unknown timer write hook {addr:#x}");
        }
        cpu.data[addr] = value;
    }
    pub fn count(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo, reschedule: bool, external: bool) {
        let divider = self.divider;
        let delta = cpu.cycles - self.last_cycle;
        if (divider != 0 && delta >= divider) || external {
            let counter_delta = if external { 1 } else { delta / divider };
            self.last_cycle += counter_delta * divider;
            let old = self.tcnt;
            let mode = self.mode;
            let top = self.top();
            let phase = self.phase();
            let new_value = if phase {
                self.phase_count(cpu, io, old, counter_delta)
            } else {
                (old + counter_delta) % (top + 1)
            };
            let overflow = old + counter_delta > top;
            if !self.tcnt_updated {
                self.tcnt = new_value;
                if !phase {
                    self.timer_updated(cpu, io, new_value, old);
                }
            }
            if !phase {
                if mode == Fast && overflow {
                    for channel in 0..2 {
                        let comp = self.comp[channel];
                        if comp != 0 {
                            self.compare_output(cpu, io, comp, channel, true);
                        }
                    }
                }
                if self.ocr_update == Update::Bottom && overflow {
                    self.ocr = self.next_ocr;
                }
                if overflow && (self.tov_update == Overflow::Top || top == self.max) {
                    cpu.set_interrupt_flag(&self.interrupts[3]);
                }
            }
        }
        if self.tcnt_updated {
            self.tcnt = self.tcnt_next;
            self.tcnt_updated = false;
            if (self.tcnt == 0 && self.ocr_update == Update::Bottom)
                || (self.tcnt == self.top() && self.ocr_update == Update::Top)
            {
                self.ocr = self.next_ocr;
            }
        }
        if self.update_divider {
            let cs = cpu.data[self.config.tccrb] & 7;
            let new_divider = self.config.dividers[cs as usize];
            self.last_cycle = if new_divider != 0 { cpu.cycles } else { 0 };
            self.update_divider = false;
            self.divider = new_divider;
            if self.config.external_port != 0 && self.external_port.is_none() {
                self.external_port = cpu.gpio_by_port.get(&self.config.external_port).copied();
            }
            if let Some(port) = self.external_port {
                io.external_listener(port, self.config.external_pin, None);
            }
            if new_divider != 0 {
                cpu.add_clock_event(
                    self.callback.clone(),
                    self.last_cycle + new_divider - cpu.cycles,
                );
            } else if let Some(port) = self.external_port {
                if cs == 6 || cs == 7 {
                    io.external_listener(
                        port,
                        self.config.external_pin,
                        Some(self.external_callback.clone()),
                    );
                    self.external_rising = cs == 7;
                }
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
        io: &mut dyn TimerIo,
        mut value: i64,
        mut delta: i64,
    ) -> i64 {
        let ocr = self.ocr;
        let top = self.top();
        let updated = self.tcnt_updated;
        if value == 0 && top == 0 {
            delta = 0;
            if self.ocr_update == Update::Top {
                self.ocr = self.next_ocr;
            }
        }
        while delta > 0 {
            if self.counting_up {
                value += 1;
                if value == top && !updated {
                    self.counting_up = false;
                    if self.ocr_update == Update::Top {
                        self.ocr = self.next_ocr;
                    }
                }
            } else {
                value -= 1;
                if value == 0 && !updated {
                    self.counting_up = true;
                    cpu.set_interrupt_flag(&self.interrupts[3]);
                    if self.ocr_update == Update::Bottom {
                        self.ocr = self.next_ocr;
                    }
                }
            }
            if !updated {
                for (channel, &compare) in
                    ocr.iter()
                        .enumerate()
                        .take(if self.config.ocr[2] != 0 { 3 } else { 2 })
                {
                    if value == compare {
                        cpu.set_interrupt_flag(&self.interrupts[channel]);
                        let comp = self.comp[channel];
                        if comp != 0 {
                            self.compare_output(cpu, io, comp, channel, false);
                        }
                    }
                }
            }
            delta -= 1;
        }
        value & self.max
    }
    fn timer_updated(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo, value: i64, previous: i64) {
        let ocr = self.ocr;
        let overflow = previous > value;
        for (channel, &compare) in
            ocr.iter()
                .enumerate()
                .take(if self.config.ocr[2] != 0 { 3 } else { 2 })
        {
            if ((previous < compare || overflow) && value >= compare)
                || (previous < compare && overflow)
            {
                cpu.set_interrupt_flag(&self.interrupts[channel]);
                let comp = self.comp[channel];
                if comp != 0 {
                    self.compare_output(cpu, io, comp, channel, false);
                }
            }
        }
    }
    fn force_compare(&mut self, cpu: &mut Cpu, io: &mut dyn TimerIo, value: u8) {
        if self.pwm() {
            return;
        }
        for channel in 0..3 {
            if (channel != 2 || self.config.ports[2] != 0) && value & (0x80 >> channel) != 0 {
                self.compare_output(cpu, io, self.comp[channel], channel, false);
            }
        }
    }
    fn compare_output(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn TimerIo,
        comp: u8,
        channel: usize,
        bottom: bool,
    ) {
        let invert = comp == 3;
        let mode = match self.mode {
            Normal | Ctc => match comp {
                1 => 4,
                2 => 3,
                3 => 2,
                _ => 1,
            },
            Fast if comp == 1 => {
                if bottom {
                    0
                } else {
                    4
                }
            }
            Fast => {
                if invert != bottom {
                    2
                } else {
                    3
                }
            }
            Phase | PhaseFrequency if comp == 1 => 4,
            Phase | PhaseFrequency => {
                if self.counting_up == invert {
                    2
                } else {
                    3
                }
            }
            Reserved => 0,
        };
        if mode != 0 {
            io.output(self, cpu, channel, mode);
        }
    }
}

pub fn parse_config(value: &Value) -> TimerConfig {
    let object = match value {
        Value::Object(value) => value.borrow(),
        _ => panic!("timer config must be an object"),
    };
    let get = |key: &str| {
        object
            .get(key)
            .unwrap_or_else(|| panic!("missing timer config field {key}"))
            .number() as i64
    };
    let dividers = match &object["dividers"] {
        Value::Object(value) => value.borrow(),
        _ => panic!("invalid timer dividers"),
    };
    let bits = get("bits") as u8;
    assert!(bits == 8 || bits == 16, "timer bits must be 8 or 16");
    TimerConfig {
        bits,
        dividers: std::array::from_fn(|i| dividers[&i.to_string()].number() as i64),
        tifr: get("TIFR") as usize,
        timsk: get("TIMSK") as usize,
        tcnt: get("TCNT") as usize,
        tccra: get("TCCRA") as usize,
        tccrb: get("TCCRB") as usize,
        tccrc: get("TCCRC") as usize,
        icr: get("ICR") as usize,
        ocr: [
            get("OCRA") as usize,
            get("OCRB") as usize,
            get("OCRC") as usize,
        ],
        vectors: [
            get("compAInterrupt"),
            get("compBInterrupt"),
            get("compCInterrupt"),
            get("ovfInterrupt"),
        ],
        flags: [
            get("OCFA") as u8,
            get("OCFB") as u8,
            get("OCFC") as u8,
            get("TOV") as u8,
        ],
        enables: [
            get("OCIEA") as u8,
            get("OCIEB") as u8,
            get("OCIEC") as u8,
            get("TOIE") as u8,
        ],
        ports: [
            get("compPortA") as u16,
            get("compPortB") as u16,
            get("compPortC") as u16,
        ],
        pins: [
            get("compPinA") as u8,
            get("compPinB") as u8,
            get("compPinC") as u8,
        ],
        external_port: get("externalClockPort") as u16,
        external_pin: get("externalClockPin") as u8,
    }
}

pub fn config(number: u8) -> Value {
    let (bits, registers, vectors, ports, pins, external) = match number {
        0 => (
            8,
            [0x35, 0x47, 0x48, 0, 0, 0x46, 0x44, 0x45, 0, 0x6e],
            [0, 0x1c, 0x1e, 0, 0x20],
            [0x2b, 0x2b, 0],
            [6, 5, 0],
            [0x2b, 4],
        ),
        1 => (
            16,
            [0x36, 0x88, 0x8a, 0, 0x86, 0x84, 0x80, 0x81, 0x82, 0x6f],
            [0x14, 0x16, 0x18, 0, 0x1a],
            [0x25, 0x25, 0],
            [1, 2, 0],
            [0x2b, 5],
        ),
        2 => (
            8,
            [0x37, 0xb3, 0xb4, 0, 0, 0xb2, 0xb0, 0xb1, 0, 0x70],
            [0, 0x0e, 0x10, 0, 0x12],
            [0x25, 0x2b, 0],
            [3, 3, 0],
            [0, 0],
        ),
        _ => panic!("unknown timer config: {number}"),
    };
    let mut object = BTreeMap::new();
    let mut add = |name: &str, value: i64| {
        object.insert(name.to_string(), Value::Number(value as f64));
    };
    add("bits", bits);
    for (key, value) in [
        "TIFR", "OCRA", "OCRB", "OCRC", "ICR", "TCNT", "TCCRA", "TCCRB", "TCCRC", "TIMSK",
    ]
    .into_iter()
    .zip(registers)
    {
        add(key, value);
    }
    for (key, value) in [
        "captureInterrupt",
        "compAInterrupt",
        "compBInterrupt",
        "compCInterrupt",
        "ovfInterrupt",
    ]
    .into_iter()
    .zip(vectors)
    {
        add(key, value);
    }
    for (key, value) in ["compPortA", "compPortB", "compPortC"]
        .into_iter()
        .zip(ports)
    {
        add(key, value);
    }
    for (key, value) in ["compPinA", "compPinB", "compPinC"].into_iter().zip(pins) {
        add(key, value);
    }
    add("externalClockPort", external[0]);
    add("externalClockPin", external[1]);
    for (key, value) in [
        ("TOV", 1),
        ("OCFA", 2),
        ("OCFB", 4),
        ("OCFC", 0),
        ("TOIE", 1),
        ("OCIEA", 2),
        ("OCIEB", 4),
        ("OCIEC", 0),
    ] {
        add(key, value);
    }
    let dividers = if number == 2 {
        [0, 1, 8, 32, 64, 128, 256, 1024]
    } else {
        [0, 1, 8, 64, 256, 1024, 0, 0]
    };
    object.insert(
        "dividers".into(),
        Value::object(
            dividers
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), Value::Number(v as f64)))
                .collect(),
        ),
    );
    Value::object(object)
}
