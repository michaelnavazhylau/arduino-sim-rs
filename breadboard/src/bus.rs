// SPDX-License-Identifier: MIT

//! External devices on the AVR's I2C and SPI buses.
//!
//! The AVR core drives its TWI and SPI peripherals, but a bus with nothing on it
//! is not a bus. This module attaches Rust devices to those peripherals through
//! the core's own callback surface, so a sensor is addressed, read and written
//! by the same code path real firmware uses.
//!
//! # How attachment works
//!
//! `AVRTWI` dispatches each master phase to its `eventHandler` object and waits
//! for the handler to complete the phase; `AVRSPI` calls `onTransfer` for each
//! byte. Both fetch their callbacks as [`Value::Native`] identities, which the
//! core resolves through [`Backend::call`]. [`BridgeBackend`] is a decorator
//! that intercepts `breadboard:` identities and routes them to the devices
//! registered in a [`Bridge`], delegating everything else untouched.
//!
//! Nothing here reimplements AVR behaviour: the core still owns status codes,
//! interrupts and timing. The bridge only decides what answers on the bus.
//!
//! # Scope
//!
//! * I2C is modelled as a **master** bus with addressable slaves, matching the
//!   core's master-only TWI state machine. There is no multi-master arbitration
//!   and no clock stretching.
//! * SPI has no chip-select: the core does not model `SS`, so a slave sees one
//!   continuous session. A device needing framing must use a GPIO as CS.
//! * Devices are attached in code. They are not netlist parts.

use avr_port_tests::runtime::Handle;
use avr_port_tests::{Backend, Runtime, Value};
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

/// Data-space address of `TWDR` (data register), for a host driving the master.
pub const TWDR: usize = 0xbb;
/// Data-space address of `TWCR` (control register).
pub const TWCR: usize = 0xbc;
/// Data-space address of `TWSR` (status register).
pub const TWSR: usize = 0xb9;
/// Data-space address of `SPCR` (SPI control register).
pub const SPCR: usize = 0x4c;
/// Data-space address of `SPSR` (SPI status register).
pub const SPSR: usize = 0x4d;
/// Data-space address of `SPDR` (SPI data register).
pub const SPDR: usize = 0x4e;

/// Why a bus device could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    /// An I2C address must fit in the 7 bits the master puts on the wire.
    InvalidAddress(u8),
    /// A register map needs at least one register.
    EmptyRegisterMap,
    /// An SPI transfer handler needs exactly one device.
    WrongDeviceCount(usize),
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress(address) => {
                write!(f, "bus: I2C address {address:#04x} is not 7 bits")
            }
            Self::EmptyRegisterMap => write!(f, "bus: a register map needs at least one register"),
            Self::WrongDeviceCount(count) => {
                write!(
                    f,
                    "bus: expected exactly one SPI device, configured {count}"
                )
            }
        }
    }
}

impl Error for BusError {}

/// A device addressed on the I2C bus.
///
/// Methods are called in bus order and their return values become the ACK bits
/// the master observes, so a device can refuse to answer rather than only
/// answering with the wrong data.
pub trait I2cSlave {
    /// The 7-bit address this device answers to.
    fn address(&self) -> u8;

    /// A START or repeated START was seen. The device is not yet addressed.
    fn start(&mut self, _repeated: bool) {}

    /// The master addressed this device. Return `true` to acknowledge.
    fn connect(&mut self, write: bool) -> bool;

    /// The master wrote a byte. Return `true` to acknowledge.
    fn write(&mut self, byte: u8) -> bool;

    /// The master is reading: return the next byte.
    fn read(&mut self) -> u8;

    /// A STOP was seen.
    fn stop(&mut self) {}

    /// Downcast hook so a host can recover concrete device state.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable downcast hook.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// A device on the SPI bus.
///
/// There is no chip-select in the core's SPI model, so this is called for every
/// transferred byte of one continuous session.
pub trait SpiSlave {
    /// Exchange one byte: `mosi` is sent to the device, the result is returned
    /// to the master as MISO.
    fn transfer(&mut self, mosi: u8) -> u8;

