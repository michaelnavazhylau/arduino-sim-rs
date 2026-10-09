// SPDX-License-Identifier: MIT

//! Bus tests: a device attached to I2C or SPI must be reachable by the same
//! register-level master sequence real firmware produces.
//!
//! The master side is driven by writing `TWCR`/`TWDR` and `SPCR`/`SPDR`
//! directly, so no compiler is needed and the whole path is exercised: AVR
//! peripheral state machine -> host handler callback -> Rust device -> ACK bit
//! back into the AVR's status register.

use breadboard::{BreadboardHost, BusError, RegisterMap, Scene, SpiRegisterMap, TWCR, TWDR, TWSR};
use breadboard::{SPCR, SPDR};

/// Cycles to let a scheduled TWI phase or SPI transfer complete.
const SETTLE: u64 = 60;

const TWINT: u8 = 0x80;
const TWEA: u8 = 0x40;
const TWSTA: u8 = 0x20;
const TWSTO: u8 = 0x10;
const TWEN: u8 = 0x04;

fn host() -> BreadboardHost {
    BreadboardHost::new(breadboard_test_flash(), Scene::empty(), Vec::new()).expect("host boots")
}

/// A minimal halt loop, so the CPU never disturbs the registers under test.
fn breadboard_test_flash() -> Vec<u8> {
    use avr8rs::sim::assembler::assemble;
    let result = assemble("loop: JMP loop");
    assert!(result.errors.is_empty(), "assembler: {:?}", result.errors);
    let mut flash = vec![0xffu8; 0x8000];
    flash[..result.bytes.len()].copy_from_slice(&result.bytes);
    flash
}

fn twi_start(host: &mut BreadboardHost) {
    host.write_data(TWCR, TWINT | TWSTA | TWEN);
    host.advance_cycles(SETTLE).expect("start settles");
}

fn twi_stop(host: &mut BreadboardHost) {
    host.write_data(TWCR, TWINT | TWSTO | TWEN);
    host.advance_cycles(SETTLE).expect("stop settles");
}

fn twi_write(host: &mut BreadboardHost, byte: u8) {
    host.write_data(TWDR, byte);
    host.write_data(TWCR, TWINT | TWEN);
    host.advance_cycles(SETTLE).expect("write settles");
}

/// Read a byte and acknowledge it, asking the slave for another.
fn twi_read_ack(host: &mut BreadboardHost) -> u8 {
    host.write_data(TWCR, TWINT | TWEA | TWEN);
    host.advance_cycles(SETTLE).expect("read settles");
    host.board().read_data(host.backend(), TWDR)
}

/// Read the last byte, answering with NACK as a real master does.
fn twi_read_nack(host: &mut BreadboardHost) -> u8 {
    host.write_data(TWCR, TWINT | TWEN);
    host.advance_cycles(SETTLE).expect("read settles");
    host.board().read_data(host.backend(), TWDR)
}

fn twi_status(host: &BreadboardHost) -> u8 {
    host.board().read_data(host.backend(), TWSR) & 0xf8
}

/// Clock one byte through the SPI peripheral and return what came back.
fn spi_exchange(host: &mut BreadboardHost, byte: u8) -> u8 {
    host.write_data(SPDR, byte);
    host.advance_cycles(200).expect("transfer settles");
    host.board().read_data(host.backend(), SPDR)
}

/// Point the master at register `index` of `address` and read one byte back.
fn read_register(host: &mut BreadboardHost, address: u8, index: u8) -> u8 {
    twi_start(host);
    twi_write(host, address << 1);
    twi_write(host, index);
    twi_start(host);
    twi_write(host, (address << 1) | 1);
    let byte = twi_read_nack(host);
    twi_stop(host);
    byte
}

