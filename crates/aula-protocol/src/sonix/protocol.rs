//! Wire format of the AULA Nova 75 Pro (SONiX `0c45:800a`), wired.
//!
//! Reverse-engineered from USBPcap captures of the vendor driver; the full
//! write-up is `docs/NOVA75.md`. Everything here is pure byte layout so it can
//! be unit-tested without hardware.
//!
//! The channel is a **64-byte feature report, report ID 0, on interface 3**
//! (`MI_03` on Windows). Every command is `04 <cmd> 00 00 00 00 00 00 <arg>`,
//! zero-padded, and is answered by a feature-report read that echoes
//! `04 <cmd> 00 01` on success.
//!
//! This is a different chip and a different protocol from the Sinowealth F75 in
//! `crate::f75`. Nothing in that module applies here.

use crate::device::{DeviceId, Link};

pub const VENDOR_ID: u16 = 0x0c45; // SONiX
pub const PRODUCT_ID: u16 = 0x800a; // AULA Nova 75 Pro, wired

pub const WIRED: DeviceId = DeviceId::new(VENDOR_ID, PRODUCT_ID);

/// Built-in known ids. Only the wired link has been captured.
pub const KNOWN: &[(DeviceId, Link)] = &[(WIRED, Link::Wired)];

/// The command collection. Windows reports it as interface 3, `ff13/0001`.
pub const USAGE_PAGE: u16 = 0xff13;
pub const INTERFACE: i32 = 3;

/// Is this the collection commands go to? By usage page where the OS reports
/// one; by interface number where it does not (Linux hidraw has reported 0
/// for every collection but the first).
pub fn is_command_collection(usage_page: u16, interface: i32) -> bool {
    usage_page == USAGE_PAGE || (usage_page == 0 && interface == INTERFACE)
}

/// Unnumbered reports: hidapi wants a leading 0x00 byte on the buffer.
pub const REPORT_ID: u8 = 0x00;
/// Payload length of one feature report, as seen on the wire.
pub const REPORT_LEN: usize = 64;
/// Buffer length handed to hidapi: report ID byte + payload.
pub const PACKET_LEN: usize = 1 + REPORT_LEN;

/// First byte of every command.
pub const PREFIX: u8 = 0x04;
/// Byte 3 of a reply when the command was accepted.
pub const ACK_OK: u8 = 0x01;
/// Offset of the argument byte inside the 64-byte payload.
pub const ARG_OFFSET: usize = 8;

/// Command bytes. Only the ones the vendor driver was seen to send are listed,
/// and the driver here only uses the read-only ones plus [`cmd::PER_KEY`].
/// Never send anything else: see "Safety" in `docs/NOVA75.md`.
pub mod cmd {
    /// Status poll. Read-only; the vendor driver sends it constantly.
    pub const STATUS: u8 = 0x02;
    /// Read the stored LED colours: `READ_PAGES` pages of `[idx, R, G, B]`.
    pub const READ_COLORS: u8 = 0xf5;
    /// Per-key colour frame: `PER_KEY_PAGES` pages of `[idx, R, G, B]` follow.
    pub const PER_KEY: u8 = 0x20;

    // Seen, decoded, deliberately NOT used yet (they write stored settings):
    /// Opens a config transaction.
    pub const BEGIN: u8 = 0x18;
    /// Lighting mode block follows (one page).
    pub const WRITE_MODE: u8 = 0x13;
    /// Closes a config transaction.
    pub const COMMIT: u8 = 0xf0;
    /// LCD clock: `00 01 5a YY MM DD hh mm ss` follows.
    pub const SET_CLOCK: u8 = 0x28;
}

/// Pages that follow a [`cmd::PER_KEY`] command.
pub const PER_KEY_PAGES: u8 = 8;
/// Pages returned after a [`cmd::READ_COLORS`] command.
pub const READ_PAGES: u8 = 9;
/// `[idx, R, G, B]` entries per 64-byte page.
pub const ENTRIES_PER_PAGE: usize = REPORT_LEN / 4;
/// Capacity of one per-key frame.
pub const MAX_ENTRIES: usize = PER_KEY_PAGES as usize * ENTRIES_PER_PAGE;

