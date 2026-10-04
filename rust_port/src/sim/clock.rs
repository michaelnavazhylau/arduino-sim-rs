// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Native port of `avr8js/src/peripherals/clock.ts`.
use super::cpu::Cpu;
use crate::runtime::Handle;

const CLKPCE: i64 = 128;
pub const DEFAULT_CLKPR: usize = 0x61;

/// Reserved prescaler indices keep the scope-measured values upstream documents.
const PRESCALERS: [i64; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 2, 4, 8, 16, 32, 64, 128];

pub struct Clock {
    pub cpu: Handle,
    pub base_freq: f64,
    pub clkpr: usize,
    clock_enabled_cycles: i64,
    prescaler_value: f64,
    pub cycles_delta: f64,
}

impl Clock {
    pub fn new(cpu: Handle, base_freq: f64, clkpr: usize) -> Self {
        Self {
            cpu,
            base_freq,
            clkpr,
            clock_enabled_cycles: 0,
            prescaler_value: 1.0,
            cycles_delta: 0.0,
        }
    }

    /// CLKPR write hook: arm the four-cycle write window, then apply the divider.
    pub fn write_hook(&mut self, cpu: &mut Cpu, value: i64) {
        if (self.clock_enabled_cycles == 0 || self.clock_enabled_cycles < cpu.cycles)
            && value == CLKPCE
        {
            self.clock_enabled_cycles = cpu.cycles + 4;
        } else if self.clock_enabled_cycles != 0 && self.clock_enabled_cycles >= cpu.cycles {
            self.clock_enabled_cycles = 0;
            let index = (value & 0xf) as usize;
            let old = self.prescaler_value;
            self.prescaler_value = PRESCALERS[index] as f64;
            cpu.data[self.clkpr] = index as u8;
            if old != self.prescaler_value {
                self.cycles_delta = (cpu.cycles as f64 + self.cycles_delta)
                    * (old / self.prescaler_value)
                    - cpu.cycles as f64;
            }
        }
    }

    pub fn frequency(&self) -> f64 {
        self.base_freq / self.prescaler_value
    }
    pub fn prescaler(&self) -> f64 {
        self.prescaler_value
    }
    pub fn time_nanos(&self, cycles: i64) -> f64 {
        ((cycles as f64 + self.cycles_delta) / self.frequency()) * 1e9
    }
    pub fn time_micros(&self, cycles: i64) -> f64 {
        ((cycles as f64 + self.cycles_delta) / self.frequency()) * 1e6
    }
    pub fn time_millis(&self, cycles: i64) -> f64 {
        ((cycles as f64 + self.cycles_delta) / self.frequency()) * 1e3
    }
}
