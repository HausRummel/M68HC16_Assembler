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
            let addr = start + off as u32;
            // HEX.exe targets a fixed record byte-count of 0x23 (= addr + data +
            // checksum), so the data per record is 32 for an S1 (16-bit address)
            // and 31 for an S2 (24-bit address).
            let (rectype, alen) = if addr <= 0xFFFF { ('1', 2) } else { ('2', 3) };
            let max_data = 0x22 - alen;
            let chunk = &run[off..(off + max_data).min(run.len())];
            lines.push(record(rectype, alen, addr, chunk));
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
    fn splits_runs_into_32_byte_records() {
        // HEX.exe puts up to 32 data bytes in an S1 record (fixed count 0x23).
        let data: Vec<(u32, u8)> = (0..40u32).map(|i| (0x2000 + i, i as u8)).collect();
        let s = write_srecords(&data, "X");
        let s1 = s.lines().filter(|l| l.starts_with("S1")).count();
        assert_eq!(s1, 2, "40 bytes -> 32 + 8 = two S1 records");
        // First record carries 32 data bytes -> byte-count field 0x23.
        let first = s.lines().find(|l| l.starts_with("S1")).unwrap();
        assert!(first.starts_with("S1232000"), "got: {first}");
    }

    #[test]
    fn s2_records_hold_31_bytes() {
        // A 24-bit address needs an extra byte, so an S2 holds 31 data bytes to
        // keep the fixed 0x23 count.
        let data: Vec<(u32, u8)> = (0..31u32).map(|i| (0x20000 + i, i as u8)).collect();
        let s = write_srecords(&data, "X");
        let rec = s.lines().find(|l| l.starts_with("S2")).unwrap();
        assert!(rec.starts_with("S22302 0000".replace(' ', "").as_str()), "got: {rec}");
    }
}
