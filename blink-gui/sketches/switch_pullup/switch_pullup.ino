// A push button read with the AVR's internal pull-up, mirroring to an LED.
//
// Wiring: D2 --- button --- GND, and D9 --- 330 ohm --- LED --- GND.
//
// With the pull-up engaged the pin idles HIGH and reads LOW while the button is
// held down, so the sketch inverts the reading: pressed means LOW. This is the
// usual "active low" arrangement, and the reason it is usual is that it needs no
// external resistor at all.
constexpr uint8_t BUTTON_PIN = 2;
constexpr uint8_t LED_PIN = 9;

void setup() {
  pinMode(BUTTON_PIN, INPUT_PULLUP);
  pinMode(LED_PIN, OUTPUT);
  digitalWrite(LED_PIN, LOW);
}

void loop() {
  const bool pressed = digitalRead(BUTTON_PIN) == LOW;
  digitalWrite(LED_PIN, pressed ? HIGH : LOW);
  delay(10);
}
