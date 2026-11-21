# Lamzu Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the Lamzu Maya X's battery level (2.4 GHz dongle) in the overlay.

**Architecture:** New `src/providers/lamzu.rs` modelled on `razer.rs` (feature reports via `FeatureChannel`): pure request/parse/query at the top, `#[cfg(windows)] mod win` poll, registered in `hid::VENDORS` and `poll_all()`.

**Tech Stack:** Rust, `windows` crate HID, `cargo test` on macOS, `cargo xwin`.

**Spec:** `docs/superpowers/specs/2026-11-21-lamzu-provider-design.md` (Findings + Phase 2)

## Global Constraints
- Vendor ID `0x373e`; protocol collection = `feature_len == 65` (report ID + 64). Don't filter by PID (`0x001c` and `0x001e` both seen).
- Only ever send the battery request `00 00 02 02 00 83` (after report ID 0). No settings commands.
- Open with `HidDevice::open_for_features` (shared; zero-access fallback).
- Reply accepted only if byte 1 (after the report ID) is `0xa1` and target/len/class/cmd echo the request.
- Level = data byte 1 (percent, clamp 100); charging = data byte 0 `== 1` (unverified — say so in the module doc). Level 0 → `None` (asleep / off; no false alert).
- `DeviceKind::Mouse`; id `lamzu:{pid:04x}`; name from product string via `clean_product_name`, fallback `"Lamzu mouse"`.
- No copied GPL code.

## Review Focus
- A stale reply left from another command (web driver running at the same time) must not be read as the battery — retry until the echoed command matches, then give up.
- The mouse off / out of range (non-`a1` status or level 0) must hide the device, not show 0 %.
- A reply shorter than 9 bytes must not panic.
- Both interfaces of the dongle (`mi_01` input-only collections) must be skipped — only `feature_len == 65` is queried.
- The device appears once even if several collections match.

---

### Task 1: Lamzu provider

**Files:**
- Create: `src/providers/lamzu.rs`
- Modify: `src/providers/hid.rs` (add `pub const LAMZU: u16 = 0x373e;`, `VENDORS: [u16; 5]`, module doc vendor list)
- Modify: `src/providers/mod.rs` (`pub mod lamzu;`, `devices.extend(lamzu::poll(&collections));`)
- Modify: `src/probe.rs` doc comment vendor list; `CLAUDE.md` providers section and `README.md` vendor line (Lamzu: Maya X via dongle, charging flag unverified; ATK still unimplemented)

**Interfaces:**
- Consumes: `hid::FeatureChannel`, `hid::HidDevice::open_for_features`, `hid::clean_product_name`, `hid::HidInfo`
- Produces: `pub const REPORT_LEN: usize = 65;` `pub fn request(target: u8, class: u8, cmd: u8, len: u8) -> [u8; REPORT_LEN]`; `pub fn parse_reply<'a>(req: &[u8; REPORT_LEN], r: &'a [u8]) -> Option<&'a [u8]>` (the `len` data bytes); `pub fn query(dev: &mut dyn FeatureChannel) -> Option<(u8, bool)>`; Windows `pub fn poll(collections: &[HidInfo]) -> Vec<Device>`

- [ ] **Step 1: Write the failing tests** in `lamzu.rs` with a `Fake: FeatureChannel` that stores the last set report and answers from a closure / fields, like `razer.rs`'s `Fake`.

```rust
#[test]
fn battery_request_matches_capture() {
    let r = request(TARGET_MOUSE, CLASS_STATUS, CMD_BATTERY, 2);
    assert_eq!(&r[..7], &[0x00, 0x00, 0x00, 0x02, 0x02, 0x00, 0x83]);
    assert!(r[7..].iter().all(|&b| b == 0));
}

#[test]
fn reads_captured_reply() {
    // A1 00 02 02 00 83 00 64 — driver showed 100 %, not charging.
    let mut dev = Fake::answering(&[0xa1, 0x00, 0x02, 0x02, 0x00, 0x83, 0x00, 0x64]);
    assert_eq!(query(&mut dev), Some((100, false)));
}

#[test]
fn charging_flag() { /* data `01 37` → Some((55, true)) */ }

#[test]
fn level_zero_or_error_status_hides_device() { /* data `00 00` → None; status 0xa2 → None */ }

#[test]
fn skips_stale_reply_then_reads_battery() { /* first get returns the 00 82 reply `a1 00 02 01 00 82 02`, second the battery reply → Some((100, false)) */ }

#[test]
fn gives_up_after_retries() { /* always the stale 00 82 reply → None, get_feature called RETRIES times */ }

#[test]
fn short_reply_is_rejected() { /* parse_reply on 5 bytes → None */ }
```

`Fake` returns replies with the report ID byte `0x00` prepended, padded to `REPORT_LEN`.

- [ ] **Step 2: Run** `cargo test lamzu` — Expected: compile errors (module/functions missing).

- [ ] **Step 3: Implement** `lamzu.rs` top half. Constants: `TARGET_MOUSE = 0x02`, `CLASS_STATUS = 0x00`, `CMD_BATTERY = 0x83`, `STATUS_OK = 0xa1`, `RETRIES = 5`. `query`: `set_feature` once, then up to `RETRIES` × (`pause`, `get_feature(0, REPORT_LEN)`, `parse_reply`); first match wins. Module doc describes the layout from the spec's Findings, that it was read from a USB capture of the Maya X, and what's unverified.

- [ ] **Step 4: Run** `cargo test` — Expected: all pass.

- [ ] **Step 5: Windows side + registration.** `mod win::poll`: collections with `vid == hid::LAMZU && feature_len as usize == REPORT_LEN`, skip PIDs already done, `open_for_features`, `query`, push `Device`. Register in `VENDORS` and `poll_all()`; update docs listed under Files.

- [ ] **Step 6: Verify** `cargo xwin build --release --target x86_64-pc-windows-msvc` — Expected: no warnings. `cargo test` — all pass.

- [ ] **Step 7: Commit** on `feature/lamzu-provider` (branched from `feature/lamzu-discovery`, which isn't merged into `dev` yet):

```bash
git commit -m "feat(lamzu-provider): read Lamzu Maya X battery over its dongle"
```
