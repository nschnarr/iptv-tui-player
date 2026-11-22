//! HDHomeRun UDP discovery (port 65001), so Plex/Jellyfin find the tuner
//! without entering its address by hand.
//!
//! Clients broadcast a discover request; we reply with our device ID, tuner
//! count and the HTTP base URL where the tuner actually lives. That URL can
//! point anywhere (e.g. a Kubernetes LoadBalancer IP), which lets a tiny
//! host-network responder advertise a tuner running elsewhere.
//!
//! Packet format (libhdhomerun): u16 type, u16 payload length (big endian),
//! TLV payload, then CRC32 (IEEE) of everything before it, little endian.

use std::net::UdpSocket;

pub const DISCOVERY_PORT: u16 = 65001;

const TYPE_DISCOVER_REQ: u16 = 0x0002;
const TYPE_DISCOVER_RPY: u16 = 0x0003;

const TAG_DEVICE_TYPE: u8 = 0x01;
const TAG_DEVICE_ID: u8 = 0x02;
const TAG_TUNER_COUNT: u8 = 0x10;
const TAG_LINEUP_URL: u8 = 0x27;
const TAG_BASE_URL: u8 = 0x2A;
const TAG_DEVICE_AUTH_STR: u8 = 0x2B;

const DEVICE_TYPE_TUNER: u32 = 0x0000_0001;
const WILDCARD: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone)]
pub struct Advertisement {
    pub device_id: u32,
    pub tuner_count: u8,
    /// e.g. `http://192.168.1.50:5004` (no trailing slash)
    pub base_url: String,
}

/// Parse an 8-hex-digit HDHomeRun device ID.
pub fn parse_device_id(s: &str) -> Option<u32> {
    if s.len() != 8 {
        return None;
    }
    u32::from_str_radix(s, 16).ok()
}

/// HDHomeRun device IDs carry a checksum; some clients ignore devices whose
/// ID fails it. Same algorithm as libhdhomerun's validate_device_id.
pub fn device_id_is_valid(id: u32) -> bool {
    const LOOKUP: [u8; 16] = [
        0xA, 0x5, 0xF, 0x6, 0x7, 0xC, 0x1, 0xB, 0x9, 0x2, 0x8, 0xD, 0x4, 0x3, 0xE, 0x0,
    ];
    let nib = |shift: u32| ((id >> shift) & 0xF) as u8;
    let checksum = LOOKUP[nib(28) as usize]
        ^ nib(24)
        ^ LOOKUP[nib(20) as usize]
        ^ nib(16)
        ^ LOOKUP[nib(12) as usize]
        ^ nib(8)
        ^ LOOKUP[nib(4) as usize]
        ^ nib(0);
    checksum == 0
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn push_tlv(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    let len = value.len();
    if len <= 127 {
        out.push(len as u8);
    } else {
        out.push((len as u8 & 0x7F) | 0x80);
        out.push((len >> 7) as u8);
    }
    out.extend_from_slice(value);
}

fn frame(pkt_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&pkt_type.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Validate framing + CRC; returns (type, payload).
fn unframe(pkt: &[u8]) -> Option<(u16, &[u8])> {
    if pkt.len() < 8 {
        return None;
    }
    let pkt_type = u16::from_be_bytes([pkt[0], pkt[1]]);
    let len = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    if pkt.len() != 4 + len + 4 {
        return None;
    }
    let (body, crc) = pkt.split_at(4 + len);
    if crc32(body) != u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]]) {
        return None;
    }
    Some((pkt_type, &body[4..]))
}

fn tlvs(mut payload: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    while payload.len() >= 2 {
        let tag = payload[0];
        let (len, hdr) = if payload[1] & 0x80 != 0 {
            if payload.len() < 3 {
                break;
            }
            (
                ((payload[1] & 0x7F) as usize) | ((payload[2] as usize) << 7),
                3,
            )
        } else {
            (payload[1] as usize, 2)
        };
        if payload.len() < hdr + len {
            break;
        }
        out.push((tag, &payload[hdr..hdr + len]));
        payload = &payload[hdr + len..];
    }
    out
}

fn tlv_u32(v: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(v.try_into().ok()?))
}

/// Build a discover request (used by tests and handy for debugging).
pub fn discover_request(device_type: u32, device_id: u32) -> Vec<u8> {
    let mut p = Vec::new();
    push_tlv(&mut p, TAG_DEVICE_TYPE, &device_type.to_be_bytes());
    push_tlv(&mut p, TAG_DEVICE_ID, &device_id.to_be_bytes());
    frame(TYPE_DISCOVER_REQ, &p)
}

