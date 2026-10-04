// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native watchdog protection window, interrupt/reset modes and WDR handling.
use super::{
    cpu::Cpu,
    peripheral::{config, Peripheral, PeripheralIo, State},
};
use crate::runtime::Value;
pub(super) fn default_config() -> Value {
    config(&[
        ("watchdogInterrupt", 0x0c),
        ("MCUSR", 0x54),
        ("WDTCSR", 0x60),
    ])
}
impl Peripheral {
    fn watchdog_interrupt(&self) -> super::cpu::InterruptConfig {
        self.interrupt("watchdogInterrupt", "WDTCSR", 0x80, "WDTCSR", 0x40)
    }
    fn watchdog_prescaler(&self, cpu: &Cpu) -> i64 {
        let value = cpu.data[self.reg("WDTCSR")];
        2048 << (((value & 0x20) >> 2) | (value & 7))
    }
    pub fn watchdog_get(&self, cpu: &Cpu, name: &str) -> Value {
        let State::Watchdog {
            enabled,
            scheduled,
            change_until,
            timeout,
            ..
        } = &self.state
        else {
            unreachable!()
        };
        match name {
            "clockFrequency" => Value::Number(128000.0),
            "prescaler" => Value::Number(self.watchdog_prescaler(cpu) as f64),
            "enabled" | "enabledValue" => Value::Bool(*enabled),
            "scheduled" => Value::Bool(*scheduled),
            "watchdogTimeout" => Value::Number(*timeout as f64),
            "changeEnabledCycles" => Value::Number(*change_until as f64),
            "resetWatchdog" | "checkWatchdog" => self.callback(name),
            _ => panic!("unimplemented watchdog property: {name}"),
        }
    }
    fn watchdog_reset(&mut self, cpu: &Cpu, io: &dyn PeripheralIo) {
        let clock = match &self.state {
            State::Watchdog { clock, .. } => *clock,
            _ => unreachable!(),
        };
        let cycles = ((io.clock_frequency(clock) / 128000.0) * self.watchdog_prescaler(cpu) as f64)
            .floor() as i64;
        if let State::Watchdog { timeout, .. } = &mut self.state {
            *timeout = cpu.cycles + cycles;
        }
    }
    fn watchdog_enabled(&self) -> bool {
        matches!(&self.state, State::Watchdog { enabled: true, .. })
    }
    fn watchdog_timeout(&self) -> i64 {
        match &self.state {
            State::Watchdog { timeout, .. } => *timeout,
            _ => unreachable!(),
        }
    }
    pub fn watchdog_write(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        mut value: u8,
    ) -> bool {
        let addr = self.reg("WDTCSR");
        let old = cpu.data[addr];
        if value & 0x10 != 0 && value & 8 != 0 {
            if let State::Watchdog { change_until, .. } = &mut self.state {
                *change_until = cpu.cycles + 4;
            }
            value &= !0x2f;
        } else {
            let change_until = match &self.state {
                State::Watchdog { change_until, .. } => *change_until,
                _ => unreachable!(),
            };
            if cpu.cycles >= change_until {
                value = (value & !0x2f) | (old & 0x2f);
            }
            if let State::Watchdog { enabled, .. } = &mut self.state {
                *enabled = value & 0x48 != 0;
            }
            cpu.data[addr] = value;
        }
        if self.watchdog_enabled() {
            self.invoke(cpu, io, "resetWatchdog", vec![]);
        }
        if self.watchdog_enabled()
            && !matches!(
                &self.state,
                State::Watchdog {
                    scheduled: true,
                    ..
                }
            )
        {
            cpu.add_clock_event(
                self.callback("checkWatchdog"),
                self.watchdog_timeout() - cpu.cycles,
            );
        }
        cpu.clear_interrupt_by_flag(&self.watchdog_interrupt(), value);
        true
    }
    pub fn watchdog_call(&mut self, cpu: &mut Cpu, io: &mut dyn PeripheralIo, name: &str) -> Value {
        match name {
            "onWatchdogReset" => {
                self.invoke(cpu, io, "resetWatchdog", vec![]);
            }
            "resetWatchdog" => self.watchdog_reset(cpu, io),
            "checkWatchdog" => {
                if self.watchdog_enabled() && cpu.cycles >= self.watchdog_timeout() {
                    let addr = self.reg("WDTCSR");
                    let value = cpu.data[addr];
                    if value & 0x40 != 0 {
                        cpu.set_interrupt_flag(&self.watchdog_interrupt());
                    }
                    if value & 8 != 0 {
                        if value & 0x40 != 0 {
                            cpu.data[addr] &= !0x40;
                        } else {
                            io.invoke(self, cpu, Some(Value::Handle(self.cpu)), "reset", vec![]);
                            if let State::Watchdog { scheduled, .. } = &mut self.state {
                                *scheduled = false;
                            }
                            cpu.data[self.reg("MCUSR")] |= 8;
                            return Value::Undefined;
                        }
                    }
                    self.invoke(cpu, io, "resetWatchdog", vec![]);
                }
                if self.watchdog_enabled() {
                    if let State::Watchdog { scheduled, .. } = &mut self.state {
                        *scheduled = true;
                    }
                    cpu.add_clock_event(
                        self.callback("checkWatchdog"),
                        self.watchdog_timeout() - cpu.cycles,
                    );
                } else if let State::Watchdog { scheduled, .. } = &mut self.state {
                    *scheduled = false;
                }
            }
            _ => panic!("unimplemented watchdog method: {name}"),
        }
        Value::Undefined
    }
}
