// SPDX-License-Identifier: MIT
// Copyright (c) Uri Shaked and contributors

//! Source-compatible ADC mux/reference selection and conversion timing.
use super::{
    cpu::Cpu,
    peripheral::{config, fields, number, Peripheral, PeripheralIo, State},
};
use crate::runtime::Value;
use std::collections::BTreeMap;

pub(super) fn channels() -> Value {
    let mut channels = BTreeMap::new();
    for channel in 0..8 {
        channels.insert(
            channel.to_string(),
            config(&[("type", 0), ("channel", channel)]),
        );
    }
    channels.insert("8".into(), config(&[("type", 3)]));
    channels.insert(
        "14".into(),
        Value::object(BTreeMap::from([
            ("type".into(), Value::Number(2.0)),
            ("voltage".into(), Value::Number(1.1)),
        ])),
    );
    channels.insert("15".into(), config(&[("type", 2), ("voltage", 0)]));
    Value::object(channels)
}
pub(super) fn default_config() -> Value {
    let mut cfg = fields(&config(&[
        ("ADMUX", 0x7c),
        ("ADCSRA", 0x7a),
        ("ADCSRB", 0x7b),
        ("ADCL", 0x78),
        ("ADCH", 0x79),
        ("DIDR0", 0x7e),
        ("adcInterrupt", 0x2a),
        ("numChannels", 8),
        ("muxInputMask", 15),
    ]));
    cfg.insert("muxChannels".into(), channels());
    cfg.insert(
        "adcReferences".into(),
        Value::array(vec![
            Value::Number(1.0),
            Value::Number(0.0),
            Value::Number(4.0),
            Value::Number(2.0),
        ]),
    );
    Value::object(cfg)
}
impl Peripheral {
    fn adc_prescaler(&self, cpu: &Cpu) -> i64 {
        1 << (cpu.data[self.reg("ADCSRA")] & 7).max(1)
    }
    fn adc_sample_cycles(&self, cpu: &Cpu) -> i64 {
        match &self.state {
            State::Adc {
                conversion_cycles, ..
            } => conversion_cycles * self.adc_prescaler(cpu),
            _ => unreachable!(),
        }
    }
    fn adc_reference_type(&self, cpu: &Cpu) -> i64 {
        let refs = self.config["adcReferences"].values();
        let admux = cpu.data[self.reg("ADMUX")];
        let index = ((admux >> 6) & 3)
            | if refs.len() > 4 && admux & 8 != 0 {
                4
            } else {
                0
            };
        refs.get(index as usize).map_or(4, number)
    }
    fn adc_reference_voltage(&self, cpu: &Cpu) -> f64 {
        let State::Adc { avcc, aref, .. } = &self.state else {
            unreachable!()
        };
        match self.adc_reference_type(cpu) {
            1 => *aref,
            2 => 1.1,
            3 => 2.56,
            _ => *avcc,
        }
    }
    pub fn adc_get(&self, cpu: &Cpu, name: &str) -> Value {
        let State::Adc {
            channels,
            avcc,
            aref,
            converting,
            conversion_cycles,
        } = &self.state
        else {
            unreachable!()
        };
        match name {
            "channelValues" => channels.clone(),
            "avcc" => Value::Number(*avcc),
            "aref" => Value::Number(*aref),
            "prescaler" => Value::Number(self.adc_prescaler(cpu) as f64),
            "referenceVoltageType" => Value::Number(self.adc_reference_type(cpu) as f64),
            "referenceVoltage" => Value::Number(self.adc_reference_voltage(cpu)),
            "sampleCycles" => Value::Number(self.adc_sample_cycles(cpu) as f64),
            "converting" => Value::Bool(*converting),
            "conversionCycles" => Value::Number(*conversion_cycles as f64),
            "onADCRead" | "completeADCRead" => self.callback(name),
            _ => panic!("unimplemented ADC property: {name}"),
        }
    }
    pub fn adc_write(&mut self, cpu: &mut Cpu, io: &mut dyn PeripheralIo, value: u8) -> bool {
        let addr = self.reg("ADCSRA");
        if value & 0x80 != 0 && cpu.data[addr] & 0x80 == 0 {
            if let State::Adc {
                conversion_cycles, ..
            } = &mut self.state
            {
                *conversion_cycles = 25;
            }
        }
        cpu.data[addr] = value;
        cpu.update_interrupt_enable(
            &self.interrupt("adcInterrupt", "ADCSRA", 0x10, "ADCSRA", 8),
            value,
        );
        let converting = matches!(
            &self.state,
            State::Adc {
                converting: true,
                ..
            }
        );
        if !converting && value & 0x40 != 0 {
            if value & 0x80 == 0 {
                self.schedule(cpu, "sampleComplete", &[0], self.adc_sample_cycles(cpu));
                return true;
            }
            let mut channel = cpu.data[self.reg("ADMUX")] & 0x1f;
            if cpu.data[self.reg("ADCSRB")] & 8 != 0 {
                channel |= 0x20;
            }
            channel &= self.int("muxInputMask") as u8;
            let mux = fields(&self.config["muxChannels"])
                .get(&channel.to_string())
                .cloned()
                .unwrap_or_else(|| config(&[("type", 2), ("voltage", 0)]));
            if let State::Adc { converting, .. } = &mut self.state {
                *converting = true;
            }
            self.invoke(cpu, io, "onADCRead", vec![mux]);
            return true;
        }
        false
    }
    pub fn adc_call(
        &mut self,
        cpu: &mut Cpu,
        io: &mut dyn PeripheralIo,
        name: &str,
        args: &[Value],
    ) -> Value {
        match name {
            "onADCRead" => {
                let input = fields(&args[0]);
                let channels = match &self.state {
                    State::Adc { channels, .. } => channels.values(),
                    _ => unreachable!(),
                };
                let channel = |name: &str| -> f64 {
                    channels
                        .get(number(&input[name]) as usize)
                        .filter(|v| v.truthy())
                        .map_or(0.0, Value::number)
                };
                let voltage = match number(&input["type"]) {
                    0 => channel("channel"),
                    1 => {
                        input["gain"].number()
                            * (channel("positiveChannel") - channel("negativeChannel"))
                    }
                    2 => input["voltage"].number(),
                    3 => 0.378125,
                    _ => panic!("unknown ADC mux input type"),
                };
                let result = ((voltage / self.adc_reference_voltage(cpu)) * 1024.0)
                    .floor()
                    .clamp(0.0, 1023.0) as i64;
                self.schedule(
                    cpu,
                    "sampleComplete",
                    &[result],
                    self.adc_sample_cycles(cpu),
                );
            }
            "sampleComplete" => {
                self.invoke(cpu, io, "completeADCRead", args.to_vec());
            }
            "completeADCRead" => {
                if let State::Adc {
                    converting,
                    conversion_cycles,
                    ..
                } = &mut self.state
                {
                    *converting = false;
                    *conversion_cycles = 13;
                }
                let value = number(&args[0]);
                let adcl = self.reg("ADCL");
                let adch = self.reg("ADCH");
                let adcsra = self.reg("ADCSRA");
                if cpu.data[self.reg("ADMUX")] & 0x20 != 0 {
                    cpu.data[adcl] = (value << 6) as u8;
                    cpu.data[adch] = (value >> 2) as u8;
                } else {
                    cpu.data[adcl] = value as u8;
                    cpu.data[adch] = ((value >> 8) & 3) as u8;
                }
                cpu.data[adcsra] &= !0x40;
                cpu.set_interrupt_flag(&self.interrupt(
                    "adcInterrupt",
                    "ADCSRA",
                    0x10,
                    "ADCSRA",
                    8,
                ));
            }
            _ => panic!("unimplemented ADC method: {name}"),
        }
        Value::Undefined
    }
}
