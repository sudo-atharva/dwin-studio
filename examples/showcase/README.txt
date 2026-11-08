DWIN Studio showcase
====================

One of every widget DWIN Studio has, on 4 screens, plus an Arduino sketch that
drives all of them.

  showcase.dwin              the project: double-click to open it in DWIN Studio
  assets\                    pictures used by the battery icons, fan animation, logo
  output\DWIN_SET\           built screen files: copy this folder to the SD card
  output\ui_map.txt          every address, what it does, the UART frames
  output\ui_gen.h            #defines for the MCU (same file as in arduino\showcase)
  arduino\showcase\          Arduino sketch (Mega, ESP32, Leonardo, ...)

Screens (swipe left/right or use the arrow buttons)
  1 Buttons & switches   push button (beep), on/off button, +/- buttons with a
                         number, slider, toggle + lamp, two toggles and lamps
                         sharing one VP by bit, big number, decimals, battery
                         icons, fan animation, picture
  2 Inputs               number keypad, text keyboard, dropdown, roller, settable
                         clock, fixed list, list filled by the MCU + Load button
  3 Displays             status text, scrolling text, unicode (Russian/Greek)
                         text, progress bar, gauge, knob, vertical bar
  4 Graphics             live chart, QR code, drawing area

What the sketch does
  - sets the clock to the time the sketch was compiled
  - fills the file list, the texts, the QR code, draws 4 colored boxes
  - every 0.1 s: chart sample, gauge, volts; every 1 s: battery icon
  - answers touches: beep sounds the buzzer, the fan lamp follows the fan
    toggle, the tank follows the +/- level, rpm follows the dimmer, the bar
    follows the knob, and the status line says what you touched

Wiring: display TX -> board RX1, display RX -> board TX1, GND -> GND.
        ESP32: RX = GPIO16, TX = GPIO17. 115200 baud, CRC off.

No display yet? Open showcase.dwin, press Simulate, pick the board's COM port:
the PC acts as the screen.

After changing the project: Build, then copy the new ui_gen.h into
arduino\showcase\ so the addresses match.