#[test]
fn an_i2c_device_is_addressed_written_and_read_over_the_wire() {
    let mut host = host();
    host.attach_i2c(vec![Box::new(RegisterMap::new(0x68, 8).unwrap())]);

    // Point at register 2 and write 0xAB.
    twi_start(&mut host);
    twi_write(&mut host, 0x68 << 1);
    assert_eq!(
        twi_status(&host),
        0x18,
        "address+write must be acknowledged"
    );
    twi_write(&mut host, 0x02);
    twi_write(&mut host, 0xab);
    twi_stop(&mut host);

    // Point at register 2 again, repeated START, read it back.
    twi_start(&mut host);
    twi_write(&mut host, 0x68 << 1);
    twi_write(&mut host, 0x02);
    twi_start(&mut host);
    twi_write(&mut host, (0x68 << 1) | 1);
    assert_eq!(twi_status(&host), 0x40, "address+read must be acknowledged");
    let byte = twi_read_nack(&mut host);
    twi_stop(&mut host);

    assert_eq!(byte, 0xab);
    let bridge = host.bridge();
    let device = bridge.i2c_device_as::<RegisterMap>(0x68).unwrap();
    assert_eq!(device.register(0x02), 0xab);
    assert_eq!(device.writes(), 1);
    assert_eq!(device.reads(), 1);
}

#[test]
fn an_unaddressed_device_nacks_and_never_sees_the_data() {
    let mut host = host();
    host.attach_i2c(vec![Box::new(RegisterMap::new(0x68, 8).unwrap())]);

    twi_start(&mut host);
    twi_write(&mut host, 0x69 << 1);
    assert_eq!(twi_status(&host), 0x20, "an unknown address must be NACKed");

    let bridge = host.bridge();
    let device = bridge.i2c_device_as::<RegisterMap>(0x68).unwrap();
    assert_eq!(device.connects(), 0);
    assert_eq!(device.writes(), 0);
}

#[test]
fn auto_increment_streams_consecutive_registers() {
    let mut host = host();
    host.attach_i2c(vec![Box::new(
        RegisterMap::from_registers(0x68, vec![0x11, 0x22, 0x33, 0x44]).unwrap(),
    )]);

    twi_start(&mut host);
    twi_write(&mut host, 0x68 << 1);
    twi_write(&mut host, 0x00);
    twi_start(&mut host);
    twi_write(&mut host, (0x68 << 1) | 1);
    let first = twi_read_ack(&mut host);
    let second = twi_read_ack(&mut host);
    let third = twi_read_nack(&mut host);
    twi_stop(&mut host);

    assert_eq!((first, second, third), (0x11, 0x22, 0x33));
}

#[test]
fn turning_off_auto_increment_keeps_the_pointer_on_one_register() {
    let mut host = host();
    host.attach_i2c(vec![Box::new(
        RegisterMap::from_registers(0x68, vec![0x01, 0x02, 0x03])
            .unwrap()
            .with_auto_increment(false),
    )]);

    twi_start(&mut host);
    twi_write(&mut host, 0x68 << 1);
    twi_write(&mut host, 0x01);
    twi_start(&mut host);
    twi_write(&mut host, (0x68 << 1) | 1);
    let first = twi_read_ack(&mut host);
    let second = twi_read_nack(&mut host);
    twi_stop(&mut host);

    assert_eq!((first, second), (0x02, 0x02));
}

#[test]
fn only_the_addressed_device_answers_on_a_shared_bus() {
    let mut host = host();
    host.attach_i2c(vec![
        Box::new(RegisterMap::from_registers(0x68, vec![9]).unwrap()),
        Box::new(RegisterMap::from_registers(0x76, vec![7]).unwrap()),
    ]);

    assert_eq!(read_register(&mut host, 0x76, 0x00), 7);

    let bridge = host.bridge();
    assert_eq!(
        bridge
            .i2c_device_as::<RegisterMap>(0x68)
            .unwrap()
            .connects(),
        0,
        "the other device must not have been addressed"
    );
    assert_eq!(
        bridge
            .i2c_device_as::<RegisterMap>(0x76)
            .unwrap()
            .connects(),
        2
    );
}

