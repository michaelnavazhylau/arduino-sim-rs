// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native port of `avr8js/src/cpu/{cpu,interrupt,instruction}.ts`.
//!
//! The executor mirrors upstream's decoder branch-for-branch, including its
//! wrapping/truncating arithmetic and its cycle accounting. All instruction
//! fields are read as `i64` so JavaScript's bitwise idioms can be transcribed
//! literally.
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;

/// One pending interrupt source, mirroring `AVRInterruptConfig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterruptConfig {
    pub address: u8,
    pub enable_register: u16,
    pub enable_mask: u8,
    pub flag_register: u16,
    pub flag_mask: u8,
    pub constant: bool,
    pub inverse_flag: bool,
}

/// Linked-list entry for the CPU clock event queue.
#[derive(Clone)]
pub struct ClockEvent {
    pub cycles: i64,
    pub callback: Value,
    pub next: Option<usize>,
}

/// Native AVR CPU state. `prog_bytes` is the single source of truth for program
/// memory; 16-bit words are derived little-endian on demand.
pub struct Cpu {
    pub data: Vec<u8>,
    pub prog_bytes: Vec<u8>,
    pub cycles: i64,
    pub pc: i64,
    pub pc22_bits: bool,
    pub read_hooks: Value,
    pub write_hooks: Value,
    pub pending_interrupts: Vec<Option<InterruptConfig>>,
    pub next_interrupt: i64,
    pub max_interrupt: i64,
    pub clock_events: Vec<ClockEvent>,
    pub next_clock_event: Option<usize>,
    pub free_events: Vec<usize>,
    /// JavaScript-level own properties, including installed spies and callbacks.
    pub props: BTreeMap<String, Value>,
    /// Native peripheral hooks registered by simulator objects (owner handle).
    pub native_write_hooks: BTreeMap<usize, Handle>,
    pub native_read_hooks: BTreeMap<usize, Handle>,
    /// Native GPIO ports, in registration order, plus a PORT-register lookup.
    pub gpio_ports: Vec<Handle>,
    pub gpio_by_port: BTreeMap<u16, Handle>,
    pub data_handle: Option<usize>,
    pub data_view_handle: Option<usize>,
    pub prog_handle: Option<usize>,
    pub prog_bytes_handle: Option<usize>,
}

const REGISTER_SPACE: usize = 0x100;
const MAX_INTERRUPTS: usize = 128;

impl Cpu {
    pub fn new(prog_bytes: Vec<u8>, sram_bytes: usize) -> Self {
        let mut cpu = Self {
            data: vec![0; sram_bytes + REGISTER_SPACE],
            pc22_bits: prog_bytes.len() > 0x20000,
            prog_bytes,
            cycles: 0,
            pc: 0,
            read_hooks: Value::array(Vec::new()),
            write_hooks: Value::array(Vec::new()),
            pending_interrupts: vec![None; MAX_INTERRUPTS],
            next_interrupt: -1,
            max_interrupt: 0,
            clock_events: Vec::new(),
            next_clock_event: None,
            free_events: Vec::new(),
            props: BTreeMap::new(),
            native_write_hooks: BTreeMap::new(),
            native_read_hooks: BTreeMap::new(),
            gpio_ports: Vec::new(),
            gpio_by_port: BTreeMap::new(),
            data_handle: None,
            data_view_handle: None,
            prog_handle: None,
            prog_bytes_handle: None,
        };
        cpu.reset();
        cpu
    }

    /// Upstream deliberately does not clear RAM or the cycle counter on reset.
    pub fn reset(&mut self) {
        self.set_sp((self.data.len() - 1) as u16);
        self.pc = 0;
        self.pending_interrupts.fill(None);
        self.next_interrupt = -1;
        self.next_clock_event = None;
    }

    pub fn spi(&self) -> u16 {
        self.sp()
    }

    pub fn sp(&self) -> u16 {
        self.get_u16(93)
    }
    pub fn set_sp(&mut self, value: u16) {
        self.set_u16(93, value);
    }
    pub fn sreg(&self) -> u8 {
        self.data[95]
    }
    pub fn set_sreg(&mut self, value: i64) {
        self.data[95] = (value & 0xff) as u8;
    }
    pub fn interrupts_enabled(&self) -> bool {
        self.sreg() & 0x80 != 0
    }