/// The vendor driver streams per-key frames 45–48 ms apart (~21 FPS), the same
/// ceiling the F75 has. Faster is untested and, on the F75, starves key
/// scanning — so treat it as a hard floor.
pub const MIN_FRAME_GAP_MS: u64 = 46;
pub const MAX_FPS: u32 = (1000 / MIN_FRAME_GAP_MS) as u32;

/// One command packet, ready for `send_feature_report`.
pub fn command(cmd: u8, arg: u8) -> [u8; PACKET_LEN] {
    let mut buf = [0u8; PACKET_LEN];
    buf[0] = REPORT_ID;
    buf[1] = PREFIX;
    buf[2] = cmd;
    buf[1 + ARG_OFFSET] = arg;
    buf
}

/// The 64-byte payload of a reply, whether or not the OS left the report ID
/// byte in front of it.
///
/// hidapi's backends disagree on this for unnumbered reports. Length decides
/// first: a full 65-byte buffer carries the ID, a full 64-byte one does not.
/// Only a truncated reply falls back to content — a command echo starts with
/// [`PREFIX`], so a leading `00 04` there is the report ID. (Content alone is
/// not safe on a colour page, whose first entry can legitimately be `00 04 …`.)
pub fn payload(resp: &[u8]) -> &[u8] {
    if resp.len() > REPORT_LEN && resp[0] == REPORT_ID {
        return &resp[1..];
    }
    if resp.len() < REPORT_LEN && resp.starts_with(&[REPORT_ID, PREFIX]) {
        return &resp[1..];
    }
    resp
}

/// Is this the device's echo of `cmd`, whatever its status byte says?
///
/// The echo alone identifies the command channel. The status byte (offset 3)
/// is `01` once the vendor driver has run its start-up sequence, but a freshly
/// connected board answers `00`, and `ff` until a config transaction has been
/// committed — so requiring `01` rejects a perfectly good keyboard.
pub fn is_echo(resp: &[u8], cmd: u8) -> bool {
    let p = payload(resp);
    p.len() >= 4 && p[0] == PREFIX && p[1] == cmd
}

/// The status byte of an echo, if `resp` is one.
pub fn status(resp: &[u8], cmd: u8) -> Option<u8> {
    is_echo(resp, cmd).then(|| payload(resp)[3])
}

/// Did the device accept `cmd` with the "ready" status?
pub fn is_ack(resp: &[u8], cmd: u8) -> bool {
    status(resp, cmd) == Some(ACK_OK)
}

/// Encode a per-key frame as the pages that follow [`cmd::PER_KEY`].
///
/// Entries are `(matrix index, [R, G, B])`. Unused slots stay zero, exactly as
/// the vendor driver sends them. Entries beyond [`MAX_ENTRIES`] are dropped.
pub fn per_key_pages(entries: &[(u8, [u8; 3])]) -> Vec<[u8; PACKET_LEN]> {
    let mut pages = vec![[0u8; PACKET_LEN]; PER_KEY_PAGES as usize];
    for (i, (idx, rgb)) in entries.iter().take(MAX_ENTRIES).enumerate() {
        let page = &mut pages[i / ENTRIES_PER_PAGE];
        let off = 1 + (i % ENTRIES_PER_PAGE) * 4;
        page[off] = *idx;
        page[off + 1..off + 4].copy_from_slice(rgb);
    }
    pages
}

