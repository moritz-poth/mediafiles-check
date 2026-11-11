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

/// What the header of an MXF file says about it. Every field is optional: files from other software (or older
/// Media Composers) may leave any of them out – nothing is guessed.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Info {
    /// file mob ID (essence container data)
    pub file_id: Option<MobId>,
    /// master clip mob ID (material package)
    pub master_id: Option<MobId>,
    /// name of the master clip
    pub clip: Option<String>,
    /// Some(true): a render (precompute); Some(false): media written by a known program without the render mark;
    /// None: nothing in the header says
    pub render: Option<bool>,
    /// product that wrote the file (identification set)
    pub writer: Option<String>,
    pub codec: Option<String>,
    /// picture width × height as displayed
    pub picture: Option<(u32, u32)>,
    /// edit rate (numerator, denominator)
    pub rate: Option<(u32, u32)>,
    /// length in edit units
    pub duration: Option<u64>,
    /// sound: sample rate (Hz), bits, channels
    pub sound: Option<(u32, u32, u32)>,
}

const IDENTIFICATION: u8 = 0x30;
const SOURCE_PACKAGE: u8 = 0x37;
const PICTURE: [u8; 3] = [0x27, 0x28, 0x29]; // generic picture, CDCI, RGBA
const MPEG_VIDEO: u8 = 0x51;
const SOUND: [u8; 3] = [0x42, 0x47, 0x48]; // generic sound, AES3, wave

/// Readable name of a picture essence coding (SMPTE registry; byte 7, the registry version, is ignored).
fn codec_name(ul: &[u8]) -> Option<String> {
    if ul.len() != 16 || ul[..4] != [0x06, 0x0e, 0x2b, 0x34] || ul[8..10] != [0x04, 0x01] {
        return None;
    }
    let tail = &ul[10..];
    Some(match tail {
        // VC-3: byte 13 = compression ID − 1234
        [0x02, 0x02, 0x71, n, ..] => match 1234 + *n as u32 {
            1270 => "DNxHR 444".into(),
            1271 => "DNxHR HQX".into(),
            1272 => "DNxHR HQ".into(),
            1273 => "DNxHR SQ".into(),
            1274 => "DNxHR LB".into(),
            cid => format!("DNxHD {cid}"),
        },
        [0x02, 0x02, 0x03, 0x06, n, ..] => match n {
            1 => "ProRes 422 Proxy",
            2 => "ProRes 422 LT",
            3 => "ProRes 422",
            4 => "ProRes 422 HQ",
            5 => "ProRes 4444",
            6 => "ProRes 4444 XQ",
            _ => "ProRes",
        }
        .into(),
        [0x02, 0x02, 0x01, 0x32, ..] => "H.264".into(),
        [0x02, 0x02, 0x01, ..] => "MPEG-2".into(),
        [0x02, 0x02, 0x02, 0x01, ..] => "DV".into(),
        [0x02, 0x02, 0x03, 0x01, ..] => "JPEG 2000".into(),
        [0x02, 0x01, ..] => "Uncompressed".into(),
        _ => return None,
    })
}

fn be32(v: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(v.get(..4)?.try_into().ok()?))
}

fn rational(v: &[u8]) -> Option<(u32, u32)> {
    Some((be32(v)?, be32(v.get(4..)?)?)).filter(|r| r.0 > 0 && r.1 > 0)
}

fn utf16(v: &[u8]) -> Option<String> {
    let units: Vec<u16> = v.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    Some(String::from_utf16_lossy(&units)).filter(|s| !s.trim().is_empty())
}

