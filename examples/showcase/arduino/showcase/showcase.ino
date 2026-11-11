// UWIN Studio showcase: drives every widget in showcase.uwin.
//
// Wiring: display TX -> board RX1, display RX -> board TX1, GND -> GND (5 V or 3.3 V
// TTL as your display is set). Default display baud: 115200, CRC off.
// Board: anything with a second hardware serial (Mega, Leonardo, ESP32, STM32, RP2040).
// On an Uno use SoftwareSerial (see below) at 115200 or lower the display baud.
//
// ui_gen.h comes from UWIN Studio's Build (copy the new one here after every rebuild).
// Test without the display: UWIN Studio > Simulate > pick this board's COM port.

#include "ui_gen.h"

#define DWIN Serial1
// Uno:  #include <SoftwareSerial.h>
//       SoftwareSerial dwinSerial(10, 11);  // RX, TX
//       #define DWIN dwinSerial

// Print every frame to the USB Serial Monitor (115200): "->" to the screen, "<-" from it.
// Set to 0 once everything works (printing slows the loop a little).
#define DEBUG_FRAMES 1

void printFrame(const char *dir, uint16_t vp, const uint8_t *data, uint8_t n) {
#if DEBUG_FRAMES
  Serial.print(dir);
  Serial.print(" VP 0x");
  Serial.print(vp, HEX);
  Serial.print(":");
  for (uint8_t i = 0; i < n && i < 24; i++) {
    Serial.print(' ');
    if (data[i] < 0x10) Serial.print('0');
    Serial.print(data[i], HEX);
  }
  Serial.println(n > 24 ? " ..." : "");
#endif
}

// ---------------------------------------------------------------- sending

// 5A A5 <len> 82 <VP hi> <VP lo> <data...>
void writeBytes(uint16_t vp, const uint8_t *data, uint8_t n) {
  DWIN.write(0x5A);
  DWIN.write(0xA5);
  DWIN.write(3 + n);
  DWIN.write(0x82);
  DWIN.write(vp >> 8);
  DWIN.write(vp & 0xFF);
  DWIN.write(data, n);
  // Live values sent 10x a second would flood the monitor; log everything else.
  if (vp != 0x0310 && vp != VP_PRESSURE && vp != VP_VOLTS && vp != VP_BATTERY) printFrame("->", vp, data, n);
}

void writeVP(uint16_t vp, uint16_t value) {
  uint8_t d[2] = {(uint8_t)(value >> 8), (uint8_t)value};
  writeBytes(vp, d, 2);
}

// ASCII text, ended with FF FF so the old text doesn't show behind it.
void writeText(uint16_t vp, const char *s) {
  uint8_t d[120];
  uint8_t n = 0;
  while (*s && n < sizeof(d) - 2) d[n++] = *s++;
  d[n++] = 0xFF;
  d[n++] = 0xFF;
  writeBytes(vp, d, n);
}

// Unicode text widget: UTF-8 in (as typed in the IDE), UTF-16BE out, ended with FF FF.
void writeUnicode(uint16_t vp, const char *utf8) {
  uint8_t d[120];
  uint8_t n = 0;
  const uint8_t *s = (const uint8_t *)utf8;
  while (*s && n < sizeof(d) - 4) {
    uint16_t c;
    if (*s < 0x80) c = *s++;
    else if ((*s & 0xE0) == 0xC0) { c = ((s[0] & 0x1F) << 6) | (s[1] & 0x3F); s += 2; }
    else { c = ((s[0] & 0x0F) << 12) | ((s[1] & 0x3F) << 6) | (s[2] & 0x3F); s += 3; }
    d[n++] = c >> 8;
    d[n++] = c & 0xFF;
  }
  d[n++] = 0xFF;
  d[n++] = 0xFF;
  writeBytes(vp, d, n);
}

void setPage(uint16_t page) {
  uint8_t d[4] = {0x5A, 0x01, (uint8_t)(page >> 8), (uint8_t)page};
  writeBytes(0x0084, d, 4);
}

// Buzzer, in 8 ms steps (T5L system register 0x00A0).
void beep(uint16_t ms) { writeVP(0x00A0, ms / 8); }