/// Return our reply if `pkt` is a discover request we should answer.
pub fn handle_packet(pkt: &[u8], ad: &Advertisement) -> Option<Vec<u8>> {
    let (pkt_type, payload) = unframe(pkt)?;
    if pkt_type != TYPE_DISCOVER_REQ {
        return None;
    }
    for (tag, value) in tlvs(payload) {
        match tag {
            TAG_DEVICE_TYPE => {
                let t = tlv_u32(value)?;
                if t != WILDCARD && t != DEVICE_TYPE_TUNER {
                    return None;
                }
            }
            TAG_DEVICE_ID => {
                let id = tlv_u32(value)?;
                if id != WILDCARD && id != ad.device_id {
                    return None;
                }
            }
            _ => {}
        }
    }
    let mut p = Vec::new();
    push_tlv(&mut p, TAG_DEVICE_TYPE, &DEVICE_TYPE_TUNER.to_be_bytes());
    push_tlv(&mut p, TAG_DEVICE_ID, &ad.device_id.to_be_bytes());
    push_tlv(&mut p, TAG_TUNER_COUNT, &[ad.tuner_count]);
    push_tlv(&mut p, TAG_BASE_URL, ad.base_url.as_bytes());
    push_tlv(
        &mut p,
        TAG_LINEUP_URL,
        format!("{}/lineup.json", ad.base_url).as_bytes(),
    );
    push_tlv(
        &mut p,
        TAG_DEVICE_AUTH_STR,
        format!("{:08X}", ad.device_id).as_bytes(),
    );
    Some(frame(TYPE_DISCOVER_RPY, &p))
}

/// Answer discovery requests forever on `bind:65001`.
pub fn serve(bind: &str, ad: &Advertisement) -> std::io::Result<()> {
    let sock = UdpSocket::bind((bind, DISCOVERY_PORT))?;
    let mut buf = [0u8; 1500];
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if let Some(reply) = handle_packet(&buf[..n], ad) {
            eprintln!("[discovery] Answered {from}");
            if let Err(e) = sock.send_to(&reply, from) {
                eprintln!("[discovery] Reply to {from} failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ad() -> Advertisement {
        Advertisement {
            device_id: 0x1F7E5E07,
            tuner_count: 2,
            base_url: "http://192.168.1.50:5004".into(),
        }
    }

    #[test]
    fn crc_matches_ieee_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn answers_wildcard_request() {
        let reply = handle_packet(&discover_request(WILDCARD, WILDCARD), &ad()).unwrap();
        let (t, payload) = unframe(&reply).unwrap();
        assert_eq!(t, TYPE_DISCOVER_RPY);
        let tags = tlvs(payload);
        let get = |tag| {
            tags.iter()
                .find(|(t, _)| *t == tag)
                .map(|(_, v)| *v)
                .unwrap()
        };
        assert_eq!(tlv_u32(get(TAG_DEVICE_ID)), Some(0x1F7E5E07));
        assert_eq!(get(TAG_TUNER_COUNT), &[2]);
        assert_eq!(get(TAG_BASE_URL), b"http://192.168.1.50:5004");
        assert_eq!(get(TAG_LINEUP_URL), b"http://192.168.1.50:5004/lineup.json");
    }

    #[test]
    fn answers_request_for_our_id_only() {
        assert!(handle_packet(&discover_request(DEVICE_TYPE_TUNER, 0x1F7E5E07), &ad()).is_some());
        assert!(handle_packet(&discover_request(DEVICE_TYPE_TUNER, 0x12345678), &ad()).is_none());
        // Storage devices (type 5) aren't us.
        assert!(handle_packet(&discover_request(5, WILDCARD), &ad()).is_none());
    }

    #[test]
    fn rejects_bad_crc_and_garbage() {
        let mut req = discover_request(WILDCARD, WILDCARD);
        *req.last_mut().unwrap() ^= 0xFF;
        assert!(handle_packet(&req, &ad()).is_none());
        assert!(handle_packet(b"hello", &ad()).is_none());
    }

    #[test]
    fn long_tlv_length_roundtrips() {
        let mut p = Vec::new();
        let long = vec![b'x'; 300];
        push_tlv(&mut p, TAG_BASE_URL, &long);
        let parsed = tlvs(&p);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].1.len(), 300);
    }

    #[test]
    fn device_id_checksum() {
        // 1F7E5E07: last nibble chosen so the checksum XORs to zero.
        assert!(device_id_is_valid(0x1F7E5E07));
        assert!(!device_id_is_valid(0x12345678));
        assert_eq!(parse_device_id("1f7e5e07"), Some(0x1F7E5E07));
        assert_eq!(parse_device_id("xyz"), None);
    }
}
