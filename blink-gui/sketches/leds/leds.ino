// Three indicator LEDs, one 330 ohm resistor each, on D9, D10 and D11.
//
// The resistors are deliberately equal. The LEDs are not: their forward
// voltages differ by roughly a volt from red to blue, so the currents differ
// too, and the same 330 ohm part is a different design point on each colour.
// That is what the overlay reports, so it is worth seeing rather than hiding.
//
// The sketch lights each LED alone first, then all three together, so both the
// individual operating points and the combined one are observable.
constexpr uint8_t LED_COUNT = 3;
constexpr uint8_t LED_PINS[LED_COUNT] = {9, 10, 11};

constexpr unsigned long SOLO_MS = 350;
constexpr unsigned long GAP_MS = 150;
constexpr unsigned long TOGETHER_MS = 1200;
constexpr unsigned long DARK_MS = 400;

static void all(uint8_t level) {
  for (uint8_t i = 0; i < LED_COUNT; i++) {
    digitalWrite(LED_PINS[i], level);
  }
}

void setup() {
  for (uint8_t i = 0; i < LED_COUNT; i++) {
    pinMode(LED_PINS[i], OUTPUT);
  }
  all(LOW);
}

void loop() {
  for (uint8_t i = 0; i < LED_COUNT; i++) {
    digitalWrite(LED_PINS[i], HIGH);
    delay(SOLO_MS);
    digitalWrite(LED_PINS[i], LOW);
    delay(GAP_MS);
  }

  all(HIGH);
  delay(TOGETHER_MS);

  all(LOW);
  delay(DARK_MS);
}