    /// Downcast hook so a host can recover concrete device state.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable downcast hook.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// A device with a register file, a pointer register and auto-increment.
///
/// This is the shape of most I2C sensors: a write transaction sets the pointer,
/// then a read transaction streams registers from it.
#[derive(Debug)]
pub struct RegisterMap {
    address: u8,
    registers: Vec<u8>,
    pointer: u8,
    auto_increment: bool,
    writing: bool,
    pointer_loaded: bool,
    reads: u64,
    writes: u64,
    connects: u64,
}

impl RegisterMap {
    /// A device at `address` with `size` zero-initialized registers.
    pub fn new(address: u8, size: usize) -> Result<Self, BusError> {
        Self::from_registers(address, vec![0; size])
    }

    /// A device at `address` initialized with `registers`.
    pub fn from_registers(address: u8, registers: Vec<u8>) -> Result<Self, BusError> {
        if address > 0x7f {
            return Err(BusError::InvalidAddress(address));
        }
        if registers.is_empty() {
            return Err(BusError::EmptyRegisterMap);
        }
        Ok(Self {
            address,
            registers,
            pointer: 0,
            auto_increment: true,
            writing: false,
            pointer_loaded: false,
            reads: 0,
            writes: 0,
            connects: 0,
        })
    }

    /// Whether the pointer advances after each byte. Defaults to `true`.
    pub fn with_auto_increment(mut self, auto_increment: bool) -> Self {
        self.auto_increment = auto_increment;
        self
    }

    /// Set a register from the host, as a sensor refresh would.
    pub fn set_register(&mut self, index: u8, value: u8) {
        if let Some(slot) = self.registers.get_mut(index as usize) {
            *slot = value;
        }
    }

    /// Read a register from the host.
    pub fn register(&self, index: u8) -> u8 {
        self.registers.get(index as usize).copied().unwrap_or(0xff)
    }

    /// Every register, for host inspection.
    pub fn registers(&self) -> &[u8] {
        &self.registers
    }

    /// Current register pointer.
    pub fn pointer(&self) -> u8 {
        self.pointer
    }

    /// Bytes the master has read.
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// Bytes the master has written.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Times the master addressed this device.
    pub fn connects(&self) -> u64 {
        self.connects
    }
}

impl I2cSlave for RegisterMap {
    fn address(&self) -> u8 {
        self.address
    }

    fn connect(&mut self, write: bool) -> bool {
        self.writing = write;
        // The first byte of a write transaction is the pointer, not data.
        self.pointer_loaded = false;
        self.connects += 1;
        true
    }

    fn write(&mut self, byte: u8) -> bool {
        if !self.pointer_loaded {
            self.pointer = byte;
            self.pointer_loaded = true;
            return true;
        }
        let index = self.pointer as usize;
        if let Some(slot) = self.registers.get_mut(index) {
            *slot = byte;
            self.writes += 1;
        }
        if self.auto_increment {
            self.pointer = self.pointer.wrapping_add(1);
        }
        true
    }

