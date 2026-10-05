// HC-SR04 ultrasonic ranging, reporting each measurement to the host over I2C.
//
// The simulated board has no serial console attached, so the firmware publishes
// its own timing result to an I2C "host sink" that the simulator implements.
// That matters: the host then displays what *the firmware measured*, not what
// the sensor model happened to produce.
//
// Wiring: TRIG = D9, ECHO = D10, proximity indicator = D13.
// Host sink address 0x68, register layout (auto-incrementing pointer):
//   0 status        0 = ok, 1 = no echo, 2 = out of range
//   1 sequence      wrapping measurement counter
//   2..3 distance   millimetres, big endian
//   4..5 echo       pulse width in microseconds, big endian
#include <Wire.h>

constexpr uint8_t TRIG_PIN = 9;
constexpr uint8_t ECHO_PIN = 10;
constexpr uint8_t NEAR_LED = 13;
constexpr uint8_t HOST_ADDR = 0x68;

// 30 ms covers the module's ~4 m range plus its response delay.
constexpr unsigned long ECHO_TIMEOUT_US = 30000UL;
constexpr uint16_t NEAR_MM = 200;
constexpr uint16_t MAX_RANGE_MM = 4000;

// Round-trip time to distance at 343 m/s: 0.343 mm/us, so mm = us * 343 / 2000.
// This is deliberately the firmware's own arithmetic, not a constant the host
// and firmware share.
static uint16_t millimetres(unsigned long echo_us) {
  unsigned long mm = (echo_us * 343UL) / 2000UL;
  return mm > 65535UL ? 65535UL : static_cast<uint16_t>(mm);
}

static uint8_t sequence = 0;

void setup() {
  pinMode(TRIG_PIN, OUTPUT);
  pinMode(ECHO_PIN, INPUT);
  pinMode(NEAR_LED, OUTPUT);
  digitalWrite(TRIG_PIN, LOW);
  Wire.begin();
}

void loop() {
  digitalWrite(TRIG_PIN, HIGH);
  delayMicroseconds(12);
  digitalWrite(TRIG_PIN, LOW);

  const unsigned long echo_us = pulseIn(ECHO_PIN, HIGH, ECHO_TIMEOUT_US);
  uint8_t status = 0;
  uint16_t mm = 0;
  if (echo_us == 0) {
    status = 1;  // nothing came back inside the timeout
  } else {
    mm = millimetres(echo_us);
    if (mm == 0 || mm > MAX_RANGE_MM) {
      status = 2;
    }
  }

  Wire.beginTransmission(HOST_ADDR);
  Wire.write(static_cast<uint8_t>(0));
  Wire.write(status);
  Wire.write(sequence++);
  Wire.write(static_cast<uint8_t>(mm >> 8));
  Wire.write(static_cast<uint8_t>(mm & 0xFF));
  Wire.write(static_cast<uint8_t>((echo_us >> 8) & 0xFF));
  Wire.write(static_cast<uint8_t>(echo_us & 0xFF));
  Wire.endTransmission();

  digitalWrite(NEAR_LED, (status == 0 && mm < NEAR_MM) ? HIGH : LOW);
  delay(60);
}