#[test]
fn a_host_can_refresh_a_sensor_register_before_the_master_reads_it() {
    // A 16-bit quantity split across two registers, as I2C sensors publish them.
    let mut host = host();
    host.attach_i2c(vec![Box::new(RegisterMap::new(0x29, 4).unwrap())]);
    let millimetres: u16 = 1234;
    {
        let mut bridge = host.bridge_mut();
        let device = bridge.i2c_device_as_mut::<RegisterMap>(0x29).unwrap();
        device.set_register(0, (millimetres >> 8) as u8);
        device.set_register(1, (millimetres & 0xff) as u8);
    }

    twi_start(&mut host);
    twi_write(&mut host, 0x29 << 1);
    twi_write(&mut host, 0x00);
    twi_start(&mut host);
    twi_write(&mut host, (0x29 << 1) | 1);
    let high = twi_read_ack(&mut host);
    let low = twi_read_nack(&mut host);
    twi_stop(&mut host);

    assert_eq!(u16::from_be_bytes([high, low]), millimetres);
}

#[test]
fn an_spi_device_writes_and_reads_registers_through_spdr() {
    let mut host = host();
    host.attach_spi(vec![Box::new(
        SpiRegisterMap::from_registers(vec![0; 8]).unwrap(),
    )])
    .expect("one device attaches");
    host.write_data(SPCR, 0x50); // SPE | MSTR

    // Command byte 0x04: write, starting at register 4.
    assert_eq!(spi_exchange(&mut host, 0x04), 0);
    assert_eq!(spi_exchange(&mut host, 0x5a), 0);
    host.bridge_mut()
        .spi_device_as_mut::<SpiRegisterMap>()
        .unwrap()
        .end_transaction();

    // Command byte 0x84: read, starting at register 4.
    assert_eq!(spi_exchange(&mut host, 0x84), 0);
    let value = spi_exchange(&mut host, 0x00);

    assert_eq!(value, 0x5a);
    let bridge = host.bridge();
    assert_eq!(
        bridge
            .spi_device_as::<SpiRegisterMap>()
            .unwrap()
            .register(4),
        0x5a
    );
}

#[test]
fn an_spi_read_streams_consecutive_registers() {
    let mut host = host();
    host.attach_spi(vec![Box::new(
        SpiRegisterMap::from_registers(vec![0xde, 0xad, 0xbe]).unwrap(),
    )])
    .unwrap();
    host.write_data(SPCR, 0x50);

    assert_eq!(spi_exchange(&mut host, 0x80), 0); // read from register 0
    assert_eq!(spi_exchange(&mut host, 0x00), 0xde);
    assert_eq!(spi_exchange(&mut host, 0x00), 0xad);
    assert_eq!(spi_exchange(&mut host, 0x00), 0xbe);
}

#[test]
fn an_invalid_bus_configuration_is_rejected_rather_than_guessed() {
    let mut host = host();
    assert_eq!(
        host.attach_spi(Vec::new()).unwrap_err(),
        BusError::WrongDeviceCount(0)
    );
    assert_eq!(
        host.attach_spi(vec![
            Box::new(SpiRegisterMap::new(4).unwrap()),
            Box::new(SpiRegisterMap::new(4).unwrap()),
        ])
        .unwrap_err(),
        BusError::WrongDeviceCount(2)
    );
    assert_eq!(
        RegisterMap::new(0x80, 4).unwrap_err(),
        BusError::InvalidAddress(0x80)
    );
    assert_eq!(
        RegisterMap::new(0x68, 0).unwrap_err(),
        BusError::EmptyRegisterMap
    );
    assert_eq!(
        SpiRegisterMap::new(0).unwrap_err(),
        BusError::EmptyRegisterMap
    );
    // A rejected SPI bus must leave the core's default handler inert: further
    // transfers still work rather than panicking on a half-installed bridge.
    host.write_data(SPCR, 0x50);
    assert_eq!(spi_exchange(&mut host, 0x00), 0);
}