    pub fn get_u16(&self, addr: usize) -> u16 {
        if addr + 1 >= self.data.len() {
            return if addr < self.data.len() {
                u16::from(self.data[addr])
            } else {
                0
            };
        }
        u16::from_le_bytes([self.data[addr], self.data[addr + 1]])
    }
    pub fn set_u16(&mut self, addr: usize, value: u16) {
        if addr + 1 < self.data.len() {
            self.data[addr..addr + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    pub fn get_i16(&self, addr: usize) -> i16 {
        self.get_u16(addr) as i16
    }
    pub fn set_i16(&mut self, addr: usize, value: i16) {
        self.set_u16(addr, value as u16);
    }
    pub fn get_i8(&self, addr: usize) -> i8 {
        self.data[addr] as i8
    }

    /// Program-memory word fetch. Out-of-range reads behave like `undefined & x`
    /// (zero) instead of panicking.
    pub fn prog_word(&self, index: i64) -> u16 {
        let byte = index * 2;
        if byte < 0 || byte + 2 > self.prog_bytes.len() as i64 {
            return 0;
        }
        let byte = byte as usize;
        u16::from_le_bytes([self.prog_bytes[byte], self.prog_bytes[byte + 1]])
    }
    pub fn prog_len(&self) -> usize {
        self.prog_bytes.len() / 2
    }
    pub fn set_prog_byte(&mut self, index: i64, value: u8) {
        if index >= 0 && (index as usize) < self.prog_bytes.len() {
            self.prog_bytes[index as usize] = value;
        }
    }
    pub fn prog_byte(&self, index: i64) -> u8 {
        if index >= 0 && (index as usize) < self.prog_bytes.len() {
            self.prog_bytes[index as usize]
        } else {
            0
        }
    }

    pub fn install_write_hook(&mut self, addr: usize, owner: Handle) {
        self.native_write_hooks.insert(addr, owner);
        Self::clear_host_hook(&self.write_hooks, addr);
    }
    pub fn install_read_hook(&mut self, addr: usize, owner: Handle) {
        self.native_read_hooks.insert(addr, owner);
        Self::clear_host_hook(&self.read_hooks, addr);
    }
    fn clear_host_hook(hooks: &Value, addr: usize) {
        let Value::Array(hooks) = hooks else {
            panic!("CPU hooks must be arrays");
        };
        if let Some(hook) = hooks.borrow_mut().get_mut(addr) {
            *hook = Value::Undefined;
        }
    }

    pub fn read_data(&self, addr: usize) -> u8 {
        self.data.get(addr).copied().unwrap_or(0)
    }
    pub fn write_data(&mut self, addr: usize, value: i64, _mask: i64) {
        if let Some(slot) = self.data.get_mut(addr) {
            *slot = to_u8(value);
        }
    }

    pub fn queue_interrupt(&mut self, interrupt: InterruptConfig) {
        let address = interrupt.address as usize;
        self.pending_interrupts[address] = Some(interrupt);
        let address = address as i64;
        if self.next_interrupt == -1 || self.next_interrupt > address {
            self.next_interrupt = address;
        }
        if address > self.max_interrupt {
            self.max_interrupt = address;
        }
    }

    pub fn clear_interrupt(&mut self, interrupt: &InterruptConfig, clear_flag: bool) {
        if clear_flag {
            self.data[interrupt.flag_register as usize] &= !interrupt.flag_mask;
        }
        let address = interrupt.address as usize;
        if self.pending_interrupts[address].is_none() {
            return;
        }
        self.pending_interrupts[address] = None;
        if self.next_interrupt == address as i64 {
            self.next_interrupt = -1;
            for i in (address + 1)..=(self.max_interrupt as usize) {
                if self.pending_interrupts[i].is_some() {
                    self.next_interrupt = i as i64;
                    break;
                }
            }
        }
    }

    pub fn set_interrupt_flag(&mut self, interrupt: &InterruptConfig) {
        let flag_register = interrupt.flag_register as usize;
        if interrupt.inverse_flag {
            self.data[flag_register] &= !interrupt.flag_mask;
        } else {
            self.data[flag_register] |= interrupt.flag_mask;
        }
        if self.data[interrupt.enable_register as usize] & interrupt.enable_mask != 0 {
            self.queue_interrupt(interrupt.clone());
        }
    }

    pub fn update_interrupt_enable(&mut self, interrupt: &InterruptConfig, register_value: u8) {
        if register_value & interrupt.enable_mask != 0 {
            let bit_set = self.data[interrupt.flag_register as usize] & interrupt.flag_mask != 0;
            if if interrupt.inverse_flag {
                !bit_set
            } else {
                bit_set
            } {
                self.queue_interrupt(interrupt.clone());
            }
        } else {
            self.clear_interrupt(interrupt, false);
        }
    }

    pub fn clear_interrupt_by_flag(&mut self, interrupt: &InterruptConfig, register_value: u8) {
        if register_value & interrupt.flag_mask != 0 {
            self.data[interrupt.flag_register as usize] &= !interrupt.flag_mask;
            self.clear_interrupt(interrupt, true);
        }
    }

    /// Insert a callback into the clock queue, preserving upstream's stable
    /// equal-deadline ordering (new equal events go before existing ones).
    pub fn add_clock_event(&mut self, callback: Value, cycles: i64) -> Value {
        let deadline = self.cycles + cycles.max(1);
        let index = match self.free_events.pop() {
            Some(index) => index,
            None => {
                self.clock_events.push(ClockEvent {
                    cycles: deadline,
                    callback: callback.clone(),
                    next: None,
                });
                self.clock_events.len() - 1
            }
        };
        self.clock_events[index] = ClockEvent {
            cycles: deadline,
            callback: callback.clone(),
            next: None,
        };
        let mut current = self.next_clock_event;
        let mut last = None;
        while let Some(entry) = current {
            if self.clock_events[entry].cycles < deadline {
                last = Some(entry);
                current = self.clock_events[entry].next;
            } else {
                break;
            }
        }
        match last {
            Some(last) => {
                self.clock_events[index].next = self.clock_events[last].next;
                self.clock_events[last].next = Some(index);
            }
            None => {
                self.clock_events[index].next = self.next_clock_event;
                self.next_clock_event = Some(index);
            }
        }
        callback
    }

    pub fn clear_clock_event(&mut self, callback: &Value) -> bool {
        let mut current = self.next_clock_event;
        let mut last: Option<usize> = None;
        while let Some(entry) = current {
            if deep_equal(&self.clock_events[entry].callback, callback) {
                match last {
                    Some(last) => self.clock_events[last].next = self.clock_events[entry].next,
                    None => self.next_clock_event = self.clock_events[entry].next,
                }
                if self.free_events.len() < 10 {
                    self.free_events.push(entry);
                }
                return true;
            }
            last = Some(entry);
            current = self.clock_events[entry].next;
        }
        false
    }

    pub fn update_clock_event(&mut self, callback: Value, cycles: i64) -> bool {
        if self.clear_clock_event(&callback) {
            self.add_clock_event(callback, cycles);
            true
        } else {
            false
        }
    }
}

/// Convert an arbitrary numeric operand into a stored byte the way a typed array
/// assignment does (truncate toward zero, then wrap into 0..=255).
pub fn to_u8(value: i64) -> u8 {
    value.rem_euclid(256) as u8
}

fn deep_equal(a: &Value, b: &Value) -> bool {
    crate::runtime::deep_equal(a, b)
}

/// Read the low/high byte pair used by `data` direct indexing.
pub fn data(cpu: &Cpu, addr: i64) -> i64 {
    if addr < 0 || addr as usize >= cpu.data.len() {
        return 0;
    }
    cpu.data[addr as usize] as i64
}

pub fn set_data(cpu: &mut Cpu, addr: i64, value: i64) {
    if addr < 0 || addr as usize >= cpu.data.len() {
        return;
    }
    cpu.data[addr as usize] = to_u8(value);
}

/// `avrInterrupt`: push the return address, clear I, add two cycles, set PC.
pub fn avr_interrupt(cpu: &mut Cpu, addr: i64) {
    let sp = cpu.sp() as i64;
    set_data(cpu, sp, cpu.pc & 0xff);
    set_data(cpu, sp - 1, (cpu.pc >> 8) & 0xff);
    if cpu.pc22_bits {
        set_data(cpu, sp - 2, (cpu.pc >> 16) & 0xff);
    }
    cpu.set_sp((sp - if cpu.pc22_bits { 3 } else { 2 }) as u16);
    cpu.data[95] &= 0x7f;
    cpu.cycles += 2;
    cpu.pc = addr;
}

fn is_two_word_instruction(opcode: i64) -> bool {
    (opcode & 0xfe0f) == 0x9000
        || (opcode & 0xfe0f) == 0x9200
        || (opcode & 0xfe0e) == 0x940e
        || (opcode & 0xfe0e) == 0x940c
}

fn rd(op: i64) -> i64 {
    (op & 0x1f0) >> 4
}
fn rr(op: i64) -> i64 {
    (op & 0xf) | ((op & 0x200) >> 5)
}
fn displacement(op: i64) -> i64 {
    (op & 7) | ((op & 0xc00) >> 7) | ((op & 0x2000) >> 8)
}

/// Data-memory bus. Native peripherals install hooks that must observe every
/// `readData`/`writeData` from executed instructions as well as from adapter calls.
pub trait Bus {
    fn read_data(&mut self, cpu: &mut Cpu, addr: usize) -> u8;
    fn write_data(&mut self, cpu: &mut Cpu, addr: usize, value: i64, mask: i64);
}

/// Execute one already-fetched instruction. Callers invoke `onWatchdogReset`
/// separately, because a host callback must not run while `Cpu` is borrowed.
pub fn execute(cpu: &mut Cpu, bus: &mut dyn Bus, opcode: i64) {
    let op = opcode;
    if (op & 0xfc00) == 0x1c00 {
        /* ADC */
        let d = rd(op);
        let r = rr(op);
        let (dv, rv) = (data(cpu, d), data(cpu, r));
        let sum = dv + rv + (cpu.sreg() as i64 & 1);
        let result = sum & 255;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if (result ^ rv) & (dv ^ result) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if sum & 256 != 0 { 1 } else { 0 };
        sreg |= if 1 & ((dv & rv) | (rv & !result) | (!result & dv)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x0c00 {
        /* ADD */
        let d = rd(op);
        let r = rr(op);
        let (dv, rv) = (data(cpu, d), data(cpu, r));
        let result = (dv + rv) & 255;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if (result ^ rv) & (result ^ dv) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if (dv + rv) & 256 != 0 { 1 } else { 0 };
        sreg |= if 1 & ((dv & rv) | (rv & !result) | (!result & dv)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xff00) == 0x9600 {
        /* ADIW */
        let addr = 2 * ((op & 0x30) >> 4) + 24;
        let value = cpu.get_u16(addr as usize) as i64;
        let result = (value + ((op & 0xf) | ((op & 0xc0) >> 2))) & 0xffff;
        cpu.set_u16(addr as usize, result as u16);
        let mut sreg = cpu.sreg() as i64 & 0xe0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 0x8000 & result != 0 { 4 } else { 0 };
        sreg |= if !value & result & 0x8000 != 0 { 8 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if !result & value & 0x8000 != 0 { 1 } else { 0 };
        cpu.set_sreg(sreg);
        cpu.cycles += 1;
    } else if (op & 0xfc00) == 0x2000 {
        /* AND */
        let d = rd(op);
        let result = data(cpu, d) & data(cpu, rr(op));
        set_data(cpu, d, result);
        logical_flags(cpu, result, 0xe1);
    } else if (op & 0xf000) == 0x7000 {
        /* ANDI */
        let d = ((op & 0xf0) >> 4) + 16;
        let result = data(cpu, d) & ((op & 0xf) | ((op & 0xf00) >> 4));
        set_data(cpu, d, result);
        logical_flags(cpu, result, 0xe1);
    } else if (op & 0xfe0f) == 0x9405 {
        /* ASR */
        let d = rd(op);
        let value = data(cpu, d);
        let result = (value >> 1) | (128 & value);
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xe0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= value & 1;
        sreg |= if ((sreg >> 2) & 1) ^ (sreg & 1) != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xff8f) == 0x9488 {
        /* BCLR */
        cpu.data[95] &= !(1 << ((op & 0x70) >> 4));
    } else if (op & 0xfe08) == 0xf800 {
        /* BLD */
        let b = op & 7;
        let d = rd(op);
        set_data(
            cpu,
            d,
            (!(1 << b) & data(cpu, d)) | (((cpu.sreg() as i64 >> 6) & 1) << b),
        );
    } else if (op & 0xfc00) == 0xf400 {
        /* BRBC */
        if cpu.sreg() as i64 & (1 << (op & 7)) == 0 {
            cpu.pc += ((op & 0x1f8) >> 3) - if op & 0x200 != 0 { 0x40 } else { 0 };
            cpu.cycles += 1;
        }
    } else if (op & 0xfc00) == 0xf000 {
        /* BRBS */
        if cpu.sreg() as i64 & (1 << (op & 7)) != 0 {
            cpu.pc += ((op & 0x1f8) >> 3) - if op & 0x200 != 0 { 0x40 } else { 0 };
            cpu.cycles += 1;
        }
    } else if (op & 0xff8f) == 0x9408 {
        /* BSET */
        cpu.data[95] |= 1 << ((op & 0x70) >> 4);
    } else if (op & 0xfe08) == 0xfa00 {
        /* BST */
        let d = data(cpu, rd(op));
        let b = op & 7;
        cpu.data[95] =
            ((cpu.sreg() as i64 & 0xbf) | (if (d >> b) & 1 != 0 { 0x40 } else { 0 })) as u8;
    } else if (op & 0xfe0e) == 0x940e {
        /* CALL */
        let k = cpu.prog_word(cpu.pc + 1) as i64 | ((op & 1) << 16) | ((op & 0x1f0) << 13);
        let ret = cpu.pc + 2;
        let sp = cpu.sp() as i64;
        let pc22 = cpu.pc22_bits;
        set_data(cpu, sp, 255 & ret);
        set_data(cpu, sp - 1, (ret >> 8) & 255);
        if pc22 {
            set_data(cpu, sp - 2, (ret >> 16) & 255);
        }
        cpu.set_sp((sp - if pc22 { 3 } else { 2 }) as u16);
        cpu.pc = k - 1;
        cpu.cycles += if pc22 { 4 } else { 3 };
    } else if (op & 0xff00) == 0x9800 {
        /* CBI */
        let a = op & 0xf8;
        let b = op & 7;
        let value = bus.read_data(cpu, ((a >> 3) + 32) as usize) as i64;
        let mask = 1 << b;
        bus.write_data(cpu, ((a >> 3) + 32) as usize, value & !mask, mask);
    } else if (op & 0xfe0f) == 0x9400 {
        /* COM */
        let d = rd(op);
        let result = 255 - data(cpu, d);
        set_data(cpu, d, result);
        let mut sreg = (cpu.sreg() as i64 & 0xe1) | 1;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x1400 {
        /* CP */
        let val1 = data(cpu, rd(op));
        let val2 = data(cpu, rr(op));
        let result = val1 - val2;
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if 0 != ((val1 ^ val2) & (val1 ^ result) & 128) {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if val2 > val1 { 1 } else { 0 };
        sreg |= if 1 & ((!val1 & val2) | (val2 & result) | (result & !val1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x0400 {
        /* CPC */
        let arg1 = data(cpu, rd(op));
        let arg2 = data(cpu, rr(op));
        let mut sreg = cpu.sreg() as i64;
        let r = arg1 - arg2 - (sreg & 1);
        sreg = (sreg & 0xc0)
            | (if r == 0 && (sreg >> 1) & 1 != 0 { 2 } else { 0 })
            | (if arg2 + (sreg & 1) > arg1 { 1 } else { 0 });
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (arg1 ^ arg2) & (arg1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if 1 & ((!arg1 & arg2) | (arg2 & r) | (r & !arg1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xf000) == 0x3000 {
        /* CPI */
        let arg1 = data(cpu, ((op & 0xf0) >> 4) + 16);
        let arg2 = (op & 0xf) | ((op & 0xf00) >> 4);
        let r = arg1 - arg2;
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if r != 0 { 0 } else { 2 };
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (arg1 ^ arg2) & (arg1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if arg2 > arg1 { 1 } else { 0 };
        sreg |= if 1 & ((!arg1 & arg2) | (arg2 & r) | (r & !arg1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x1000 {
        /* CPSE */
        if data(cpu, rd(op)) == data(cpu, rr(op)) {
            let skip = skip_size(cpu, cpu.pc + 1);
            cpu.pc += skip;
            cpu.cycles += skip;
        }
    } else if (op & 0xfe0f) == 0x940a {
        /* DEC */
        let d = rd(op);
        let value = data(cpu, d);
        let result = value - 1;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xe1;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if value == 128 { 8 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if op == 0x9519 {
        /* EICALL */
        let ret = cpu.pc + 1;
        let sp = cpu.sp() as i64;
        let eind = data(cpu, 0x5c);
        set_data(cpu, sp, ret & 255);
        set_data(cpu, sp - 1, (ret >> 8) & 255);
        set_data(cpu, sp - 2, (ret >> 16) & 255);
        cpu.set_sp((sp - 3) as u16);
        cpu.pc = ((eind << 16) | cpu.get_u16(30) as i64) - 1;
        cpu.cycles += 3;
    } else if op == 0x9419 {
        /* EIJMP */
        let eind = data(cpu, 0x5c);
        cpu.pc = ((eind << 16) | cpu.get_u16(30) as i64) - 1;
        cpu.cycles += 1;
    } else if op == 0x95d8 {
        /* ELPM */
        let rampz = data(cpu, 0x5b);
        set_data(
            cpu,
            0,
            cpu.prog_byte((rampz << 16) | cpu.get_u16(30) as i64) as i64,
        );
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9006 {
        /* ELPM(REG) */
        let rampz = data(cpu, 0x5b);
        let value = cpu.prog_byte((rampz << 16) | cpu.get_u16(30) as i64);
        set_data(cpu, rd(op), value as i64);
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9007 {
        /* ELPM(INC) */
        let rampz = data(cpu, 0x5b);
        let i = cpu.get_u16(30) as i64;
        let value = cpu.prog_byte((rampz << 16) | i);
        set_data(cpu, rd(op), value as i64);
        cpu.set_u16(30, (i + 1) as u16);
        if i == 0xffff {
            cpu.data[0x5b] = ((rampz + 1) % (cpu.prog_bytes.len() as i64 >> 16)) as u8;
        }
        cpu.cycles += 2;
    } else if (op & 0xfc00) == 0x2400 {
        /* EOR */
        let d = rd(op);
        let result = data(cpu, d) ^ data(cpu, rr(op));
        set_data(cpu, d, result);
        logical_flags(cpu, result, 0xe1);
    } else if (op & 0xff88) == 0x0308 {
        /* FMUL */
        let v1 = data(cpu, ((op & 0x70) >> 4) + 16);
        let v2 = data(cpu, (op & 7) + 16);
        let product = v1 * v2;
        let result = product << 1;
        cpu.set_u16(0, result as u16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 0 } else { 2 })
            | (if product & 0x8000 != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if (op & 0xff88) == 0x0380 {
        /* FMULS */
        let v1 = cpu.get_i8((((op & 0x70) >> 4) + 16) as usize) as i64;
        let v2 = cpu.get_i8(((op & 7) + 16) as usize) as i64;
        let product = v1 * v2;
        let result = product << 1;
        cpu.set_i16(0, result as i16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 0 } else { 2 })
            | (if product & 0x8000 != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if (op & 0xff88) == 0x0388 {
        /* FMULSU */
        let v1 = cpu.get_i8((((op & 0x70) >> 4) + 16) as usize) as i64;
        let v2 = data(cpu, (op & 7) + 16);
        let product = v1 * v2;
        let result = product << 1;
        cpu.set_i16(0, result as i16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 2 } else { 0 })
            | (if product & 0x8000 != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if op == 0x9509 {
        /* ICALL */
        let ret = cpu.pc + 1;
        let sp = cpu.sp() as i64;
        let pc22 = cpu.pc22_bits;
        set_data(cpu, sp, ret & 255);
        set_data(cpu, sp - 1, (ret >> 8) & 255);
        if pc22 {
            set_data(cpu, sp - 2, (ret >> 16) & 255);
        }
        cpu.set_sp((sp - if pc22 { 3 } else { 2 }) as u16);
        cpu.pc = cpu.get_u16(30) as i64 - 1;
        cpu.cycles += if pc22 { 3 } else { 2 };
    } else if op == 0x9409 {
        /* IJMP */
        cpu.pc = cpu.get_u16(30) as i64 - 1;
        cpu.cycles += 1;
    } else if (op & 0xf800) == 0xb000 {
        /* IN */
        let i = bus.read_data(cpu, (((op & 0xf) | ((op & 0x600) >> 5)) + 32) as usize) as i64;
        set_data(cpu, rd(op), i);
    } else if (op & 0xfe0f) == 0x9403 {
        /* INC */
        let d = rd(op);
        let value = data(cpu, d);
        let result = (value + 1) & 255;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xe1;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if value == 127 { 8 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfe0e) == 0x940c {
        /* JMP */
        cpu.pc = (cpu.prog_word(cpu.pc + 1) as i64 | ((op & 1) << 16) | ((op & 0x1f0) << 13)) - 1;
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9206 {
        /* LAC */
        let r = rd(op);
        let clear = data(cpu, r);
        let address = cpu.get_u16(30) as usize;
        let value = bus.read_data(cpu, address) as i64;
        bus.write_data(cpu, address, value & (255 - clear), 0xff);
        set_data(cpu, r, value);
    } else if (op & 0xfe0f) == 0x9205 {
        /* LAS */
        let r = rd(op);
        let set = data(cpu, r);
        let address = cpu.get_u16(30) as usize;
        let value = bus.read_data(cpu, address) as i64;
        bus.write_data(cpu, address, value | set, 0xff);
        set_data(cpu, r, value);
    } else if (op & 0xfe0f) == 0x9207 {
        /* LAT */
        let r = rd(op);
        let value = data(cpu, r);
        let address = cpu.get_u16(30) as usize;
        let result = bus.read_data(cpu, address) as i64;
        bus.write_data(cpu, address, value ^ result, 0xff);
        set_data(cpu, r, result);
    } else if (op & 0xf000) == 0xe000 {
        /* LDI */
        set_data(
            cpu,
            ((op & 0xf0) >> 4) + 16,
            (op & 0xf) | ((op & 0xf00) >> 4),
        );
    } else if (op & 0xfe0f) == 0x9000 {
        /* LDS */
        cpu.cycles += 1;
        let address = cpu.prog_word(cpu.pc + 1) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
        cpu.pc += 1;
    } else if (op & 0xfe0f) == 0x900c {
        /* LDX */
        cpu.cycles += 1;
        let address = cpu.get_u16(26) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xfe0f) == 0x900d {
        /* LDX(INC) */
        let x = cpu.get_u16(26) as i64;
        cpu.cycles += 1;
        let value = bus.read_data(cpu, x as usize) as i64;
        set_data(cpu, rd(op), value);
        cpu.set_u16(26, (x + 1) as u16);
    } else if (op & 0xfe0f) == 0x900e {
        /* LDX(DEC) */
        let x = cpu.get_u16(26) as i64 - 1;
        cpu.set_u16(26, x as u16);
        cpu.cycles += 1;
        let value = bus.read_data(cpu, x as usize) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xfe0f) == 0x8008 {
        /* LDY */
        cpu.cycles += 1;
        let address = cpu.get_u16(28) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xfe0f) == 0x9009 {
        /* LDY(INC) */
        let y = cpu.get_u16(28) as i64;
        cpu.cycles += 1;
        let value = bus.read_data(cpu, y as usize) as i64;
        set_data(cpu, rd(op), value);
        cpu.set_u16(28, (y + 1) as u16);
    } else if (op & 0xfe0f) == 0x900a {
        /* LDY(DEC) */
        let y = cpu.get_u16(28) as i64 - 1;
        cpu.set_u16(28, y as u16);
        cpu.cycles += 1;
        let value = bus.read_data(cpu, y as usize) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xd208) == 0x8008 && displacement(op) != 0 {
        /* LDDY */
        cpu.cycles += 1;
        let address = (cpu.get_u16(28) as i64 + displacement(op)) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xfe0f) == 0x8000 {
        /* LDZ */
        cpu.cycles += 1;
        let address = cpu.get_u16(30) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xfe0f) == 0x9001 {
        /* LDZ(INC) */
        let z = cpu.get_u16(30) as i64;
        cpu.cycles += 1;
        let value = bus.read_data(cpu, z as usize) as i64;
        set_data(cpu, rd(op), value);
        cpu.set_u16(30, (z + 1) as u16);
    } else if (op & 0xfe0f) == 0x9002 {
        /* LDZ(DEC) */
        let z = cpu.get_u16(30) as i64 - 1;
        cpu.set_u16(30, z as u16);
        cpu.cycles += 1;
        let value = bus.read_data(cpu, z as usize) as i64;
        set_data(cpu, rd(op), value);
    } else if (op & 0xd208) == 0x8000 && displacement(op) != 0 {
        /* LDDZ */
        cpu.cycles += 1;
        let address = (cpu.get_u16(30) as i64 + displacement(op)) as usize;
        let value = bus.read_data(cpu, address) as i64;
        set_data(cpu, rd(op), value);
    } else if op == 0x95c8 {
        /* LPM */
        set_data(cpu, 0, cpu.prog_byte(cpu.get_u16(30) as i64) as i64);
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9004 {
        /* LPM(REG) */
        let value = cpu.prog_byte(cpu.get_u16(30) as i64);
        set_data(cpu, rd(op), value as i64);
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9005 {
        /* LPM(INC) */
        let i = cpu.get_u16(30) as i64;
        let value = cpu.prog_byte(i);
        set_data(cpu, rd(op), value as i64);
        cpu.set_u16(30, (i + 1) as u16);
        cpu.cycles += 2;
    } else if (op & 0xfe0f) == 0x9406 {
        /* LSR */
        let d = rd(op);
        let value = data(cpu, d);
        let result = value >> 1;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xe0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= value & 1;
        sreg |= if ((sreg >> 2) & 1) ^ (sreg & 1) != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x2c00 {
        /* MOV */
        set_data(cpu, rd(op), data(cpu, rr(op)));
    } else if (op & 0xff00) == 0x0100 {
        /* MOVW */
        let r2 = 2 * (op & 0xf);
        let d2 = 2 * ((op & 0xf0) >> 4);
        set_data(cpu, d2, data(cpu, r2));
        set_data(cpu, d2 + 1, data(cpu, r2 + 1));
    } else if (op & 0xfc00) == 0x9c00 {
        /* MUL */
        let result = data(cpu, rd(op)) * data(cpu, rr(op));
        cpu.set_u16(0, result as u16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 0 } else { 2 })
            | (if 0x8000 & result != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if (op & 0xff00) == 0x0200 {
        /* MULS */
        let result = cpu.get_i8((((op & 0xf0) >> 4) + 16) as usize) as i64
            * cpu.get_i8(((op & 0xf) + 16) as usize) as i64;
        cpu.set_i16(0, result as i16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 0 } else { 2 })
            | (if 0x8000 & result != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if (op & 0xff88) == 0x0300 {
        /* MULSU */
        let result =
            cpu.get_i8((((op & 0x70) >> 4) + 16) as usize) as i64 * data(cpu, (op & 7) + 16);
        cpu.set_i16(0, result as i16);
        cpu.data[95] = ((cpu.sreg() as i64 & 0xfc)
            | (if 0xffff & result != 0 { 0 } else { 2 })
            | (if 0x8000 & result != 0 { 1 } else { 0 })) as u8;
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x9401 {
        /* NEG */
        let d = rd(op);
        let value = data(cpu, d);
        let result = -value;
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if result == 128 { 8 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if result != 0 { 1 } else { 0 };
        sreg |= if 1 & (result | value) != 0 { 0x20 } else { 0 };
        cpu.set_sreg(sreg);
    } else if op == 0 {
        /* NOP */
    } else if (op & 0xfc00) == 0x2800 {
        /* OR */
        let d = rd(op);
        let result = data(cpu, d) | data(cpu, rr(op));
        set_data(cpu, d, result);
        logical_flags(cpu, result, 0xe1);
    } else if (op & 0xf000) == 0x6000 {
        /* SBR */
        let d = ((op & 0xf0) >> 4) + 16;
        let result = data(cpu, d) | ((op & 0xf) | ((op & 0xf00) >> 4));
        set_data(cpu, d, result);
        logical_flags(cpu, result, 0xe1);
    } else if (op & 0xf800) == 0xb800 {
        /* OUT */
        let value = data(cpu, rd(op));
        bus.write_data(
            cpu,
            (((op & 0xf) | ((op & 0x600) >> 5)) + 32) as usize,
            value,
            0xff,
        );
    } else if (op & 0xfe0f) == 0x900f {
        /* POP */
        let value = cpu.get_u16(93) as i64 + 1;
        cpu.set_u16(93, value as u16);
        let byte = bus.read_data(cpu, value as usize) as i64;
        set_data(cpu, rd(op), byte);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x920f {
        /* PUSH */
        let value = cpu.get_u16(93) as i64;
        let byte = data(cpu, rd(op));
        set_data(cpu, value, byte);
        cpu.set_u16(93, (value - 1) as u16);
        cpu.cycles += 1;
    } else if (op & 0xf000) == 0xd000 {
        /* RCALL */
        let k = (op & 0x7ff) - if op & 0x800 != 0 { 0x800 } else { 0 };
        let ret = cpu.pc + 1;
        let sp = cpu.sp() as i64;
        let pc22 = cpu.pc22_bits;
        set_data(cpu, sp, 255 & ret);
        set_data(cpu, sp - 1, (ret >> 8) & 255);
        if pc22 {
            set_data(cpu, sp - 2, (ret >> 16) & 255);
        }
        cpu.set_sp((sp - if pc22 { 3 } else { 2 }) as u16);
        cpu.pc += k;
        cpu.cycles += if pc22 { 3 } else { 2 };
    } else if op == 0x9508 {
        /* RET */
        let pc22 = cpu.pc22_bits;
        let i = cpu.get_u16(93) as i64 + if pc22 { 3 } else { 2 };
        cpu.set_u16(93, i as u16);
        cpu.pc = (data(cpu, i - 1) << 8) + data(cpu, i) - 1;
        if pc22 {
            cpu.pc |= data(cpu, i - 2) << 16;
        }
        cpu.cycles += if pc22 { 4 } else { 3 };
    } else if op == 0x9518 {
        /* RETI */
        let pc22 = cpu.pc22_bits;
        let i = cpu.get_u16(93) as i64 + if pc22 { 3 } else { 2 };
        cpu.set_u16(93, i as u16);
        cpu.pc = (data(cpu, i - 1) << 8) + data(cpu, i) - 1;
        if pc22 {
            cpu.pc |= data(cpu, i - 2) << 16;
        }
        cpu.cycles += if pc22 { 4 } else { 3 };
        cpu.data[95] |= 0x80;
    } else if (op & 0xf000) == 0xc000 {
        /* RJMP */
        cpu.pc += (op & 0x7ff) - if op & 0x800 != 0 { 0x800 } else { 0 };
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x9407 {
        /* ROR */
        let d = rd(op);
        let value = data(cpu, d);
        let result = (value >> 1) | ((cpu.sreg() as i64 & 1) << 7);
        set_data(cpu, d, result);
        let mut sreg = cpu.sreg() as i64 & 0xe0;
        sreg |= if result != 0 { 0 } else { 2 };
        sreg |= if 128 & result != 0 { 4 } else { 0 };
        sreg |= if 1 & value != 0 { 1 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ (sreg & 1) != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfc00) == 0x0800 {
        /* SBC */
        let val1 = data(cpu, rd(op));
        let val2 = data(cpu, rr(op));
        let mut sreg = cpu.sreg() as i64;
        let r = val1 - val2 - (sreg & 1);
        set_data(cpu, rd(op), r);
        sreg = (sreg & 0xc0)
            | (if r == 0 && (sreg >> 1) & 1 != 0 { 2 } else { 0 })
            | (if val2 + (sreg & 1) > val1 { 1 } else { 0 });
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (val1 ^ val2) & (val1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if 1 & ((!val1 & val2) | (val2 & r) | (r & !val1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xf000) == 0x4000 {
        /* SBCI */
        let d = ((op & 0xf0) >> 4) + 16;
        let val1 = data(cpu, d);
        let val2 = (op & 0xf) | ((op & 0xf00) >> 4);
        let mut sreg = cpu.sreg() as i64;
        let r = val1 - val2 - (sreg & 1);
        set_data(cpu, d, r);
        sreg = (sreg & 0xc0)
            | (if r == 0 && (sreg >> 1) & 1 != 0 { 2 } else { 0 })
            | (if val2 + (sreg & 1) > val1 { 1 } else { 0 });
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (val1 ^ val2) & (val1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if 1 & ((!val1 & val2) | (val2 & r) | (r & !val1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xff00) == 0x9a00 {
        /* SBI */
        let target = ((op & 0xf8) >> 3) + 32;
        let mask = 1 << (op & 7);
        let value = bus.read_data(cpu, target as usize) as i64;
        bus.write_data(cpu, target as usize, value | mask, mask);
        cpu.cycles += 1;
    } else if (op & 0xff00) == 0x9900 {
        /* SBIC */
        let value = bus.read_data(cpu, ((op & 0xf8) >> 3) as usize + 32) as i64;
        if value & (1 << (op & 7)) == 0 {
            let skip = skip_size(cpu, cpu.pc + 1);
            cpu.cycles += skip;
            cpu.pc += skip;
        }
    } else if (op & 0xff00) == 0x9b00 {
        /* SBIS */
        let value = bus.read_data(cpu, ((op & 0xf8) >> 3) as usize + 32) as i64;
        if value & (1 << (op & 7)) != 0 {
            let skip = skip_size(cpu, cpu.pc + 1);
            cpu.cycles += skip;
            cpu.pc += skip;
        }
    } else if (op & 0xff00) == 0x9700 {
        /* SBIW */
        let i = 2 * ((op & 0x30) >> 4) + 24;
        let a = cpu.get_u16(i as usize) as i64;
        let l = (op & 0xf) | ((op & 0xc0) >> 2);
        let r = a - l;
        cpu.set_u16(i as usize, r as u16);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if r != 0 { 0 } else { 2 };
        sreg |= if 0x8000 & r != 0 { 4 } else { 0 };
        sreg |= if a & !r & 0x8000 != 0 { 8 } else { 0 };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if l > a { 1 } else { 0 };
        sreg |= if 1 & ((!a & l) | (l & r) | (r & !a)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
        cpu.cycles += 1;
    } else if (op & 0xfe08) == 0xfc00 {
        /* SBRC */
        if data(cpu, rd(op)) & (1 << (op & 7)) == 0 {
            let skip = skip_size(cpu, cpu.pc + 1);
            cpu.cycles += skip;
            cpu.pc += skip;
        }
    } else if (op & 0xfe08) == 0xfe00 {
        /* SBRS */
        if data(cpu, rd(op)) & (1 << (op & 7)) != 0 {
            let skip = skip_size(cpu, cpu.pc + 1);
            cpu.cycles += skip;
            cpu.pc += skip;
        }
    } else if op == 0x9588 {
        /* SLEEP: not implemented upstream */
    } else if op == 0x95e8 || op == 0x95f8 {
        /* SPM / SPM(INC): not implemented upstream */
    } else if (op & 0xfe0f) == 0x9200 {
        /* STS */
        let value = data(cpu, rd(op));
        let address = cpu.prog_word(cpu.pc + 1) as usize;
        bus.write_data(cpu, address, value, 0xff);
        cpu.pc += 1;
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x920c {
        /* STX */
        let address = cpu.get_u16(26) as usize;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, address, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x920d {
        /* STX(INC) */
        let x = cpu.get_u16(26) as i64;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, x as usize, value, 0xff);
        cpu.set_u16(26, (x + 1) as u16);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x920e {
        /* STX(DEC) */
        let value = data(cpu, rd(op));
        let x = cpu.get_u16(26) as i64 - 1;
        cpu.set_u16(26, x as u16);
        bus.write_data(cpu, x as usize, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x8208 {
        /* STY */
        let address = cpu.get_u16(28) as usize;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, address, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x9209 {
        /* STY(INC) */
        let value = data(cpu, rd(op));
        let y = cpu.get_u16(28) as i64;
        bus.write_data(cpu, y as usize, value, 0xff);
        cpu.set_u16(28, (y + 1) as u16);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x920a {
        /* STY(DEC) */
        let value = data(cpu, rd(op));
        let y = cpu.get_u16(28) as i64 - 1;
        cpu.set_u16(28, y as u16);
        bus.write_data(cpu, y as usize, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xd208) == 0x8208 && displacement(op) != 0 {
        /* STDY */
        let address = (cpu.get_u16(28) as i64 + displacement(op)) as usize;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, address, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x8200 {
        /* STZ */
        let address = cpu.get_u16(30) as usize;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, address, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x9201 {
        /* STZ(INC) */
        let z = cpu.get_u16(30) as i64;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, z as usize, value, 0xff);
        cpu.set_u16(30, (z + 1) as u16);
        cpu.cycles += 1;
    } else if (op & 0xfe0f) == 0x9202 {
        /* STZ(DEC) */
        let value = data(cpu, rd(op));
        let z = cpu.get_u16(30) as i64 - 1;
        cpu.set_u16(30, z as u16);
        bus.write_data(cpu, z as usize, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xd208) == 0x8200 && displacement(op) != 0 {
        /* STDZ */
        let address = (cpu.get_u16(30) as i64 + displacement(op)) as usize;
        let value = data(cpu, rd(op));
        bus.write_data(cpu, address, value, 0xff);
        cpu.cycles += 1;
    } else if (op & 0xfc00) == 0x1800 {
        /* SUB */
        let val1 = data(cpu, rd(op));
        let val2 = data(cpu, rr(op));
        let r = val1 - val2;
        set_data(cpu, rd(op), r);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if r != 0 { 0 } else { 2 };
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (val1 ^ val2) & (val1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if val2 > val1 { 1 } else { 0 };
        sreg |= if 1 & ((!val1 & val2) | (val2 & r) | (r & !val1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xf000) == 0x5000 {
        /* SUBI */
        let d = ((op & 0xf0) >> 4) + 16;
        let val1 = data(cpu, d);
        let val2 = (op & 0xf) | ((op & 0xf00) >> 4);
        let r = val1 - val2;
        set_data(cpu, d, r);
        let mut sreg = cpu.sreg() as i64 & 0xc0;
        sreg |= if r != 0 { 0 } else { 2 };
        sreg |= if 128 & r != 0 { 4 } else { 0 };
        sreg |= if (val1 ^ val2) & (val1 ^ r) & 128 != 0 {
            8
        } else {
            0
        };
        sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
            0x10
        } else {
            0
        };
        sreg |= if val2 > val1 { 1 } else { 0 };
        sreg |= if 1 & ((!val1 & val2) | (val2 & r) | (r & !val1)) != 0 {
            0x20
        } else {
            0
        };
        cpu.set_sreg(sreg);
    } else if (op & 0xfe0f) == 0x9402 {
        /* SWAP */
        let d = rd(op);
        let i = data(cpu, d);
        set_data(cpu, d, ((15 & i) << 4) | ((240 & i) >> 4));
    } else if op == 0x95a8 {
        /* WDR: callback invoked by the caller */
    } else if (op & 0xfe0f) == 0x9204 {
        /* XCH */
        let r = rd(op);
        let val1 = data(cpu, r);
        let address = cpu.get_u16(30) as usize;
        let val2 = data(cpu, address as i64);
        set_data(cpu, address as i64, val1);
        set_data(cpu, r, val2);
    }

    let length = cpu.prog_len() as i64;
    cpu.pc = if length == 0 {
        0
    } else {
        (cpu.pc + 1) % length
    };
    cpu.cycles += 1;
}

/// Shared logical-op SREG update (Z, N, V=0, S).
fn logical_flags(cpu: &mut Cpu, result: i64, preserved: i64) {
    let mut sreg = cpu.sreg() as i64 & preserved;
    sreg |= if result != 0 { 0 } else { 2 };
    sreg |= if 128 & result != 0 { 4 } else { 0 };
    sreg |= if ((sreg >> 2) & 1) ^ ((sreg >> 3) & 1) != 0 {
        0x10
    } else {
        0
    };
    cpu.set_sreg(sreg);
}

fn skip_size(cpu: &Cpu, index: i64) -> i64 {
    let next = cpu.prog_word(index) as i64;
    if is_two_word_instruction(next) {
        2
    } else {
        1
    }
}
