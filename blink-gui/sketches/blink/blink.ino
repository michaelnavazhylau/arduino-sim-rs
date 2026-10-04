// Minimal blink: PORTB5 (Arduino pin 13), 500 ms half-period.
void setup() { pinMode(13, OUTPUT); }
void loop() {
  digitalWrite(13, HIGH); delay(500);
  digitalWrite(13, LOW);  delay(500);
}