// Screen clock (RTC): 5A A5 YY MM DD hh mm ss at 0x009C.
void setClock(uint8_t yy, uint8_t mo, uint8_t dd, uint8_t hh, uint8_t mi, uint8_t ss) {
  uint8_t d[8] = {0x5A, 0xA5, yy, mo, dd, hh, mi, ss};
  writeBytes(0x009C, d, 8);
}

// One sample onto chart channel `ch` (curve buffer command at 0x0310).
void chartPush(uint8_t ch, int16_t v) {
  uint8_t d[8] = {0x5A, 0xA5, 0x01, 0x00, ch, 0x01, (uint8_t)(v >> 8), (uint8_t)v};
  writeBytes(0x0310, d, 8);
}

// Drawing widget: filled boxes (basic-graphics command 0x0004: x0 y0 x1 y1 color each).
// Coordinates are screen pixels. Check the command table in your T5L manual.
void drawBoxes() {
  const uint16_t boxes[][5] = {
    {40, 330, 140, 450, 0xF800},   // red
    {160, 330, 260, 450, 0x07E0},  // green
    {280, 330, 380, 450, 0x001F},  // blue
    {400, 330, 490, 450, 0xFFE0},  // yellow
  };
  const uint8_t count = sizeof(boxes) / sizeof(boxes[0]);
  uint8_t d[4 + count * 10 + 2];
  uint8_t n = 0;
  d[n++] = 0x00; d[n++] = 0x04;   // command: filled rectangles
  d[n++] = 0x00; d[n++] = count;  // how many
  for (uint8_t i = 0; i < count; i++)
    for (uint8_t k = 0; k < 5; k++) { d[n++] = boxes[i][k] >> 8; d[n++] = boxes[i][k] & 0xFF; }
  d[n++] = 0xFF; d[n++] = 0x00;   // end
  writeBytes(VP_CANVAS, d, n);
}

// MCU list rows: row i (0-based) text lives at VP_FILES + 0x10 + i * 10 (20 bytes per row).
const char *files[] = {"boot.cfg", "recipe_a.txt", "recipe_b.txt", "log_2026.csv"};
uint16_t fileRow(uint8_t i) { return VP_FILES + 0x10 + i * 10; }

// ---------------------------------------------------------------- receiving

// What the screen last sent for the MCU list (1..4, 0 = none).
uint16_t pickedFile = 0;
// The screen draws the list highlight over the row texts, so after a pick the rows are written
// again (blank first: the screen skips redrawing a text whose content did not change).
uint32_t redrawRowsAt = 0;

void redrawRows() {
  for (uint8_t i = 0; i < 4; i++) {
    writeText(fileRow(i), "");
    writeText(fileRow(i), files[i]);
  }
}
char line[48];

void status(const char *s) { writeText(VP_STATUS, s); }

// Screen sent: 5A A5 <len> 83 <VP hi> <VP lo> <words> <data...>
void onTouch(uint16_t vp, const uint8_t *data, uint8_t bytes) {
  uint16_t v = bytes >= 2 ? (data[0] << 8) | data[1] : 0;
  switch (vp) {
    case VP_BEEP:
      beep(100);
      status("Beep!");
      break;
    case VP_PUMP:
      status(v ? "Pump ON" : "Pump OFF");
      break;
    case VP_LEVEL:  // +/- buttons
      writeVP(VP_TANK, v);
      break;
    case VP_DIMMER:
      writeVP(VP_RPM, v * 9);
      break;
    case VP_FAN:
      writeVP(VP_FAN_LAMP, v);  // lamp follows the toggle
      break;
    case VP_HEATER:  // bits: heater = bit 0, light = bit 1 (lamps on the same VP follow by themselves)
      snprintf(line, sizeof(line), "Heater %s, light %s", v & 1 ? "on" : "off", v & 2 ? "on" : "off");
      status(line);
      break;
    case VP_SETPOINT:
      snprintf(line, sizeof(line), "Setpoint %u", v);
      status(line);
      break;
    case VP_NAME: {  // text input: ASCII, ends at FF or 00
      char name[21];
      uint8_t n = 0;
      while (n < bytes && n < 20 && data[n] != 0xFF && data[n] != 0x00) { name[n] = data[n]; n++; }
      name[n] = 0;
      snprintf(line, sizeof(line), "Hello, %s", name);
      status(line);
      break;
    }
    case VP_MODE: {
      const char *modes[] = {"Auto", "Manual", "Off"};
      snprintf(line, sizeof(line), "Mode: %s", v < 3 ? modes[v] : "?");
      status(line);
      break;
    }
    case VP_HOUR:
      snprintf(line, sizeof(line), "Hour %02u", v);
      status(line);
      break;
    case VP_RECIPE:
      snprintf(line, sizeof(line), v ? "Recipe %u picked" : "No recipe", v);
      status(line);
      break;
    case VP_FILES:
      pickedFile = v;
      redrawRowsAt = millis() + 150;  // after the screen has drawn the new highlight
      break;
    case VP_LOAD:
      snprintf(line, sizeof(line), pickedFile ? "Loading %s" : "Pick a file first", pickedFile ? files[pickedFile - 1] : "");
      status(line);
      break;
    case VP_VOLUME:  // knob
      writeVP(VP_VOLUME_BAR, v);
      break;
  }
}

