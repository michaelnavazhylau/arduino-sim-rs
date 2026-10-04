// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native SPI byte transfers and source-compatible status/interrupt behavior.
use super::{
    cpu::Cpu,
    peripheral::{config, number, Peripheral, PeripheralIo, State},
};
use crate::runtime::Value;
pub(super) fn default_config() -> Value {
    config(&[
        ("spiInterrupt", 0x22),
        ("SPCR", 0x4c),
        ("SPSR", 0x4d),
        ("SPDR", 0x4e),
    ])
}
impl Peripheral {
    fn spi_interrupt(&self) -> super::cpu::InterruptConfig {
        self.interrupt("spiInterrupt", "SPSR", 0x80, "SPCR", 0x80)
    }
    fn spi_divider(&self, cpu: &Cpu) -> i64 {
        let base = if cpu.data[self.reg("SPSR")] & 1 != 0 {
            2
        } else {
            4
        };
        base * [1, 4, 16, 32][(cpu.data[self.reg("SPCR")] & 3) as usize]
    }
    pub fn spi_get(&self, cpu: &Cpu, name: &str) -> Value {
        let spcr = cpu.data[self.reg("SPCR")];
        match name {
            "isMaster" => Value::Bool(spcr & 0x10 != 0),
            "dataOrder" => Value::Text(
                if spcr & 0x20 != 0 {
                    "lsbFirst"
                } else {
                    "msbFirst"
                }
                .into(),
            ),
            "spiMode" => Value::Number(
                ((if spcr & 4 != 0 { 2 } else { 0 }) | (if spcr & 8 != 0 { 1 } else { 0 })) as f64,
            ),
            "clockDivider" => Value::Number(self.spi_divider(cpu) as f64),
            "transferCycles" => Value::Number((self.spi_divider(cpu) * 8) as f64),
            "spiFrequency" => Value::Number(self.freq / self.spi_divider(cpu) as f64),
            "transmissionActive" => Value::Bool(matches!(&self.state, State::Spi { active: true })),
            "onByte" | "onTransfer" | "completeTransfer" | "reset" => self.callback(name),
            _ => panic!("unimplemented SPI property: {name}"),
        }
    }
    pub fn spi_write(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        addr: usize,
        value: u8,
    ) -> bool {
        let interrupt = self.spi_interrupt();
        if addr == self.reg("SPDR") {
            if cpu.data[self.reg("SPCR")] & 0x40 == 0 {
                return false;
            }
            let spsr = self.reg("SPSR");
            if matches!(&self.state, State::Spi { active: true }) {
                cpu.data[spsr] |= 0x40;
                return true;
            }
            cpu.data[spsr] &= !0x40;
            cpu.clear_interrupt(&interrupt, true);
            if let State::Spi { active } = &mut self.state {
                *active = true;
            }
            self.invoke(cpu, io, "onByte", vec![Value::Number(value as f64)]);
            return true;
        }
        if addr == self.reg("SPCR") {
            cpu.update_interrupt_enable(&interrupt, value);
        } else if addr == self.reg("SPSR") {
            cpu.data[addr] = value;
            cpu.clear_interrupt_by_flag(&interrupt, value);
        } else {
            panic!("unexpected SPI write address: {addr:#x}");
        }
        false // Preserve upstream hook fallthrough, including SPSR raw fallback.
    }
    pub fn spi_call(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: &[Value],
    ) -> Value {
        match name {
            "onTransfer" => return Value::Number(0.0),
            "onByte" => {
                let received = self.invoke(cpu, io, "onTransfer", args.to_vec());
                self.schedule(
                    cpu,
                    "transferComplete",
                    &[number(&received)],
                    self.spi_divider(cpu) * 8,
                );
            }
            "transferComplete" => {
                self.invoke(cpu, io, "completeTransfer", args.to_vec());
            }
            "completeTransfer" => {
                cpu.data[self.reg("SPDR")] = number(&args[0]) as u8;
                cpu.set_interrupt_flag(&self.spi_interrupt());
                if let State::Spi { active } = &mut self.state {
                    *active = false;
                }
            }
            "reset" => {
                if let State::Spi { active } = &mut self.state {
                    *active = false;
                }
            }
            _ => panic!("unimplemented SPI method: {name}"),
        }
        Value::Undefined
    }
}
