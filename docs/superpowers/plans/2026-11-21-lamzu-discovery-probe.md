# Lamzu Discovery Probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `--probe --all` also lists vendor-defined USB HID collections from vendors we don't support yet, so the Lamzu Maya X dongle's IDs and report sizes can be read.

**Architecture:** `hid::enumerate` becomes a thin wrapper over a new `hid::enumerate_where(keep_vid)` that filters by vendor ID before opening. A platform-independent predicate picks "unknown vendor + vendor-defined usage page" collections; probe prints them with the existing `collection_line`.

**Tech Stack:** Rust, `windows` crate (CfgMgr32 / HID), `cargo test` on macOS, `cargo xwin` for Windows.

**Spec:** `docs/superpowers/specs/2026-11-21-lamzu-provider-design.md` (Phase 1)

## Global Constraints
- Opening for enumeration uses zero access (`open_handle(&path, 0, false)`), shared — unchanged.
- USB only: paths that `parse_usb_path` rejects (Bluetooth HID) stay excluded.
- Plain `--probe` output is unchanged; the new section appears only with `--all`.
- Vendor-defined usage page means `usage_page >= 0xFF00`.
- Pure logic stays outside `#[cfg(windows)]` so it tests on macOS.

## Review Focus
- A known vendor (e.g. Logitech `0xff00` collection) must not appear in the new section — it's already in "Vendor HID collections".
- Standard collections of unknown vendors (mouse `0x0001:0x0002`, keyboard, consumer `0x000c`) must not appear — they're noise.
- Usage page exactly `0xFF00` and `0xFFFF` both count as vendor-defined; `0xFEFF` doesn't.
- An empty result prints the heading only, no panic.
- Enumeration of every vendor must not open anything with read/write access.

---

### Task 1: List unknown vendor-defined HID collections in `--probe --all`

**Files:**
- Modify: `src/providers/hid.rs` (pure predicate near `clean_product_name`; `enumerate` → `enumerate_where` in `mod win`; tests module)
- Modify: `src/probe.rs:147-152` (new section after "Vendor HID collections")
- Modify: `CLAUDE.md` (probe line mentions the new section)

**Interfaces:**
- Produces: `pub fn is_unknown_vendor_defined(c: &HidInfo) -> bool` (platform-independent)
- Produces (Windows): `pub fn enumerate_where(keep_vid: impl Fn(u16) -> bool) -> Vec<HidInfo>`; `enumerate(vendors: &[u16])` keeps its signature and calls `enumerate_where(|v| vendors.contains(&v))`. Export `enumerate_where` alongside `enumerate`.

- [ ] **Step 1: Write the failing test** in `hid.rs` tests

```rust
#[test]
fn picks_unknown_vendor_defined_collections() {
    let c = |vid, usage_page| HidInfo { vid, usage_page, ..Default::default() };
    assert!(is_unknown_vendor_defined(&c(0x3554, 0xff00)));
    assert!(is_unknown_vendor_defined(&c(0x3554, 0xffff)));
    assert!(!is_unknown_vendor_defined(&c(0x3554, 0xfeff)));
    assert!(!is_unknown_vendor_defined(&c(0x3554, 0x0001)));
    assert!(!is_unknown_vendor_defined(&c(0x3554, 0x000c)));
    assert!(!is_unknown_vendor_defined(&c(LOGITECH, 0xff00)));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test picks_unknown_vendor_defined` — Expected: compile error, `is_unknown_vendor_defined` not found.

- [ ] **Step 3: Implement** `is_unknown_vendor_defined`: `!VENDORS.contains(&c.vid) && c.usage_page >= 0xff00`, with a one-line doc comment saying it's for spotting dongles of unsupported vendors in `--probe --all`.

- [ ] **Step 4: Run tests** — `cargo test` — Expected: all pass.

- [ ] **Step 5: Windows side**
  - In `hid::win`, rename the body of `enumerate` to `enumerate_where(keep_vid)`, filtering with `keep_vid(vid)` before `open_handle`; `enumerate` delegates. Update the module doc's "Only USB devices from the vendors we know are ever opened" to note the probe exception (zero-access only).
  - In `probe::run`, inside `if all`, after the vendor section: heading `"\nOther vendor-defined HID collections (USB):"`, then `hid::enumerate_where(|_| true)` filtered by `hid::is_unknown_vendor_defined`, sorted by path, each printed with `collection_line`. Update the probe module doc comment to mention it.
  - CLAUDE.md "Can't run the app here" bullet: `--probe --all` also lists vendor-defined HID collections of unsupported vendors.

- [ ] **Step 6: Verify the Windows build**

Run: `cargo xwin build --release --target x86_64-pc-windows-msvc` — Expected: builds with no new warnings. Then `cargo test` — all pass.

- [ ] **Step 7: Commit** (on a feature branch off `dev`, per the user's git skill)

```bash
git add src/providers/hid.rs src/probe.rs CLAUDE.md docs/superpowers
git commit -m "feat(lamzu-discovery): list unknown vendor-defined HID collections in --probe --all"
```
