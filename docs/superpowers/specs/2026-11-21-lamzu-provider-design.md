# Lamzu provider — design

## Intent
Show the battery level of a Lamzu Maya X (2.4 GHz dongle) in the overlay, and
alert when it runs low, like the other vendor providers. Lamzu has no public
protocol notes, so the protocol is learned from the user's own hardware rather
than guessed.

## Goals / non-goals / anti-goals
- Goal: Maya X battery level (and charging, if the device reports it) via its USB dongle.
- Goal: other Lamzu models work if they share the protocol; unknown ones are ignored.
- Non-goal: Lamzu over Bluetooth or wired-only mode (Maya X is 2.4 GHz / wired).
- Non-goal: ATK in this change (the probe extension below should help it later).
- Anti-goal: anything that disturbs the Lamzu web driver or the mouse's settings —
  only read-only battery queries, handles opened shared.

## Constraints
Same as every vendor provider (CLAUDE.md): USB only, filter by vendor ID before
opening, non-exclusive handles, replies matched by content, parser
platform-independent and tested with `MockChannel`/feature mock, no copied GPL code.

## Approaches
1. **Capture the web driver's USB traffic + extend `--probe`** — chosen. Real bytes
   from the user's dongle; costs the user ~10 min on Windows (Wireshark + USBPcap).
2. Read the web driver's JavaScript — no capture needed, but minified code may be
   misread or differ by firmware; still needs on-device confirmation. Used only as
   a cross-check while reading the capture.
3. Guess, labelled unverified — fastest, but likely wrong; would end up at 1 anyway.

## Phase 1 — discovery (this change)
- `hid::enumerate` gains a way to list USB HID collections of any vendor
  (e.g. an `enumerate_where(filter)` with the current call as the vendor-list case).
  Opening still uses zero access, so it's safe for mice and keyboards.
- `--probe --all` adds a section "Other vendor-defined HID collections (USB)":
  collections whose vendor isn't in `hid::VENDORS` and whose usage page is
  vendor-defined (≥ 0xFF00), printed with the existing `collection_line`.
  Plain `--probe` is unchanged.
- Capture instructions for the user: Wireshark + USBPcap on the dongle's root hub,
  open the Lamzu web driver, capture once on the charger and once off it at a known
  level; also `battery_overlay.exe --probe --all > out.txt`.

## Phase 2 — provider (after the capture)
- `src/providers/lamzu.rs` following `steelseries.rs`: `LAMZU` vendor ID constant
  in `hid.rs` and `VENDORS`, request bytes + parser at the top, `#[cfg(windows)] mod win`
  poll, one line in `poll_all()`, `DeviceKind::Mouse`, id `lamzu:{pid}`.
- Output-report or feature-report transport, whichever the capture shows
  (`transact` or `FeatureChannel`).
- Tests built from the captured replies.
- Update CLAUDE.md/README: Lamzu verified on Maya X; ATK still unimplemented.

## Left to the builder
Exact filter signature, section wording, and phase-2 details that depend on the capture.