/// Decode the pages returned after [`cmd::READ_COLORS`] into
/// `(matrix index, [R, G, B])`, in matrix order.
///
/// Brightness is applied before readback: a stored `ff 00 00` reads back as
/// `fa 00 00` or `fd 00 00` depending on the brightness level.
pub fn parse_colors(pages: &[Vec<u8>]) -> Vec<(u8, [u8; 3])> {
    pages
        .iter()
        .flat_map(|raw| {
            let p = payload(raw);
            let p = &p[..p.len().min(REPORT_LEN)];
            p.chunks_exact(4)
                .map(|e| (e[0], [e[1], e[2], e[3]]))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn only_the_ff13_collection_is_a_command_channel() {
        // The eight collections `keylux diagnose` listed on Windows.
        let seen = [
            (0x000c, 1, false),
            (0x0001, 1, false),
            (0xffff, 1, false),
            (0xff13, 3, true),
            (0xff68, 2, false),
            (0x0001, 0, false),
        ];
        for (page, iface, want) in seen {
            assert_eq!(
                is_command_collection(page, iface),
                want,
                "{page:#06x}/{iface}"
            );
        }
        assert!(is_command_collection(0, 3), "hidraw without usage pages");
    }

    #[test]
    fn per_key_header_matches_capture() {
        // 03_esc_blue.pcapng: 04 20 00 00 00 00 00 00 08
        let c = command(cmd::PER_KEY, PER_KEY_PAGES);
        assert_eq!(c.len(), 65);
        assert_eq!(hex(&c[..10]), "00042000000000000008");
        assert!(c[10..].iter().all(|&b| b == 0));
    }

    #[test]
    fn status_and_read_headers_match_capture() {
        assert_eq!(hex(&command(cmd::STATUS, 0)[1..3]), "0402");
        assert_eq!(
            hex(&command(cmd::READ_COLORS, READ_PAGES)[1..10]),
            "04f500000000000009"
        );
    }

    #[test]
    fn ack_parsing_tolerates_both_report_shapes() {
        let bare = [0x04, 0x20, 0x00, 0x01, 0, 0, 0, 0, 0x08];
        let mut with_id = vec![0x00];
        with_id.extend_from_slice(&bare);
        assert!(is_ack(&bare, cmd::PER_KEY));
        assert!(is_ack(&with_id, cmd::PER_KEY));
        // Wrong command, and the "busy" reply seen once in 00_open_app.
        assert!(!is_ack(&bare, cmd::STATUS));
        assert!(!is_ack(&[0x04, 0x02, 0x00, 0xff], cmd::STATUS));
        assert!(!is_ack(&[], cmd::STATUS));
    }

    #[test]
    fn a_not_ready_status_is_still_an_echo() {
        // What the board answered on hardware, 65 bytes with the report ID.
        let mut resp = vec![0u8; PACKET_LEN];
        resp[1..5].copy_from_slice(&[0x04, 0x02, 0x00, 0xff]);
        assert!(is_echo(&resp, cmd::STATUS));
        assert_eq!(status(&resp, cmd::STATUS), Some(0xff));
        assert!(!is_ack(&resp, cmd::STATUS));
        assert!(!is_echo(&resp, cmd::PER_KEY));
        // A colour page is not an echo.
        assert!(!is_echo(&[0x00, 0x00, 0x00, 0x00, 0x01, 0xfa], cmd::STATUS));
    }

    #[test]
    fn first_page_matches_capture() {
        // First page the vendor driver sent with Esc red and the rest off.
        let ids = [
            0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x13,
            0x14, 0x15,
        ];
        let entries: Vec<_> = ids
            .iter()
            .map(|&i| (i, if i == 1 { [0xfe, 0, 0] } else { [0, 0, 0] }))
            .collect();
        let pages = per_key_pages(&entries);
        assert_eq!(pages.len(), 8);
        assert_eq!(
            hex(&pages[0][1..]),
            "01fe0000020000000300000004000000050000000600000007000000080000000900000\
             00a0000000b0000000c0000000d000000130000001400000015000000"
        );
        assert!(pages[1..].iter().all(|p| p.iter().all(|&b| b == 0)));
    }

    #[test]
    fn overflow_is_dropped_not_panicking() {
        let entries = vec![(1u8, [1, 2, 3]); MAX_ENTRIES + 10];
        let pages = per_key_pages(&entries);
        assert_eq!(pages.len(), PER_KEY_PAGES as usize);
        assert_eq!(&pages[7][61..65], &[1, 1, 2, 3]);
    }

    #[test]
    fn readback_decodes_capture() {
        // First readback page from 01_all_red.pcapng.
        let raw: Vec<u8> = (0..16u8)
            .flat_map(|i| {
                let lit = (1..=0x0d).contains(&i);
                [i, if lit { 0xfa } else { 0 }, 0, 0]
            })
            .collect();
        let got = parse_colors(&[raw]);
        assert_eq!(got.len(), 16);
        assert_eq!(got[1], (1, [0xfa, 0, 0]));
        assert_eq!(got[0x0e], (0x0e, [0, 0, 0]));
    }
}
