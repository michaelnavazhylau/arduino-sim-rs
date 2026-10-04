// SPDX-License-Identifier: MIT
//
// Firmware used by tests/arduino_cli.rs to prove the native backend can execute
// genuine `arduino-cli` output for an ATmega328P board.
//
// Everything the Rust test observes is reported through address-stable
// locations, so the test never needs to parse ELF symbol tables:
//   GPIOR0 (data 0x3E) -> millis()/100, confirming AVR time advances
//   GPIOR1 (data 0x4A) -> 0x77 once Serial.flush() drains the TX ring
//   GPIOR2 (data 0x4B) -> 0x5A from the INT0 handler, proving interrupt dispatch
//   PORTB  (data 0x25) -> bit 5 tracks the 500 ms digitalWrite(13) boundary

#include <EEPROM.h>
#include <SPI.h>
#include <Wire.h>
#include <avr/io.h>
#include <avr/sleep.h>

static const uint8_t TX_DRAINED = 0x77;
static const uint8_t INT0_RAN = 0x5A;
static const uint8_t EEPROM_SEED = 42;

void onPinChange() { GPIOR2 = INT0_RAN; }

void setup() {
  Serial.begin(9600);
  pinMode(13, OUTPUT);
  pinMode(2, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(2), onPinChange, CHANGE);
  SPI.begin();
  Wire.begin();
  EEPROM.write(0, EEPROM_SEED);
  set_sleep_mode(SLEEP_MODE_IDLE);
}

void loop() {
  unsigned long now = millis();
  GPIOR0 = (uint8_t)(now / 100);
  int sample = analogRead(A0);        // blocks until ADSC clears
  digitalWrite(13, (now / 500) % 2);  // PORTB5 tracks each 500 ms boundary
  Serial.print(F("t="));
  Serial.print(now);
  Serial.print(' ');
  Serial.println(sample);
  Serial.flush();                     // drains only if the UDRE interrupt runs
  GPIOR1 = TX_DRAINED;
  EEPROM.update(1, (uint8_t)now);     // 3.4 ms write whenever the byte changes
  SPI.transfer(0x55);                 // blocks until SPIF is set
  delay(200);
}