void readDwin() {
  static uint8_t buf[64];
  static uint8_t pos = 0, len = 0;
  while (DWIN.available()) {
    uint8_t b = DWIN.read();
    if (pos == 0 && b != 0x5A) continue;
    if (pos == 1 && b != 0xA5) { pos = 0; continue; }
    if (pos == 2) { len = b; if (len > sizeof(buf) - 3) { pos = 0; continue; } }
    buf[pos++] = b;
    if (pos > 2 && pos == len + 3) {
      pos = 0;
      if (buf[3] == 0x83) {
        printFrame("<-", (buf[4] << 8) | buf[5], buf + 7, len - 4);
        onTouch((buf[4] << 8) | buf[5], buf + 7, len - 4);
      }
      // buf[3] == 0x82 is the "OK" reply to our writes: nothing to do.
    }
  }
}

// ---------------------------------------------------------------- main

uint8_t month(const char *m) {
  const char *names = "JanFebMarAprMayJunJulAugSepOctNovDec";
  for (uint8_t i = 0; i < 12; i++)
    if (!strncmp(m, names + i * 3, 3)) return i + 1;
  return 1;
}

void setup() {
  Serial.begin(115200);  // USB, for debugging
#ifdef ESP32
  DWIN.begin(115200, SERIAL_8N1, 16, 17);  // RX = GPIO16, TX = GPIO17
#else
  DWIN.begin(115200);
#endif
  delay(2000);  // let the screen boot first, or the first frames (clock, texts) are lost

  setPage(PAGE_BUTTONS);
  // Clock = when this sketch was compiled ("Nov  8 2026", "11:51:00").
  const char *d = __DATE__, *t = __TIME__;
  setClock(atoi(d + 9), month(d), atoi(d + 4), atoi(t), atoi(t + 3), atoi(t + 6));

  writeVP(VP_LEVEL, 50);
  writeVP(VP_TANK, 50);
  writeVP(VP_DIMMER, 30);
  writeVP(VP_RPM, 270);
  writeVP(VP_SETPOINT, 120);
  writeVP(VP_MODE, 0);
  writeVP(VP_HOUR, 12);
  writeVP(VP_VOLUME, 40);
  writeVP(VP_VOLUME_BAR, 40);
  writeVP(VP_HEATER, 0b01);  // heater on, light off
  for (uint8_t i = 0; i < 4; i++) writeText(fileRow(i), files[i]);
  status("Ready");
  writeText(VP_TICKER, "UWIN Studio showcase - every widget - ");
  writeUnicode(VP_GREETING, "Привет · Γειά · Hi");
  writeText(VP_QR, "UWIN Studio showcase");
  drawBoxes();
}

void loop() {
  readDwin();
  if (redrawRowsAt && (int32_t)(millis() - redrawRowsAt) >= 0) {
    redrawRowsAt = 0;
    redrawRows();
  }

  static uint32_t fast = 0, slow = 0;
  static uint16_t tick = 0;
  if (millis() - fast >= 100) {  // 10x a second: live values
    fast = millis();
    tick++;
    int16_t wave = 50 + 40 * sin(tick * 0.08);
    chartPush(0, wave);
    writeVP(VP_PRESSURE, wave);
    writeVP(VP_VOLTS, 1200 + 30 * sin(tick * 0.05));  // 12.00 V +/- 0.30
  }
  if (millis() - slow >= 1000) {  // once a second
    slow = millis();
    static uint8_t bat = 0;
    writeVP(VP_BATTERY, bat = (bat + 1) % 4);
  }
}