    fn read(&mut self) -> u8 {
        let index = self.pointer as usize;
        let value = self.registers.get(index).copied().unwrap_or(0xff);
        self.reads += 1;
        if self.auto_increment {
            self.pointer = self.pointer.wrapping_add(1);
        }
        value
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// An SPI device with a command/register protocol.
///
/// The first byte of a session is a command: bit 7 selects read or write and
/// bits 6-0 are the starting register. Later bytes stream data, advancing the
/// pointer.
#[derive(Debug)]
pub struct SpiRegisterMap {
    registers: Vec<u8>,
    pointer: u8,
    reading: bool,
    commanded: bool,
    transfers: u64,
}

impl SpiRegisterMap {
    /// A device with `size` zero-initialized registers.
    pub fn new(size: usize) -> Result<Self, BusError> {
        Self::from_registers(vec![0; size])
    }

    /// A device initialized with `registers`.
    pub fn from_registers(registers: Vec<u8>) -> Result<Self, BusError> {
        if registers.is_empty() {
            return Err(BusError::EmptyRegisterMap);
        }
        Ok(Self {
            registers,
            pointer: 0,
            reading: false,
            commanded: false,
            transfers: 0,
        })
    }

    /// Set a register from the host.
    pub fn set_register(&mut self, index: u8, value: u8) {
        if let Some(slot) = self.registers.get_mut(index as usize) {
            *slot = value;
        }
    }

    /// Read a register from the host.
    pub fn register(&self, index: u8) -> u8 {
        self.registers.get(index as usize).copied().unwrap_or(0xff)
    }

    /// Current register pointer.
    pub fn pointer(&self) -> u8 {
        self.pointer
    }

    /// Bytes exchanged so far.
    pub fn transfers(&self) -> u64 {
        self.transfers
    }

    /// Delimit a transaction, standing in for chip-select.
    ///
    /// The core's SPI model has no `SS`, so a slave cannot see where one
    /// transaction ends. A host that selects the device with a GPIO should call
    /// this when that pin is released; without it the device treats every byte
    /// as part of one continuous session.
    pub fn end_transaction(&mut self) {
        self.commanded = false;
    }
}

impl SpiSlave for SpiRegisterMap {
    fn transfer(&mut self, mosi: u8) -> u8 {
        self.transfers += 1;
        if !self.commanded {
            self.commanded = true;
            self.reading = mosi & 0x80 != 0;
            self.pointer = mosi & 0x7f;
            return 0;
        }
        let index = self.pointer as usize;
        let response = if self.reading {
            self.registers.get(index).copied().unwrap_or(0xff)
        } else {
            0
        };
        if self.reading {
            self.pointer = self.pointer.wrapping_add(1);
        } else if let Some(slot) = self.registers.get_mut(index) {
            *slot = mosi;
            self.pointer = self.pointer.wrapping_add(1);
        }
        response
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct TwiBus {
    peripheral: Handle,
    slaves: Vec<Box<dyn I2cSlave>>,
    addressed: Option<usize>,
}

struct SpiBus {
    slaves: Vec<Box<dyn SpiSlave>>,
}

/// Devices attached to the simulated buses.
#[derive(Default)]
pub struct Bridge {
    twi: Option<TwiBus>,
    spi: Option<SpiBus>,
}

impl Bridge {
    /// No devices attached.
    pub fn new() -> Self {
        Self::default()
    }

    /// Route the I2C bus on `peripheral` to `slaves`.
    pub fn attach_i2c(&mut self, peripheral: Handle, slaves: Vec<Box<dyn I2cSlave>>) {
        self.twi = Some(TwiBus {
            peripheral,
            slaves,
            addressed: None,
        });
    }

    /// Route the SPI bus to `slaves`, which must number exactly one.
    pub fn attach_spi(&mut self, slaves: Vec<Box<dyn SpiSlave>>) -> Result<(), BusError> {
        if slaves.len() != 1 {
            return Err(BusError::WrongDeviceCount(slaves.len()));
        }
        self.spi = Some(SpiBus { slaves });
        Ok(())
    }

    /// The I2C device at `address`, for host inspection.
    pub fn i2c_device(&self, address: u8) -> Option<&dyn I2cSlave> {
        let bus = self.twi.as_ref()?;
        bus.slaves
            .iter()
            .find(|slave| slave.address() == address)
            .map(|slave| slave.as_ref())
    }

    /// A mutable handle to an I2C device, for host-driven register refreshes.
    pub fn i2c_device_mut(&mut self, address: u8) -> Option<&mut (dyn I2cSlave + 'static)> {
        let bus = self.twi.as_mut()?;
        bus.slaves
            .iter_mut()
            .find(|slave| slave.address() == address)
            .map(|slave| slave.as_mut())
    }

    /// A device's concrete state, recovered by type.
    pub fn i2c_device_as<T: Any>(&self, address: u8) -> Option<&T> {
        self.i2c_device(address)?.as_any().downcast_ref::<T>()
    }

    /// Mutable concrete device state, recovered by type.
    pub fn i2c_device_as_mut<T: Any>(&mut self, address: u8) -> Option<&mut T> {
        self.i2c_device_mut(address)?
            .as_any_mut()
            .downcast_mut::<T>()
    }

    /// Number of attached I2C devices.
    pub fn i2c_device_count(&self) -> usize {
        self.twi.as_ref().map_or(0, |bus| bus.slaves.len())
    }

    /// Number of attached SPI devices.
    pub fn spi_device_count(&self) -> usize {
        self.spi.as_ref().map_or(0, |bus| bus.slaves.len())
    }

    /// The SPI device's concrete state, recovered by type.
    pub fn spi_device_as<T: Any>(&self) -> Option<&T> {
        let bus = self.spi.as_ref()?;
        bus.slaves.first()?.as_any().downcast_ref::<T>()
    }

    /// Mutable concrete SPI device state, recovered by type.
    pub fn spi_device_as_mut<T: Any>(&mut self) -> Option<&mut T> {
        let bus = self.spi.as_mut()?;
        bus.slaves.first_mut()?.as_any_mut().downcast_mut::<T>()
    }

    /// Answer one intercepted bus callback.
    fn dispatch(
        &mut self,
        runtime: &mut Runtime,
        inner: &dyn Backend,
        method: &str,
        args: Vec<Value>,
    ) -> Value {
        match method {
            "twi:start" => {
                let repeated = args.first().is_some_and(Value::truthy);
                let Some(bus) = self.twi.as_mut() else {
                    return Value::Undefined;
                };
                for slave in &mut bus.slaves {
                    slave.start(repeated);
                }
                // A repeated START addresses a device again from scratch.
                bus.addressed = None;
                let peripheral = bus.peripheral;
                complete(inner, runtime, peripheral, "completeStart", Vec::new())
            }
            "twi:stop" => {
                let Some(bus) = self.twi.as_mut() else {
                    return Value::Undefined;
                };
                if let Some(index) = bus.addressed.take() {
                    bus.slaves[index].stop();
                }
                let peripheral = bus.peripheral;
                complete(inner, runtime, peripheral, "completeStop", Vec::new())
            }
            "twi:connect" => {
                let address = number_at(&args, 0) as u8;
                let write = args.get(1).is_some_and(Value::truthy);
                let Some(bus) = self.twi.as_mut() else {
                    return Value::Undefined;
                };
                // Only the addressed device acknowledges; anything else NACKs,
                // which is how firmware discovers that nobody is home.
                bus.addressed = bus
                    .slaves
                    .iter()
                    .position(|slave| slave.address() == address);
                let ack = match bus.addressed {
                    Some(index) => bus.slaves[index].connect(write),
                    None => false,
                };
                let peripheral = bus.peripheral;
                complete(
                    inner,
                    runtime,
                    peripheral,
                    "completeConnect",
                    vec![Value::Bool(ack)],
                )
            }
            "twi:write" => {
                let byte = number_at(&args, 0) as u8;
                let Some(bus) = self.twi.as_mut() else {
                    return Value::Undefined;
                };
                let ack = match bus.addressed {
                    Some(index) => bus.slaves[index].write(byte),
                    None => false,
                };
                let peripheral = bus.peripheral;
                complete(
                    inner,
                    runtime,
                    peripheral,
                    "completeWrite",
                    vec![Value::Bool(ack)],
                )
            }
            "twi:read" => {
                let Some(bus) = self.twi.as_mut() else {
                    return Value::Undefined;
                };
                let byte = match bus.addressed {
                    Some(index) => bus.slaves[index].read(),
                    None => 0xff,
                };
                let peripheral = bus.peripheral;
                complete(
                    inner,
                    runtime,
                    peripheral,
                    "completeRead",
                    vec![Value::Number(byte as f64)],
                )
            }
            "spi:transfer" => {
                let mosi = number_at(&args, 0) as u8;
                let Some(bus) = self.spi.as_mut() else {
                    return Value::Number(0.0);
                };
                let index = 0;
                let miso = bus
                    .slaves
                    .get_mut(index)
                    .map_or(0, |slave| slave.transfer(mosi));
                Value::Number(miso as f64)
            }
            _ => panic!("unknown breadboard bridge callback: {method}"),
        }
    }
}

/// Complete one TWI master phase on the peripheral.
fn complete(
    inner: &dyn Backend,
    runtime: &mut Runtime,
    peripheral: Handle,
    name: &str,
    args: Vec<Value>,
) -> Value {
    inner.call(
        runtime,
        None,
        &format!("peripheral:{}:{name}", peripheral.0),
        args,
    )
}

fn number_at(args: &[Value], index: usize) -> f64 {
    args.get(index).map_or(0.0, Value::number)
}

/// The handler object `AVRTWI` dispatches master phases to.
pub fn twi_handler() -> Value {
    Value::object(BTreeMap::from([
        (
            "start".to_string(),
            Value::Native("breadboard:twi:start".into()),
        ),
        (
            "stop".to_string(),
            Value::Native("breadboard:twi:stop".into()),
        ),
        (
            "connectToSlave".to_string(),
            Value::Native("breadboard:twi:connect".into()),
        ),
        (
            "writeByte".to_string(),
            Value::Native("breadboard:twi:write".into()),
        ),
        (
            "readByte".to_string(),
            Value::Native("breadboard:twi:read".into()),
        ),
    ]))
}

/// The callback identity `AVRSPI` uses to exchange a byte.
pub fn spi_transfer_handler() -> Value {
    Value::Native("breadboard:spi:transfer".into())
}

/// A [`Backend`] that routes `breadboard:` callbacks to a [`Bridge`].
///
/// Every other operation is delegated unchanged, so the AVR core's behaviour —
/// status codes, interrupts, timing and the parity contract — is untouched.
pub struct BridgeBackend {
    inner: Rc<dyn Backend>,
    bridge: Rc<RefCell<Bridge>>,
}

impl BridgeBackend {
    /// Wrap `inner`, routing `breadboard:` callbacks to `bridge`.
    pub fn new(inner: Rc<dyn Backend>, bridge: Rc<RefCell<Bridge>>) -> Self {
        Self { inner, bridge }
    }
}

impl Backend for BridgeBackend {
    fn resolve(&self, name: &str) -> Value {
        self.inner.resolve(name)
    }

    fn construct(&self, runtime: &mut Runtime, kind: &str, args: Vec<Value>) -> Value {
        self.inner.construct(runtime, kind, args)
    }

    fn get(&self, object: Handle, key: &str) -> Value {
        self.inner.get(object, key)
    }

    fn set(&self, runtime: &mut Runtime, object: Handle, key: &str, value: Value) {
        self.inner.set(runtime, object, key, value);
    }

    fn call(
        &self,
        runtime: &mut Runtime,
        receiver: Option<Handle>,
        name: &str,
        args: Vec<Value>,
    ) -> Value {
        if receiver.is_none() {
            if let Some(method) = name.strip_prefix("breadboard:") {
                // The borrow ends before the device completes its phase on the
                // peripheral, so a nested core callback cannot re-enter here.
                return self.bridge.borrow_mut().dispatch(
                    runtime,
                    self.inner.as_ref(),
                    method,
                    args,
                );
            }
        }
        self.inner.call(runtime, receiver, name, args)
    }

    fn properties(&self, object: Handle) -> BTreeMap<String, Value> {
        self.inner.properties(object)
    }
}