/// Everything `Info` holds, from the header bytes of an MXF file (one pass over the header metadata).
pub fn info(d: &[u8]) -> Result<Info, String> {
    if !same_key(d, &PARTITION, &[7]) {
        return Err("not an MXF file".into());
    }
    let mut out = Info::default();
    let (mut precompute_source, mut company) = (false, None::<String>);
    let mut layout = 0u8;
    let mut stored: (u32, u32) = (0, 0);
    let mut p = 0;
    while p + 16 < d.len() {
        let key = &d[p..p + 16];
        let Some((len, n)) = ber(d, p + 16) else { break };
        let start = p + 16 + n;
        let Some(end) = start.checked_add(len as usize).filter(|&e| e <= d.len()) else { break };
        if key[..4] == SET[..4] && ((key[4] == 0x01 && key[5] == 0x02) || (key[4] == 0x02 && key[5] == 0x53 && key[13] == 0x10)) {
            break;
        }
        let body = &d[start..end];
        if same_key(key, &SET, &[7, 14]) {
            let mut q = 0;
            while q + 4 <= body.len() {
                let tag = u16::from_be_bytes([body[q], body[q + 1]]);
                let l = u16::from_be_bytes([body[q + 2], body[q + 3]]) as usize;
                let Some(v) = body.get(q + 4..q + 4 + l) else { break };
                let kind = key[14];
                match (kind, tag) {
                    (ESSENCE_CONTAINER_DATA, 0x2701) if l == 32 => out.file_id = Some(to_mob_id(v)),
                    (MATERIAL_PACKAGE, 0x4401) if l == 32 => out.master_id = Some(to_mob_id(v)),
                    (MATERIAL_PACKAGE, 0x4402) => out.clip = utf16(v),
                    (SOURCE_PACKAGE, 0x4402) => precompute_source |= utf16(v).is_some_and(|n| n == "Precompute Source Mob"),
                    (IDENTIFICATION, 0x3c01) => company = company.take().or_else(|| utf16(v)),
                    (IDENTIFICATION, 0x3c02) => out.writer = out.writer.take().or_else(|| utf16(v)),
                    (k, 0x3201) if PICTURE.contains(&k) || k == MPEG_VIDEO => out.codec = codec_name(v),
                    (k, 0x3209) if PICTURE.contains(&k) || k == MPEG_VIDEO => out.picture = be32(v).map(|w| (w, out.picture.map_or(0, |p| p.1))),
                    (k, 0x3208) if PICTURE.contains(&k) || k == MPEG_VIDEO => out.picture = be32(v).map(|h| (out.picture.map_or(0, |p| p.0), h)),
                    (k, 0x3203) if PICTURE.contains(&k) || k == MPEG_VIDEO => stored.0 = be32(v).unwrap_or(0),
                    (k, 0x3202) if PICTURE.contains(&k) || k == MPEG_VIDEO => stored.1 = be32(v).unwrap_or(0),
                    (k, 0x320c) if PICTURE.contains(&k) || k == MPEG_VIDEO => layout = v.first().copied().unwrap_or(0),
                    (k, 0x3001) if PICTURE.contains(&k) || k == MPEG_VIDEO || SOUND.contains(&k) || k == 0x44 => out.rate = out.rate.or(rational(v)),
                    (k, 0x3002) if PICTURE.contains(&k) || k == MPEG_VIDEO || SOUND.contains(&k) => {
                        out.duration = v.get(..8).map(|b| u64::from_be_bytes(b.try_into().unwrap())).filter(|&d| d > 0 && d < u64::MAX / 2)
                    }
                    (k, 0x3d03) if SOUND.contains(&k) => out.sound = rational(v).map(|r| (r.0 / r.1, out.sound.map_or(0, |s| s.1), out.sound.map_or(0, |s| s.2))),
                    (k, 0x3d01) if SOUND.contains(&k) => out.sound = be32(v).map(|b| (out.sound.map_or(0, |s| s.0), b, out.sound.map_or(0, |s| s.2))),
                    (k, 0x3d07) if SOUND.contains(&k) => out.sound = be32(v).map(|c| (out.sound.map_or(0, |s| s.0), out.sound.map_or(0, |s| s.1), c)),
                    _ => {}
                }
                q += 4 + l;
            }
        }
        p = end;
    }
    // displayed size, else stored size; separate fields / segmented frame: the height is that of one field
    if out.picture.is_none_or(|p| p.0 == 0 || p.1 == 0) && stored.0 > 0 && stored.1 > 0 {
        out.picture = Some(stored);
    }
    if matches!(layout, 1 | 4) {
        out.picture = out.picture.map(|(w, h)| (w, h * 2));
    }
    out.picture = out.picture.filter(|p| p.0 > 0 && p.1 > 0);
    // "OP1a Muxer" says little without "FFmpeg"; Avid's product name already holds the company
    if let (Some(c), Some(w)) = (&company, &out.writer)
        && !w.contains(c.split_whitespace().next().unwrap_or(c))
    {
        out.writer = Some(format!("{c} {w}"));
    }
    // a render's source is the "Precompute Source Mob". Avid's usage mark (1 = precompute) is no proof: imported
    // graphics with alpha carry it too (checked on 187,000 real files, 2026-11-11)
    out.render = if precompute_source {
        Some(true)
    } else if company.is_some() || out.writer.is_some() {
        // only Media Composer makes renders and it always marks them; a file from another program is no render
        Some(false)
    } else {
        None
    };
    Ok(out)
}

