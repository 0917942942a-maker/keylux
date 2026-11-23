//! AULA Nova 75 Pro driver (SONiX `0c45:800a`, wired).
//!
//! A different chip and protocol from the Sinowealth F75 in `crate::f75`; see
//! `docs/NOVA75.md`. What this driver does, and deliberately does not do:
//!
//! * **Discovery is read-only.** A collection is confirmed by the status poll
//!   (`04 02`), which the vendor driver itself sends several times a second.
//! * **Colours go out as per-key frames (`04 20`).** They take effect
//!   immediately in whatever lighting mode the board is in, and stay after the
//!   host goes quiet, so there is no mode switch and no keepalive.
//! * **No stored-settings writes.** The mode transaction (`04 18 / 04 13 /
//!   04 f0`) is decoded but unused: nothing in keylux needs it yet, and it is
//!   the one path here that changes non-volatile state.

pub mod keymap;
pub mod protocol;

use std::thread;
use std::time::{Duration, Instant};

use crate::color::ChannelOrder;
use crate::device::{DeviceId, Frame, KeyPos, RgbDevice};
use crate::transport::{self, Choice, DeviceCandidate, ScanOptions, ScanSpec, Transport};
use crate::{Error, Result};

use protocol as p;

pub const MODEL: &str = "AULA Nova 75 Pro";

pub struct Sonix {
    io: Transport,
    layout: Vec<KeyPos>,
    order: ChannelOrder,
    last_write: Option<Instant>,
    min_gap: Duration,
    /// Last frame actually sent, so identical ones are skipped.
    last_frame: Option<Frame>,
    id: DeviceId,
    name: String,
}

fn scan_spec<'a>(opts: &'a ScanOptions) -> ScanSpec<'a> {
    ScanSpec {
        model: MODEL,
        report_id: p::REPORT_ID,
        packet_len: p::PACKET_LEN,
        known: p::KNOWN,
        opts,
        // The Nova only answers a read after a command; a bare read can stall.
        read_probe: false,
        collection: Some(p::is_command_collection),
    }
}

/// Send one command and check its echo.
///
/// The vendor driver reads the echo ~3 ms after the write, and the firmware
/// answers `04 xx 00 ff` while busy, so the read is retried a few times
/// before giving up. The write itself is never repeated here.
fn exchange(io: &Transport, cmd: u8, arg: u8) -> Result<Vec<u8>> {
    io.send(&p::command(cmd, arg))?;
    let mut last = Err(Error::Nack { cmd });
    for attempt in 0..4 {
        thread::sleep(Duration::from_millis(if attempt == 0 { 3 } else { 15 }));
        match io.receive() {
            Ok(resp) if p::is_ack(&resp, cmd) => return Ok(resp),
            Ok(_) => last = Err(Error::Nack { cmd }),
            Err(e) => last = Err(e),
        }
    }
    last
}

/// Step-by-step report of how the Nova's command collection responds.
///
/// Touches only the `ff13` collection, and sends only the status poll the
/// vendor driver itself sends constantly. One line per step, for a user to
/// paste back when discovery fails.
pub fn diagnose() -> Result<Vec<String>> {
    let hex = |b: &[u8]| {
        b.iter()
            .take(12)
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = Vec::new();
    let cols = transport::list_collections(p::VENDOR_ID, p::PRODUCT_ID)?;
    let ours: Vec<_> = cols
        .iter()
        .filter(|c| p::is_command_collection(c.usage_page, c.interface))
        .collect();
    out.push(format!(
        "{} collection(s) for {}, {} on usage page {:#06x}",
        cols.len(),
        p::WIRED,
        ours.len(),
        p::USAGE_PAGE
    ));
    for c in ours {
        out.push(format!("interface {} usage {:#06x}", c.interface, c.usage));
        let io = match Transport::open_path(&c.path, p::REPORT_ID, p::PACKET_LEN) {
            Ok(io) => io,
            Err(e) => {
                out.push(format!("  open: FAILED {e}"));
                continue;
            }
        };
        out.push("  open: ok".into());
        match io.send(&p::command(p::cmd::STATUS, 0)) {
            Ok(()) => out.push(format!("  write 04 02 ({} bytes): ok", p::PACKET_LEN)),
            Err(e) => out.push(format!(
                "  write 04 02 ({} bytes): FAILED {e}",
                p::PACKET_LEN
            )),
        }
        for attempt in 0..3 {
            thread::sleep(Duration::from_millis(if attempt == 0 { 3 } else { 15 }));
            match io.receive() {
                Ok(r) => out.push(format!(
                    "  read #{attempt}: {} bytes [{}] ack={}",
                    r.len(),
                    hex(&r),
                    p::is_ack(&r, p::cmd::STATUS)
                )),
                Err(e) => out.push(format!("  read #{attempt}: FAILED {e}")),
            }
        }
    }
    Ok(out)
}

/// Is this collection the Nova's command channel?
///
/// Only the status poll is sent — a read the vendor driver issues constantly.
fn speaks_the_protocol(io: &Transport) -> bool {
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        if exchange(io, p::cmd::STATUS, 0).is_ok() {
            return true;
        }
    }
    false
}

/// The Nova's own ids only.
///
/// Unlike the F75 scan this never widens to allow-listed or unrecognised
/// hardware, whatever `opts` says: the confirming status poll is a *write* of
/// `04 02`, harmless on this board and unknown on anyone else's.
fn known_only(_opts: &ScanOptions) -> ScanOptions {
    ScanOptions::default()
}

