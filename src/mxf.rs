//! Reads the mob ID of an Avid MXF media file from its header, for files that are in no Avid database.
//!
//! MXF (SMPTE 377M) starts with the header partition pack, followed by the header metadata as KLV local sets.
//! The essence container data set (tag 0x2701, LinkedPackageUID) names the package of the media in the file –
//! for Avid OP-Atom files that is the file source package, whose UMID is the file mob ID of the bins and of
//! msmFMID.pmr. The material package (tag 0x4401, PackageUID) is the master clip – shared by the video and audio
//! files of one clip, like the second ID in msmFMID.pmr. Only the header is read (normally a few KB, Media Composer
//! writes up to ~100 KB).
use crate::avb::MobId;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Header metadata set keys share everything but byte 14 (byte 7 is the registry version and is ignored).
pub(crate) const SET: [u8; 16] = [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x53, 0x01, 0x00, 0x0d, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00];
pub(crate) const ESSENCE_CONTAINER_DATA: u8 = 0x23;
pub(crate) const MATERIAL_PACKAGE: u8 = 0x36;
pub(crate) const PARTITION: [u8; 13] = [0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x00, 0x0d, 0x01, 0x02, 0x01, 0x01];
/// stop reading a header that claims to be larger than this
const MAX_HEADER: u64 = 16 << 20;

fn same_key(key: &[u8], pattern: &[u8], skip: &[usize]) -> bool {
    pattern.iter().enumerate().all(|(i, b)| skip.contains(&i) || key.get(i) == Some(b))
}

/// BER length at `p`: (length, bytes used).
fn ber(d: &[u8], p: usize) -> Option<(u64, usize)> {
    let first = *d.get(p)?;
    if first < 0x80 {
        return Some((first as u64, 1));
    }
    let n = (first & 0x7f) as usize;
    if n == 0 || n > 8 {
        return None;
    }
    let bytes = d.get(p + 1..p + 1 + n)?;
    Some((bytes.iter().fold(0u64, |a, &b| a << 8 | b as u64), 1 + n))
}

/// MXF UMID → our mob ID layout: the material number's first three fields are big-endian in MXF,
/// little-endian in bins and msmFMID.pmr.
fn to_mob_id(umid: &[u8]) -> MobId {
    let mut id: MobId = umid.try_into().unwrap();
    id[16..20].reverse();
    id[20..22].reverse();
    id[22..24].reverse();
    id
}

/// File mob ID from the header bytes of an MXF file.
pub fn parse(d: &[u8]) -> Result<MobId, String> {
    find_set(d, ESSENCE_CONTAINER_DATA, 0x2701)?.ok_or_else(|| "essence container data without package".into())
}

/// Master clip mob ID (material package) from the header bytes of an MXF file.
pub fn parse_master(d: &[u8]) -> Result<MobId, String> {
    find_set(d, MATERIAL_PACKAGE, 0x4401)?.ok_or_else(|| "material package without ID".into())
}

/// The UMID under `tag` in the first header set of kind `kind`; Ok(None) if that set has no such UMID.
fn find_set(d: &[u8], kind: u8, want: u16) -> Result<Option<MobId>, String> {
    if !same_key(d, &PARTITION, &[7]) {
        return Err("not an MXF file".into());
    }
    let mut p = 0;
    while p + 16 < d.len() {
        let key = &d[p..p + 16];
        let (len, n) = ber(d, p + 16).ok_or("bad KLV length")?;
        let start = p + 16 + n;
        let end = start.checked_add(len as usize).filter(|&e| e <= d.len());
        if same_key(key, &SET, &[7, 14]) && key[14] == kind {
            let end = end.ok_or("header ends too early")?;
            let mut q = start;
            while q + 4 <= end {
                let tag = u16::from_be_bytes([d[q], d[q + 1]]);
                let l = u16::from_be_bytes([d[q + 2], d[q + 3]]) as usize;
                if tag == want && l == 32 && q + 4 + 32 <= end {
                    return Ok(Some(to_mob_id(&d[q + 4..q + 36])));
                }
                q += 4 + l;
            }
            return Ok(None);
        }
        // essence or index data: the header metadata is over
        // (essence element 06 0e 2b 34 01 02 …, index table segment …0d 01 02 01 01 10 – not KLV fill 01 01 …)
        if key[..4] == SET[..4] && ((key[4] == 0x01 && key[5] == 0x02) || (key[4] == 0x02 && key[5] == 0x53 && key[13] == 0x10)) {
            break;
        }
        match end {
            Some(e) => p = e,
            None => break,
        }
    }
    Err(if kind == MATERIAL_PACKAGE { "no material package in the header" } else { "no essence container data in the header" }.into())
}

/// File mob ID of an MXF file, read from its header.
pub fn file_mob_id(path: &Path) -> Result<MobId, String> {
    parse(&header(path)?)
}

/// Master clip mob ID of an MXF file, read from its header.
pub fn master_mob_id(path: &Path) -> Result<MobId, String> {
    parse_master(&header(path)?)
}

fn header(path: &Path) -> Result<Vec<u8>, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut d = vec![0u8; 64 << 10];
    let n = read_full(&mut f, &mut d)?;
    d.truncate(n);
    // partition pack: key, length, then major, minor, KAG (8 bytes) and four u64 – the fourth is HeaderByteCount
    if let Some((pack, l)) = ber(&d, 16) {
        let at = 16 + l + 8 + 24;
        if let Some(b) = d.get(at..at + 8) {
            let header = u64::from_be_bytes(b.try_into().unwrap());
            let want = (16 + l as u64 + pack + header).min(MAX_HEADER) as usize;
            if want > d.len() && n == 64 << 10 {
                let have = d.len();
                d.resize(want, 0);
                let more = read_full(&mut f, &mut d[have..])?;
                d.truncate(have + more);
            }
        }
    }
    Ok(d)
}

fn read_full(f: &mut File, buf: &mut [u8]) -> Result<usize, String> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_linked_package_of_the_essence_container_data() {
        let mut umid = [0u8; 32];
        umid[..12].copy_from_slice(&[6, 0x0a, 0x2b, 0x34, 1, 1, 1, 1, 1, 1, 0x0f, 0]);
        umid[12] = 0x13;
        umid[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let d = crate::selftest::sample_mxf(&umid);
        let id = parse(&d).unwrap();
        assert_eq!(id[..16], umid[..16]);
        assert_eq!(id[16..24], [4, 3, 2, 1, 6, 5, 8, 7]);
        assert!(parse(&d[..d.len() - 10]).is_err());
        assert!(parse(b"not mxf at all, really not").is_err());
    }
}