/// `Info` of an MXF file, read from its header.
pub fn file_info(path: &Path) -> Result<Info, String> {
    info(&header(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn klv(key: [u8; 16], body: &[u8]) -> Vec<u8> {
        let mut d = key.to_vec();
        d.push(0x83);
        d.extend(&(body.len() as u32).to_be_bytes()[1..]);
        d.extend(body);
        d
    }

    fn set(kind: u8, items: &[(u16, &[u8])]) -> Vec<u8> {
        let mut key = SET;
        key[14] = kind;
        let mut body = vec![];
        for (tag, v) in items {
            body.extend(tag.to_be_bytes());
            body.extend((v.len() as u16).to_be_bytes());
            body.extend(*v);
        }
        klv(key, &body)
    }

    fn text(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    /// header of a Media Composer file: identification, material package, source package named `source`
    fn avid_header(source: Option<&str>, company: Option<&str>) -> Vec<u8> {
        let mut partition = PARTITION.to_vec();
        partition.extend([0x02, 0x04, 0x00]);
        let mut d = klv(partition.try_into().unwrap(), &[0; 88]);
        if let Some(c) = company {
            d.extend(set(IDENTIFICATION, &[(0x3c01, &text(c)), (0x3c02, &text("Avid Media Composer 26.8"))]));
        }
        let (name, umid) = (text("TEST,Dissolve+2"), [7u8; 32]);
        d.extend(set(MATERIAL_PACKAGE, &[(0x4401, &umid), (0x4402, &name)]));
        if let Some(s) = source {
            d.extend(set(SOURCE_PACKAGE, &[(0x4402, &text(s))]));
        }
        d
    }

    #[test]
    fn render_mark_and_writer() {
        let r = info(&avid_header(Some("Precompute Source Mob"), Some("Avid Technology, Inc."))).unwrap();
        assert_eq!((r.render, r.clip.as_deref(), r.writer.as_deref()), (Some(true), Some("TEST,Dissolve+2"), Some("Avid Media Composer 26.8")));
        assert!(r.master_id.is_some());
        // a graphic with alpha names its imported file; a Media Composer file without a render source is media
        assert_eq!(info(&avid_header(Some("Baum.png"), Some("Avid Technology, Inc."))).unwrap().render, Some(false));
        assert_eq!(info(&avid_header(None, Some("Avid Technology, Inc."))).unwrap().render, Some(false));
        // nothing says who wrote it: unknown, never guessed
        assert_eq!(info(&avid_header(None, None)).unwrap().render, None);
    }

    #[test]
    fn files_from_other_software() {
        // written by ffmpeg: no clip name, no duration, stored height 1088 shown as 1080
        let a = info(include_bytes!("../testdata/ff_opatom.hdr")).unwrap();
        assert_eq!((a.codec.as_deref(), a.picture, a.rate, a.clip, a.duration, a.render), (Some("DNxHR LB"), Some((1920, 1080)), Some((25, 1)), None, None, Some(false)));
        assert!(a.writer.unwrap().starts_with("FFmpeg"));
        let b = info(include_bytes!("../testdata/ff_op1a.hdr")).unwrap();
        assert_eq!((b.codec.as_deref(), b.sound), (Some("MPEG-2"), Some((48000, 24, 1))));
    }

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
