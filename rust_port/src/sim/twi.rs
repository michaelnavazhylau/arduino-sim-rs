// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native TWI master-state dispatch. Slave states remain absent, as upstream.
use super::{
    cpu::Cpu,
    peripheral::{config, number, Peripheral, PeripheralIo, State},
};
use crate::runtime::{Handle, Value};
use std::collections::BTreeMap;
pub(super) fn default_config() -> Value {
    config(&[
        ("twiInterrupt", 0x30),
        ("TWBR", 0xb8),
        ("TWSR", 0xb9),
        ("TWAR", 0xba),
        ("TWDR", 0xbb),
        ("TWCR", 0xbc),
        ("TWAMR", 0xbd),
    ])
}
pub(super) fn noop_handler(handle: Handle) -> Value {
    Value::object(BTreeMap::from_iter(
        [
            ("start", "noopStart"),
            ("stop", "noopStop"),
            ("connectToSlave", "noopConnect"),
            ("writeByte", "noopWrite"),
            ("readByte", "noopRead"),
        ]
        .map(|(method, native)| {
            (
                method.into(),
                Value::Native(format!("peripheral:{}:{native}", handle.0)),
            )
        }),
    ))
}
impl Peripheral {
    fn twi_interrupt(&self) -> super::cpu::InterruptConfig {
        self.interrupt("twiInterrupt", "TWCR", 0x80, "TWCR", 1)
    }
    fn twi_status(&self, cpu: &Cpu) -> u8 {
        cpu.data[self.reg("TWSR")] & 0xf8
    }
    pub fn twi_update_status(&mut self, cpu: &mut Cpu, value: u8) {
        let addr = self.reg("TWSR");
        cpu.data[addr] = (cpu.data[addr] & !0xf8) | value;
        cpu.set_interrupt_flag(&self.twi_interrupt());
    }
    pub fn twi_get(&self, cpu: &Cpu, name: &str) -> Value {
        let prescaler = 1 << (2 * (cpu.data[self.reg("TWSR")] & 3));
        match name {
            "prescaler" => Value::Number(prescaler as f64),
            "sclFrequency" => Value::Number(
                self.freq / (16.0 + 2.0 * cpu.data[self.reg("TWBR")] as f64 * prescaler as f64),
            ),
            "status" => Value::Number(self.twi_status(cpu) as f64),
            "busy" => Value::Bool(matches!(&self.state, State::Twi { busy: true })),
            "completeStart" | "completeStop" | "completeConnect" | "completeWrite"
            | "completeRead" => self.callback(name),
            _ => panic!("unimplemented TWI property: {name}"),
        }
    }
    pub fn twi_write(&mut self, cpu: &mut Cpu, value: u8) -> bool {
        cpu.data[self.reg("TWCR")] = value;
        let interrupt = self.twi_interrupt();
        cpu.clear_interrupt_by_flag(&interrupt, value);
        cpu.update_interrupt_enable(&interrupt, value);
        let status = self.twi_status(cpu);
        if value & 0x80 != 0 && value & 4 != 0 && !matches!(&self.state, State::Twi { busy: true })
        {
            self.schedule(
                cpu,
                "controlEvent",
                &[
                    value as i64,
                    status as i64,
                    cpu.data[self.reg("TWDR")] as i64,
                ],
                0,
            );
            return true;
        }
        false
    }
    pub fn twi_call(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: &[Value],
    ) -> Value {
        match name {
            "controlEvent" => {
                let value = number(&args[0]) as u8;
                let status = number(&args[1]) as u8;
                let byte = number(&args[2]) as u8;
                let (method, args) = if value & 0x20 != 0 {
                    ("start", vec![Value::Bool(status != 0xf8)])
                } else if value & 0x10 != 0 {
                    ("stop", vec![])
                } else if status == 8 || status == 0x10 {
                    (
                        "connectToSlave",
                        vec![
                            Value::Number((byte >> 1) as f64),
                            Value::Bool(byte & 1 == 0),
                        ],
                    )
                } else if status == 0x18 || status == 0x28 {
                    ("writeByte", vec![Value::Number(byte as f64)])
                } else if status == 0x40 || status == 0x50 {
                    ("readByte", vec![Value::Bool(value & 0x40 != 0)])
                } else {
                    return Value::Undefined;
                };
                if let State::Twi { busy } = &mut self.state {
                    *busy = true;
                }
                let handler = self.props["eventHandler"].clone();
                io.invoke(self, cpu, Some(handler), method, args);
            }
            "noopStart" => {
                self.invoke(cpu, io, "completeStart", vec![]);
            }
            "noopStop" => {
                self.invoke(cpu, io, "completeStop", vec![]);
            }
            "noopConnect" => {
                self.invoke(cpu, io, "completeConnect", vec![Value::Bool(false)]);
            }
            "noopWrite" => {
                self.invoke(cpu, io, "completeWrite", vec![Value::Bool(false)]);
            }
            "noopRead" => {
                self.invoke(cpu, io, "completeRead", vec![Value::Number(255.0)]);
            }
            "completeStart" | "completeStop" | "completeConnect" | "completeWrite"
            | "completeRead" => {
                if let State::Twi { busy } = &mut self.state {
                    *busy = false;
                }
                let status = match name {
                    "completeStart" => {
                        if self.twi_status(cpu) == 0xf8 {
                            8
                        } else {
                            0x10
                        }
                    }
                    "completeStop" => {
                        cpu.data[self.reg("TWCR")] &= !0x10;
                        0xf8
                    }
                    "completeConnect" => {
                        match (cpu.data[self.reg("TWDR")] & 1 != 0, args[0].truthy()) {
                            (true, true) => 0x40,
                            (true, false) => 0x48,
                            (false, true) => 0x18,
                            (false, false) => 0x20,
                        }
                    }
                    "completeWrite" => {
                        if args[0].truthy() {
                            0x28
                        } else {
                            0x30
                        }
                    }
                    "completeRead" => {
                        cpu.data[self.reg("TWDR")] = number(&args[0]) as u8;
                        if cpu.data[self.reg("TWCR")] & 0x40 != 0 {
                            0x50
                        } else {
                            0x58
                        }
                    }
                    _ => unreachable!(),
                };
                self.twi_update_status(cpu, status);
            }
            _ => panic!("unimplemented TWI method: {name}"),
        }
        Value::Undefined
    }
}
