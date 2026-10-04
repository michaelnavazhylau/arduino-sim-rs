// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native USART framing, timed TX/RX and synchronous host callbacks.
use super::{
    cpu::{Cpu, InterruptConfig},
    peripheral::{config, number, Peripheral, PeripheralIo, State},
};
use crate::runtime::Value;
pub(super) fn default_config() -> Value {
    config(&[
        ("rxCompleteInterrupt", 0x24),
        ("dataRegisterEmptyInterrupt", 0x26),
        ("txCompleteInterrupt", 0x28),
        ("UCSRA", 0xc0),
        ("UCSRB", 0xc1),
        ("UCSRC", 0xc2),
        ("UBRRL", 0xc4),
        ("UBRRH", 0xc5),
        ("UDR", 0xc6),
    ])
}
impl Peripheral {
    fn usart_interrupt(&self, name: &str, mask: u8) -> InterruptConfig {
        let mut int = self.interrupt(name, "UCSRA", mask, "UCSRB", mask);
        int.constant = mask == 0x80;
        int
    }
    fn usart_bits(&self, cpu: &Cpu) -> i64 {
        let size = ((cpu.data[self.reg("UCSRC")] & 6) >> 1) | (cpu.data[self.reg("UCSRB")] & 4);
        match size {
            0 => 5,
            1 => 6,
            2 => 7,
            3 => 8,
            _ => 9,
        }
    }
    fn usart_multiplier(&self, cpu: &Cpu) -> i64 {
        if cpu.data[self.reg("UCSRA")] & 2 != 0 {
            8
        } else {
            16
        }
    }
    fn usart_ubrr(&self, cpu: &Cpu) -> i64 {
        ((cpu.data[self.reg("UBRRH")] as i64) << 8) | cpu.data[self.reg("UBRRL")] as i64
    }
    fn usart_stop_bits(&self, cpu: &Cpu) -> i64 {
        if cpu.data[self.reg("UCSRC")] & 8 != 0 {
            2
        } else {
            1
        }
    }
    fn usart_cycles(&self, cpu: &Cpu) -> i64 {
        let parity = i64::from(cpu.data[self.reg("UCSRC")] & 0x20 != 0);
        (self.usart_ubrr(cpu) + 1)
            * self.usart_multiplier(cpu)
            * (1 + self.usart_bits(cpu) + self.usart_stop_bits(cpu) + parity)
    }
    pub fn usart_reset(&mut self, cpu: &mut Cpu) {
        cpu.data[self.reg("UCSRA")] = 0x20;
        cpu.data[self.reg("UCSRB")] = 0;
        cpu.data[self.reg("UCSRC")] = 6;
        if let State::Usart {
            busy,
            rx_byte,
            line,
        } = &mut self.state
        {
            *busy = false;
            *rx_byte = 0;
            line.clear();
        }
    }
    pub fn usart_get(&self, cpu: &Cpu, name: &str) -> Value {
        let State::Usart {
            busy,
            rx_byte,
            line,
        } = &self.state
        else {
            unreachable!()
        };
        match name {
            "rxBusy" | "rxBusyValue" => Value::Bool(*busy),
            "rxByte" => Value::Number(*rx_byte as f64),
            "lineBuffer" => Value::Text(line.clone()),
            "rxEnable" => Value::Bool(cpu.data[self.reg("UCSRB")] & 0x10 != 0),
            "txEnable" => Value::Bool(cpu.data[self.reg("UCSRB")] & 8 != 0),
            "baudRate" => Value::Number(
                (self.freq / (self.usart_multiplier(cpu) * (1 + self.usart_ubrr(cpu))) as f64)
                    .floor(),
            ),
            "bitsPerChar" => Value::Number(self.usart_bits(cpu) as f64),
            "stopBits" => Value::Number(self.usart_stop_bits(cpu) as f64),
            "parityEnabled" => Value::Bool(cpu.data[self.reg("UCSRC")] & 0x20 != 0),
            "parityOdd" => Value::Bool(cpu.data[self.reg("UCSRC")] & 0x10 != 0),
            "cyclesPerChar" => Value::Number(self.usart_cycles(cpu) as f64),
            "UBRR" => Value::Number(self.usart_ubrr(cpu) as f64),
            "multiplier" => Value::Number(self.usart_multiplier(cpu) as f64),
            "onByteTransmit" | "onLineTransmit" | "onRxComplete" | "onConfigurationChange" => {
                Value::Null
            }
            "writeByte" | "reset" => self.callback(name),
            _ => panic!("unimplemented USART property: {name}"),
        }
    }
    fn usart_optional(&mut self, cpu: &mut Cpu, io: &mut dyn PeripheralIo, name: &str) {
        if self
            .props
            .get(name)
            .is_some_and(|v| !matches!(v, Value::Null | Value::Undefined))
        {
            self.invoke(cpu, io, name, vec![]);
        }
    }
    pub fn usart_read(&mut self, cpu: &mut Cpu) -> u8 {
        let mask = match self.usart_bits(cpu) {
            5 => 0x1f,
            6 => 0x3f,
            7 => 0x7f,
            _ => 0xff,
        };
        let result = if let State::Usart { rx_byte, .. } = &mut self.state {
            let result = *rx_byte & mask;
            *rx_byte = 0;
            result as u8
        } else {
            unreachable!()
        };
        cpu.clear_interrupt(&self.usart_interrupt("rxCompleteInterrupt", 0x80), true);
        result
    }
    pub fn usart_write(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        addr: usize,
        value: u8,
    ) -> bool {
        let old = cpu.data[addr];
        if addr == self.reg("UCSRA") {
            cpu.data[addr] = value & 3;
            cpu.clear_interrupt_by_flag(&self.usart_interrupt("txCompleteInterrupt", 0x40), value);
            if (value & 2) != (old & 2) {
                self.usart_optional(cpu, io, "onConfigurationChange");
            }
            return true;
        }
        if addr == self.reg("UCSRB") {
            for (name, mask) in [
                ("rxCompleteInterrupt", 0x80),
                ("dataRegisterEmptyInterrupt", 0x20),
                ("txCompleteInterrupt", 0x40),
            ] {
                cpu.update_interrupt_enable(&self.usart_interrupt(name, mask), value);
            }
            if value & 0x10 != 0 && old & 0x10 != 0 {
                cpu.clear_interrupt(&self.usart_interrupt("rxCompleteInterrupt", 0x80), true);
            }
            if value & 8 != 0 && old & 8 == 0 {
                cpu.set_interrupt_flag(&self.usart_interrupt("dataRegisterEmptyInterrupt", 0x20));
            }
            cpu.data[addr] = value;
            if (value & 0x1c) != (old & 0x1c) {
                self.usart_optional(cpu, io, "onConfigurationChange");
            }
            return true;
        }
        if [self.reg("UCSRC"), self.reg("UBRRH"), self.reg("UBRRL")].contains(&addr) {
            cpu.data[addr] = value;
            self.usart_optional(cpu, io, "onConfigurationChange");
            return true;
        }
        assert_eq!(addr, self.reg("UDR"));
        if self.props.get("onByteTransmit").is_some_and(Value::truthy) {
            self.invoke(cpu, io, "onByteTransmit", vec![Value::Number(value as f64)]);
        }
        if self.props.get("onLineTransmit").is_some_and(Value::truthy) {
            if value == b'\n' {
                let line = match &self.state {
                    State::Usart { line, .. } => line.clone(),
                    _ => unreachable!(),
                };
                self.invoke(cpu, io, "onLineTransmit", vec![Value::Text(line)]);
                if let State::Usart { line, .. } = &mut self.state {
                    line.clear();
                }
            } else if let State::Usart { line, .. } = &mut self.state {
                line.push(char::from(value));
            }
        }
        self.schedule(cpu, "transmitComplete", &[], self.usart_cycles(cpu));
        cpu.clear_interrupt(&self.usart_interrupt("txCompleteInterrupt", 0x40), true);
        cpu.clear_interrupt(
            &self.usart_interrupt("dataRegisterEmptyInterrupt", 0x20),
            true,
        );
        false
    }
    pub fn usart_call(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: &[Value],
    ) -> Value {
        match name {
            "reset" => self.usart_reset(cpu),
            "writeByte" => {
                if matches!(&self.state, State::Usart { busy: true, .. })
                    || cpu.data[self.reg("UCSRB")] & 0x10 == 0
                {
                    return Value::Bool(false);
                }
                if args.get(1).is_some_and(Value::truthy) {
                    if let State::Usart { rx_byte, .. } = &mut self.state {
                        *rx_byte = number(&args[0]);
                    }
                    cpu.set_interrupt_flag(&self.usart_interrupt("rxCompleteInterrupt", 0x80));
                    self.usart_optional(cpu, io, "onRxComplete");
                } else {
                    if let State::Usart { busy, .. } = &mut self.state {
                        *busy = true;
                    }
                    self.schedule(
                        cpu,
                        "receiveComplete",
                        &[number(&args[0])],
                        self.usart_cycles(cpu),
                    );
                    return Value::Bool(true);
                }
            }
            "receiveComplete" => {
                if let State::Usart { busy, .. } = &mut self.state {
                    *busy = false;
                }
                self.invoke(
                    cpu,
                    io,
                    "writeByte",
                    vec![args[0].clone(), Value::Bool(true)],
                );
            }
            "transmitComplete" => {
                cpu.set_interrupt_flag(&self.usart_interrupt("dataRegisterEmptyInterrupt", 0x20));
                cpu.set_interrupt_flag(&self.usart_interrupt("txCompleteInterrupt", 0x40));
            }
            _ => panic!("unimplemented USART method: {name}"),
        }
        Value::Undefined
    }
}
