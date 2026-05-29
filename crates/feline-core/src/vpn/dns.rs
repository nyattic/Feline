use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use anyhow::{Result, anyhow, bail};

pub const TYPE_A: u16 = 1;
pub const TYPE_AAAA: u16 = 28;

pub fn build_query(id: u16, domain: &str, qtype: u16) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());

    for label in domain.trim_end_matches('.').split('.') {
        if label.is_empty() {
            bail!("empty DNS label in `{domain}`");
        }
        if label.len() > 63 {
            bail!("DNS label `{label}` exceeds 63 bytes");
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    Ok(buf)
}

pub fn parse_response(buf: &[u8], expected_id: u16) -> Result<Vec<IpAddr>> {
    if buf.len() < 12 {
        bail!("DNS response shorter than 12-byte header");
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    if id != expected_id {
        bail!("DNS ID mismatch: expected {expected_id:04x}, got {id:04x}");
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let rcode = flags & 0x0f;
    if rcode != 0 {
        bail!("DNS server returned RCODE {rcode}");
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    let ancount = u16::from_be_bytes([buf[6], buf[7]]);

    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(buf, pos)?;
        if pos + 4 > buf.len() {
            bail!("DNS question section truncated");
        }
        pos += 4;
    }

    let mut ips = Vec::new();
    for _ in 0..ancount {
        pos = skip_name(buf, pos)?;
        if pos + 10 > buf.len() {
            bail!("DNS answer record truncated");
        }
        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rdlen = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > buf.len() {
            bail!("DNS RDATA truncated");
        }
        let rdata = &buf[pos..pos + rdlen];
        match (rtype, rdlen) {
            (TYPE_A, 4) => {
                ips.push(IpAddr::V4(Ipv4Addr::new(
                    rdata[0], rdata[1], rdata[2], rdata[3],
                )));
            }
            (TYPE_AAAA, 16) => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(rdata);
                ips.push(IpAddr::V6(Ipv6Addr::from(octets)));
            }
            _ => {}
        }
        pos += rdlen;
    }

    if ips.is_empty() {
        return Err(anyhow!("DNS response had no A or AAAA records"));
    }
    Ok(ips)
}

fn skip_name(buf: &[u8], mut pos: usize) -> Result<usize> {
    loop {
        if pos >= buf.len() {
            bail!("DNS name extends past response");
        }
        let len = buf[pos];
        if len == 0 {
            return Ok(pos + 1);
        }
        if len & 0xc0 == 0xc0 {
            if pos + 2 > buf.len() {
                bail!("DNS name pointer truncated");
            }
            return Ok(pos + 2);
        }
        pos += 1 + len as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_query_with_correct_structure() {
        let q = build_query(0xabcd, "e621.net", TYPE_A).unwrap();
        assert_eq!(&q[0..2], &[0xab, 0xcd]);
        assert_eq!(&q[2..4], &[0x01, 0x00]);
        assert_eq!(&q[4..6], &[0x00, 0x01]);
        assert_eq!(q[12], 4);
        assert_eq!(&q[13..17], b"e621");
        assert_eq!(q[17], 3);
        assert_eq!(&q[18..21], b"net");
        assert_eq!(q[21], 0);
        assert_eq!(&q[22..24], &[0x00, 0x01]);
        assert_eq!(&q[24..26], &[0x00, 0x01]);
    }

    #[test]
    fn builds_aaaa_query_with_qtype_28() {
        let q = build_query(0x1234, "example.com", TYPE_AAAA).unwrap();
        let qtype = &q[q.len() - 4..q.len() - 2];
        assert_eq!(qtype, &[0x00, 0x1c]);
    }

    #[test]
    fn rejects_empty_label() {
        assert!(build_query(1, "foo..bar", TYPE_A).is_err());
    }

    #[test]
    fn parses_a_record_response() {
        let mut buf = vec![];
        buf.extend_from_slice(&[0xab, 0xcd]);
        buf.extend_from_slice(&[0x81, 0x80]);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0x00, 0x00]);
        buf.extend_from_slice(&[0x00, 0x00]);
        buf.push(4);
        buf.extend_from_slice(b"e621");
        buf.push(3);
        buf.extend_from_slice(b"net");
        buf.push(0);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0xc0, 0x0c]);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0x00, 0x01]);
        buf.extend_from_slice(&[0x00, 0x00, 0x01, 0x2c]);
        buf.extend_from_slice(&[0x00, 0x04]);
        buf.extend_from_slice(&[104, 17, 200, 50]);

        let ips = parse_response(&buf, 0xabcd).unwrap();
        assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(104, 17, 200, 50))]);
    }

    #[test]
    fn rejects_id_mismatch() {
        let mut buf = vec![0x00, 0x01];
        buf.extend_from_slice(&[0; 10]);
        let err = parse_response(&buf, 0x9999).unwrap_err().to_string();
        assert!(err.contains("ID mismatch"), "got: {err}");
    }

    #[test]
    fn rejects_nonzero_rcode() {
        let mut buf = vec![0x12, 0x34];
        buf.extend_from_slice(&[0x81, 0x83]);
        buf.extend_from_slice(&[0; 8]);
        let err = parse_response(&buf, 0x1234).unwrap_err().to_string();
        assert!(err.contains("RCODE"), "got: {err}");
    }
}
