//! Motorola S-record writer.
//!
//! Emits an `S0` header, `S1`/`S2` data records (16- or 24-bit addresses, up to
//! 16 data bytes each, matching HEX.exe's record sizing), and an `S9` terminator.
//! Records use CRLF line endings as the DOS toolchain does.

/// Build one S-record line. `rectype` is the digit ('0','1','2','9', …).
fn record(rectype: char, addr_bytes: usize, addr: u32, data: &[u8]) -> String {
    let mut payload = Vec::with_capacity(addr_bytes + data.len() + 1);
    payload.push((addr_bytes + data.len() + 1) as u8); // byte count: addr + data + checksum
    for i in (0..addr_bytes).rev() {
        payload.push((addr >> (8 * i)) as u8);
    }
    payload.extend_from_slice(data);
    let sum = payload.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    let checksum = !sum;

    let mut s = String::with_capacity(4 + payload.len() * 2);
    s.push('S');
    s.push(rectype);
    for b in &payload {
        s.push_str(&format!("{b:02X}"));
    }
    s.push_str(&format!("{checksum:02X}"));
    s
}

/// Render `(address, byte)` data as a full S-record file. `module` names the
/// `S0` header record.
pub fn write_srecords(data: &[(u32, u8)], module: &str) -> String {
    let mut sorted = data.to_vec();
    sorted.sort_by_key(|(a, _)| *a);

    let mut lines = vec![record('0', 2, 0, module.as_bytes())];

    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i].0;
        let mut run = vec![sorted[i].1];
        let mut j = i + 1;
        while j < sorted.len() && sorted[j].0 == sorted[j - 1].0 + 1 {
            run.push(sorted[j].1);
            j += 1;
        }
        let mut off = 0;
        while off < run.len() {
            let chunk = &run[off..(off + 16).min(run.len())];
            let addr = start + off as u32;
            if addr <= 0xFFFF {
                lines.push(record('1', 2, addr, chunk));
            } else {
                lines.push(record('2', 3, addr, chunk));
            }
            off += chunk.len();
        }
        i = j;
    }

    lines.push(record('9', 2, 0, &[]));
    lines.join("\r\n") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_record_checksum() {
        // S1, count=05, addr=2000, data=7512, checksum=53.
        let s = write_srecords(&[(0x2000, 0x75), (0x2001, 0x12)], "X");
        assert!(s.contains("S1052000751253"), "got: {s}");
        assert!(s.starts_with("S0"));
        assert!(s.trim_end().ends_with("S9030000FC"));
    }

    #[test]
    fn splits_runs_into_16_byte_records() {
        let data: Vec<(u32, u8)> = (0..20u32).map(|i| (0x2000 + i, i as u8)).collect();
        let s = write_srecords(&data, "X");
        let s1 = s.lines().filter(|l| l.starts_with("S1")).count();
        assert_eq!(s1, 2, "20 bytes -> 16 + 4 = two S1 records");
    }
}
