//! AULA Nova 75 Pro driver (SONiX `0c45:800a`, wired).
//!
//! A different chip and protocol from the Sinowealth F75 in `crate::f75`; see
//! `docs/NOVA75.md`. What this driver does, and deliberately does not do:
//!
//! * **Discovery is read-only.** A collection is confirmed by the status poll
//!   (`04 02`), which the vendor driver itself sends several times a second.
//! * **Colours go out as per-key frames (`04 20`).** Verified on hardware:
//!   they show over the board's **static** mode, are painted over by its
//!   animated effects, and are dropped shortly after the host stops sending —
//!   so frames are kept alive (see [`protocol::KEEPALIVE_MS`]).
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

/// Send one command and read its echo.
///
/// Succeeds on any echo of `cmd`; the status byte is the caller's business
/// (see [`p::is_echo`]). An echo with the ready status ends the wait early;
/// otherwise the read is retried a few times and the last echo returned.
/// The write itself is never repeated here.
fn exchange(io: &Transport, cmd: u8, arg: u8) -> Result<Vec<u8>> {
    io.send(&p::command(cmd, arg))?;
    let mut echo = None;
    let mut err = None;
    for attempt in 0..3 {
        thread::sleep(Duration::from_millis(if attempt == 0 { 3 } else { 15 }));
        match io.receive() {
            Ok(resp) if p::is_ack(&resp, cmd) => return Ok(resp),
            Ok(resp) if p::is_echo(&resp, cmd) => echo = Some(resp),
            Ok(_) => {}
            Err(e) => err = Some(e),
        }
    }
    match (echo, err) {
        (Some(resp), _) => Ok(resp),
        (None, Some(e)) => Err(e),
        (None, None) => Err(Error::Nack { cmd }),
    }
}

/// Step-by-step report of how the Nova's command collection responds.
///
/// Touches only the `ff13` collection, and sends only the status poll the
/// vendor driver itself sends constantly. One line per step, for a user to
/// paste back when discovery fails.
/// Optional write steps for [`diagnose`]. All off means read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiagnoseWrites {
    /// Per-key frames of Esc red, everything else off. 0 = none.
    pub esc_red_frames: u32,
    /// Send `04 f0` (end of a config transaction) once before anything else,
    /// then re-read the status. The vendor driver sends it after every config
    /// write; here it closes a transaction left open, if there is one.
    pub end_transaction: bool,
}

pub fn diagnose(writes: DiagnoseWrites) -> Result<Vec<String>> {
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
                    "  read #{attempt}: {} bytes [{}] echo={} status={:?}",
                    r.len(),
                    hex(&r),
                    p::is_echo(&r, p::cmd::STATUS),
                    p::status(&r, p::cmd::STATUS)
                )),
                Err(e) => out.push(format!("  read #{attempt}: FAILED {e}")),
            }
        }
        if writes.end_transaction {
            match io.send(&p::command(p::cmd::COMMIT, 0)) {
                Ok(()) => out.push("  write 04 f0: ok".into()),
                Err(e) => out.push(format!("  write 04 f0: FAILED {e}")),
            }
            thread::sleep(Duration::from_millis(300));
            match exchange(&io, p::cmd::STATUS, 0) {
                Ok(r) => out.push(format!(
                    "  status after 04 f0: [{}] status={:?}",
                    hex(&r),
                    p::status(&r, p::cmd::STATUS)
                )),
                Err(e) => out.push(format!("  status after 04 f0: FAILED {e}")),
            }
        }
        if writes.esc_red_frames > 0 {
            // Esc red, everything else off. Exactly the packets the vendor
            // driver streams, at its own ~48 ms spacing.
            let mut entries: Vec<(u8, [u8; 3])> =
                keymap::SLOTS.iter().map(|(i, _)| (*i, [0, 0, 0])).collect();
            entries[0].1 = [0xff, 0, 0];
            let mut ok = 0;
            let mut first_err = None;
            let mut statuses = std::collections::BTreeSet::new();
            for _ in 0..writes.esc_red_frames {
                let started = std::time::Instant::now();
                match exchange(&io, p::cmd::PER_KEY, p::PER_KEY_PAGES) {
                    Ok(r) => {
                        if let Some(s) = p::status(&r, p::cmd::PER_KEY) {
                            statuses.insert(s);
                        }
                    }
                    Err(e) => {
                        first_err.get_or_insert(format!("04 20: {e}"));
                        continue;
                    }
                }
                let mut pages_ok = true;
                for page in p::per_key_pages(&entries) {
                    if let Err(e) = io.send(&page) {
                        first_err.get_or_insert(format!("page: {e}"));
                        pages_ok = false;
                        break;
                    }
                }
                if pages_ok {
                    ok += 1;
                }
                let gap = Duration::from_millis(48);
                if let Some(rest) = gap.checked_sub(started.elapsed()) {
                    thread::sleep(rest);
                }
            }
            out.push(format!(
                "  per-key frames: {ok}/{} sent, 04 20 status seen {:?}{}",
                writes.esc_red_frames,
                statuses,
                first_err
                    .map(|e| format!(", first error {e}"))
                    .unwrap_or_default()
            ));
            match exchange(&io, p::cmd::STATUS, 0) {
                Ok(r) => out.push(format!(
                    "  status after frames: status={:?}",
                    p::status(&r, p::cmd::STATUS)
                )),
                Err(e) => out.push(format!("  status after frames: FAILED {e}")),
            }
            out.push("  Esc should now be red and every other key off.".into());
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

    /// One frame, paced exactly like the vendor driver: status poll, per-key
    /// command, then the pages with a gap between each. The status byte is
    /// ignored (the vendor driver gets `ff` here too and carries on); only a
    /// missing echo of `04 20` is an error.
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let ms = |n| thread::sleep(Duration::from_millis(n));
        self.respect_gap();

        self.io.send(&p::command(p::cmd::STATUS, 0))?;
        ms(p::ECHO_DELAY_MS);
        let _ = self.io.receive()?;
        ms(p::COMMAND_GAP_MS);

        self.io
            .send(&p::command(p::cmd::PER_KEY, p::PER_KEY_PAGES))?;
        ms(p::ECHO_DELAY_MS);
        let echo = self.io.receive()?;
        if !p::is_echo(&echo, p::cmd::PER_KEY) {
            return Err(Error::Nack {
                cmd: p::cmd::PER_KEY,
            });
        }

        for (i, page) in p::per_key_pages(&encode(self.order, frame))
            .iter()
            .enumerate()
        {
            if i > 0 {
                ms(p::PAGE_GAP_MS);
            }
            self.io.send(page)?;
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

    /// Not implemented. Frames only show over the board's static mode, and
    /// switching to it is a stored-settings write (`04 18 / 04 13 / 04 f0`)
    /// that has not been exercised from keylux yet; set static in the vendor
    /// app once instead. See `docs/NOVA75.md`.
    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        Ok(false)
    }

    /// Nothing on this board persists host colours, so this is an immediate
    /// full repaint and nothing more: the board returns to its stored mode
    /// once frames stop.
    fn set_static(&mut self, frame: &Frame) -> Result<()> {
        self.send_frame(frame)
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        // Skip a frame the board is already showing, but only until the
        // firmware would drop it.
        let fresh = self
            .last_write
            .is_some_and(|t| t.elapsed() < Duration::from_millis(p::KEEPALIVE_MS));
        if fresh && self.last_frame.as_ref() == Some(frame) {
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
