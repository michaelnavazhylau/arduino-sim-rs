// The same button, wired the other way round, with an external pull-down.
//
// Wiring: D2 --- button --- 5V, plus a 10k resistor from D2 to GND.
//         D9 --- 330 ohm --- LED --- GND.
//
// The pin idles LOW through the pull-down and reads HIGH while the button is
// held down, so "pressed means lit" needs no inversion. The trade is an extra
// resistor, and 500 uA drawn whenever the button is held instead of 167 uA.
constexpr uint8_t BUTTON_PIN = 2;
constexpr uint8_t LED_PIN = 9;

void setup() {
  // Plain input: the external pull-down does the work, not the AVR.
  pinMode(BUTTON_PIN, INPUT);
  pinMode(LED_PIN, OUTPUT);
  digitalWrite(LED_PIN, LOW);
}

void loop() {
  const bool pressed = digitalRead(BUTTON_PIN) == HIGH;
  digitalWrite(LED_PIN, pressed ? HIGH : LOW);
  delay(10);
}
