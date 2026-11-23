# AULA Nova 75 Pro (SONiX 0c45:800a) — protocol notes

Derived from 5 USBPcap captures of the official AULA driver (wired, 2026-11-23).
Items marked ❓ still need a confirming capture or test.

## Transport

| | |
|---|---|
| VID/PID | `0c45:800a` (SONiX) |
| Interfaces | IF0 boot keyboard (EP 0x81), IF1 keyboard/consumer (EP 0x82), IF2 vendor (EP 0x84 IN / 0x03 OUT), **IF3 vendor (EP 0x86, report desc 30 B)** |
| Command channel | **Feature reports on IF3** via control transfers, report ID 0, 64-byte payload |
| Write | `SET_REPORT`: `bmRequestType=0x21 bRequest=0x09 wValue=0x0300 wIndex=0x0003 wLength=64` |
| Read | `GET_REPORT`: `bmRequestType=0xA1 bRequest=0x01 wValue=0x0300 wIndex=0x0003 wLength=64` |
| Windows hidapi path | the `...&MI_03...` collection of `VID_0C45&PID_800A` |

With hidapi: `send_feature_report([0x00] + 64 bytes)` / `get_feature_report(report_id=0, 65)`.

## Command framing

Command packets start with `0x04, <cmd>`; bytes 2..7 = 0; byte 8 = argument
(usually the number of 64-byte data pages that follow).
After each command the driver reads a feature report; the device echoes
`04 <cmd> 00 01 ...` (byte 3 = `01` → OK). `04 02 00 ff` was seen once (busy?) ❓

| Cmd | Name (inferred) | Arg (byte 8) | Data pages after | Notes |
|---|---|---|---|---|
| `04 02` | status / ping | – | 0 | polled constantly; reply `04 02 00 01` |
| `04 18` | begin config transaction | – | 0 | precedes `04 13` |
| `04 13` | write lighting mode | `01` | 1 | see *Mode block* |
| `04 f0` | commit / end transaction | – | 0 | follows the data page |
| `04 f5` | **read LED colours** | `09` | 9 pages IN | 144 × `[led_idx, R, G, B]` — safe, read-only |
| `04 20` | **per-key colour frame (direct mode)** | `08` | 8 pages OUT | 128 × `[led_idx, R, G, B]` (unused slots = 0), streamed every ~48 ms (~21 FPS); ack `04 20 00 01` |
| `04 28` | set clock (LCD) | `01` | 1 | `00 01 5a YY MM DD hh mm ss` → `1a 0b 17 11 00 1b` = 26-11-23 17:00:27 |

## Mode block (data page after `04 13`)

```
byte:  0    1  2  3   4..7   8     9    10   11..13  14 15
       mode R  G  B   00     flag  bri  spd  00      aa 55
```

| Seen | Meaning |
|---|---|
| `01 ff 00 00 … 00 05 03 … aa 55` | static red |
| `01 00 ff 00 … 00 05 03 … aa 55` | static green |
| `06 ff 00 00 … 01 05 03 … aa 55` | animated effect (wave/rainbow), byte 8 = `01` ("colourful") |
| `00 00 00 00 … 00 00 00 … aa 55` | sent on app start (off / reset?) ❓ |

Byte 9 = brightness (5 = max?) ❓, byte 10 = speed ❓. Trailer `aa 55` is constant.

Full sequence for "set static colour":
```
04 18            → read ack
04 13 .. 01      → read ack
<mode block>
04 02            → read (04 02 00 01 07 02)
04 f0
```

## LED index space

Matrix of 8 rows × 18 columns (`idx = row*0x12 + col`), 0x00–0x8f.
The per-key frame lists **80 lit LEDs** in this order (= physical key order):

```
01-0d (Esc, F1-F12)  13-15
16-1f 67 77
25-31 43 76
37-42 55 79
49-54 65 78
5b-5e 60 62-64 66
```
The full index → key map is `crates/aula-protocol/src/sonix/keymap.rs`. Esc
(0x01) is verified on hardware; the rest follow from the order, except `0x60`
(RAlt) and `0x62` (Fn), which are inferred ❓.

## keylux implementation

`aula_protocol::sonix::Sonix`, reached through `Keyboard::Nova`.

- Discovery: known id only (never deep/allow-list), confirmed by `04 02` ack.
- `stream` / `set_static`: one `04 20 … 08` + ack + 8 pages,
  paced ≥ 46 ms; identical frames are skipped in `stream`.
- `ensure_per_key_mode` is a no-op; mode writes are not implemented.
- Smoke: `nova-ping` (status + colour readback), `nova-led <slot>`.

## Open questions ❓

1. ✅ **Per-key byte order = `[led_idx, R, G, B]`.** Confirmed: the final frame
   `01 fe 00 00` made Esc physically **red**.
2. **Per-key frames vs. mode.** `04 20` frames changed the LEDs live *without* a
   `04 13` mode switch (keyboard was in static-green mode), but `04 f5` readback
   kept reporting the stored mode colours. So: `04 20` = live/direct overlay,
   `04 f5` = stored state. ✅ User-reported: per-key colours **persist** after the
   host stops streaming, so keylux can send one frame per change (no keep-alive).
   Behaviour after unplug/replug or a mode change not yet checked ❓
3. Meaning of bytes 9/10 and the "off" block.

## Safety

- Discovery should only use `04 02` and `04 f5` (both read-only).
- Keep per-key writes ≤ ~21 FPS (driver itself uses ~48 ms spacing).
- Never send unknown `04 xx` opcodes.