pub fn discover(opts: &ScanOptions) -> Result<Vec<DeviceCandidate>> {
    let opts = known_only(opts);
    transport::discover_with(&scan_spec(&opts), speaks_the_protocol)
}

/// The same scan, keeping every near-miss. For diagnostics.
pub fn probe(opts: &ScanOptions) -> Result<Vec<transport::CollectionProbe>> {
    let opts = known_only(opts);
    transport::probe_with(&scan_spec(&opts), speaks_the_protocol)
}

impl Sonix {
    pub fn open(opts: &ScanOptions) -> Result<Self> {
        let found = discover(opts)?;
        match transport::choose(&found, None) {
            Choice::Use(i) => Self::open_candidate(&found[i], ChannelOrder::Rgb),
            _ => Err(Error::NotFound {
                searched: p::KNOWN.len(),
            }),
        }
    }

    pub fn open_candidate(c: &DeviceCandidate, order: ChannelOrder) -> Result<Self> {
        let io = Transport::open_path(&c.path, p::REPORT_ID, p::PACKET_LEN)?;
        Ok(Self {
            io,
            layout: keymap::layout(),
            order,
            last_write: None,
            min_gap: Duration::from_millis(p::MIN_FRAME_GAP_MS),
            last_frame: None,
            id: c.id,
            name: format!("{MODEL} ({})", c.link.suffix()),
        })
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn hid_path(&self) -> &str {
        &self.io.path
    }

    pub fn set_channel_order(&mut self, order: ChannelOrder) {
        self.order = order;
    }

    /// Status poll. Read-only.
    pub fn ping(&self) -> Result<Vec<u8>> {
        exchange(&self.io, p::cmd::STATUS, 0)
    }

    /// The stored colours of all 144 matrix positions. Read-only.
    ///
    /// This reports the board's own lighting state; per-key frames sent by the
    /// host are not reflected in it.
    pub fn read_colors(&self) -> Result<Vec<(u8, [u8; 3])>> {
        exchange(&self.io, p::cmd::READ_COLORS, p::READ_PAGES)?;
        let pages = (0..p::READ_PAGES)
            .map(|_| self.io.receive())
            .collect::<Result<Vec<_>>>()?;
        Ok(p::parse_colors(&pages))
    }

    fn respect_gap(&mut self) {
        if let Some(prev) = self.last_write {
            let elapsed = prev.elapsed();
            if elapsed < self.min_gap {
                thread::sleep(self.min_gap - elapsed);
            }
        }
        self.last_write = Some(Instant::now());
    }

    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.respect_gap();
        exchange(&self.io, p::cmd::PER_KEY, p::PER_KEY_PAGES)?;
        for page in p::per_key_pages(&encode(self.order, frame)) {
            self.io.send(&page)?;
        }
        self.last_frame = Some(frame.clone());
        Ok(())
    }
}

/// Frame slots -> `(matrix index, wire colour)`. Pure, for tests.
fn encode(order: ChannelOrder, frame: &Frame) -> Vec<(u8, [u8; 3])> {
    keymap::SLOTS
        .iter()
        .enumerate()
        .map(|(slot, (idx, _))| (*idx, order.apply(frame.get(slot))))
        .collect()
}

impl RgbDevice for Sonix {
    fn name(&self) -> &str {
        &self.name
    }

    fn led_count(&self) -> usize {
        keymap::LED_COUNT
    }

    fn layout(&self) -> &[KeyPos] {
        &self.layout
    }

    fn max_fps(&self) -> u32 {
        p::MAX_FPS
    }

    /// Nothing to do: per-key frames render in any mode.
    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        Ok(false)
    }

    /// The same per-key frame as [`RgbDevice::stream`], always sent.
    ///
    /// Colours stay after the host stops sending. Whether they survive a
    /// replug has not been checked; see `docs/NOVA75.md`.
    fn set_static(&mut self, frame: &Frame) -> Result<()> {
        self.send_frame(frame)
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        if self.last_frame.as_ref() == Some(frame) {
            return Ok(());
        }
        self.send_frame(frame)
    }

    fn set_max_fps(&mut self, fps: u32) {
        let fps = fps.clamp(1, p::MAX_FPS);
        self.min_gap = Duration::from_millis((1000 / u64::from(fps)).max(p::MIN_FRAME_GAP_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[test]
    fn esc_red_encodes_like_the_vendor_driver() {
        let mut f = Frame::black(keymap::LED_COUNT);
        f.set(0, Rgb::new(0xfe, 0, 0));
        let e = encode(ChannelOrder::Rgb, &f);
        assert_eq!(e.len(), 80);
        assert_eq!(e[0], (0x01, [0xfe, 0, 0]));
        assert!(e[1..].iter().all(|(_, c)| *c == [0, 0, 0]));
    }

    #[test]
    fn every_slot_reaches_its_matrix_index() {
        let f = Frame::solid(keymap::LED_COUNT, Rgb::WHITE);
        let e = encode(ChannelOrder::Rgb, &f);
        let ids: Vec<u8> = e.iter().map(|(i, _)| *i).collect();
        let want: Vec<u8> = keymap::SLOTS.iter().map(|(i, _)| *i).collect();
        assert_eq!(ids, want);
    }
}
