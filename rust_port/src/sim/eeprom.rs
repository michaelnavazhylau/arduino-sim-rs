// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native EEPROM timing and erase/program behavior, ported from AVR8js.
use super::{
    cpu::Cpu,
    peripheral::{config, number, Peripheral, PeripheralIo, State},
};
use crate::runtime::Value;

pub(super) fn default_config() -> Value {
    config(&[
        ("eepromReadyInterrupt", 0x2c),
        ("EECR", 0x3f),
        ("EEDR", 0x40),
        ("EEARL", 0x41),
        ("EEARH", 0x42),
        ("eraseCycles", 28800),
        ("writeCycles", 28800),
    ])
}
impl Peripheral {
    fn eer(&self) -> super::cpu::InterruptConfig {
        let mut interrupt = self.interrupt("eepromReadyInterrupt", "EECR", 2, "EECR", 8);
        interrupt.constant = true;
        interrupt.inverse_flag = true;
        interrupt
    }
    pub fn eeprom_write(&mut self, cpu: &mut Cpu, io: &mut dyn PeripheralIo, value: u8) -> bool {
        let eecr = self.reg("EECR");
        let eedr = self.reg("EEDR");
        let address =
            ((cpu.data[self.reg("EEARH")] as i64) << 8) | cpu.data[self.reg("EEARL")] as i64;
        let interrupt = self.eer();
        cpu.data[eecr] = (cpu.data[eecr] & !0x3e) | (value & 0x3e);
        cpu.update_interrupt_enable(&interrupt, value);
        if value & 1 != 0 {
            cpu.clear_interrupt(&interrupt, true);
        }
        if value & 4 != 0 {
            if let State::Eeprom { enabled_until, .. } = &mut self.state {
                *enabled_until = cpu.cycles + 4;
            }
            self.schedule(cpu, "clearMasterEnable", &[], 4);
        }
        let (backend, enabled_until, complete_at) = match &self.state {
            State::Eeprom {
                backend,
                enabled_until,
                complete_at,
            } => (backend.clone(), *enabled_until, *complete_at),
            _ => unreachable!(),
        };
        if value & 1 != 0 {
            let result = io.invoke(
                self,
                cpu,
                Some(backend),
                "readMemory",
                vec![Value::Number(address as f64)],
            );
            cpu.data[eedr] = number(&result) as u8;
            cpu.cycles += 4;
            return true;
        }
        if value & 2 != 0 {
            if cpu.cycles >= enabled_until {
                cpu.data[eecr] &= !2;
                return true;
            }
            if cpu.cycles < complete_at {
                return true;
            }
            let byte = cpu.data[eedr];
            if let State::Eeprom { complete_at, .. } = &mut self.state {
                *complete_at = cpu.cycles;
            }
            if value & 0x20 == 0 {
                io.invoke(
                    self,
                    cpu,
                    Some(backend.clone()),
                    "eraseMemory",
                    vec![Value::Number(address as f64)],
                );
                let delay = self.int("eraseCycles");
                if let State::Eeprom { complete_at, .. } = &mut self.state {
                    *complete_at += delay;
                }
            }
            if value & 0x10 == 0 {
                io.invoke(
                    self,
                    cpu,
                    Some(backend),
                    "writeMemory",
                    vec![Value::Number(address as f64), Value::Number(byte as f64)],
                );
                let delay = self.int("writeCycles");
                if let State::Eeprom { complete_at, .. } = &mut self.state {
                    *complete_at += delay;
                }
            }
            cpu.data[eecr] |= 2;
            let complete_at = match &self.state {
                State::Eeprom { complete_at, .. } => *complete_at,
                _ => unreachable!(),
            };
            self.schedule(cpu, "writeComplete", &[], complete_at - cpu.cycles);
            cpu.cycles += 2;
        }
        true
    }
    pub fn eeprom_call(&mut self, cpu: &mut Cpu, name: &str) -> Value {
        match name {
            "clearMasterEnable" => cpu.data[self.reg("EECR")] &= !4,
            "writeComplete" => cpu.set_interrupt_flag(&self.eer()),
            _ => panic!("unimplemented EEPROM method: {name}"),
        }
        Value::Undefined
    }
}
