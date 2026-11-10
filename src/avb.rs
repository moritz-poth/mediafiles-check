//! Reads Avid bins (.avb) – only what is needed to find out which media files the clips of a bin use.
//!
//! Format knowledge from pyavb by Mark Reid (MIT license, https://github.com/markreidvfx/pyavb), see THIRD-PARTY.md.
//! A bin is a table of objects (class id + size + data) that refer to each other by index. Only these are parsed:
//! mobs (CMPO), source clips (SCLP), sequences (SEQU), every track group (their track list comes first, so the
//! prefix is enough), media descriptors and the class of their locator. Every other object is skipped unread,
//! so effects or new object types Avid adds do not stop the reader.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Avid mob ID as 32 bytes in the layout of msmFMID.pmr / MXF: 12-byte label, length, 3-byte instance,
/// 16-byte material (first three fields little-endian).
pub type MobId = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MobKind {
    /// sequence (and other compositions)
    Composition,
    /// master clip or subclip
    Master,
    /// file, tape or import source
    Source,
    Other(u8),
}

#[derive(Debug, Clone)]
pub struct Mob {
    pub name: String,
    pub id: MobId,
    pub kind: MobKind,
    /// 2 = subclip (OMF usage codes)
    pub usage: i32,
    /// class id of the media descriptor (source mobs), e.g. "CDCI", "PCMA", "MULD", "MDTP"
    pub descriptor: Option<String>,
    /// class id of the descriptor's locator, e.g. "MSML" (managed media) or "FILE" / "URLL" (linked)
    pub locator: Option<String>,
    /// the descriptor names an AMA plugin (acfUID) – a linked file, not Avid-managed media
    pub ama: bool,
    tracks: Vec<u32>,
    /// track number of each entry in `tracks` (V1 and A1 both have 1; the media kind tells them apart)
    track_numbers: Vec<Option<u16>>,
}

impl Mob {
    /// Source mob of a media file (not a tape, film or import source).
    pub fn is_file(&self) -> bool {
        const FILE_DESCRIPTORS: [&str; 13] = ["MDFL", "MULD", "WAVE", "AIFC", "PCMA", "MPGA", "DIDD", "CDCI", "MPGI", "JPED", "RGBA", "DATD", "ANCD"];
        self.kind == MobKind::Source && self.descriptor.as_deref().is_some_and(|d| FILE_DESCRIPTORS.contains(&d))
    }
}

#[derive(Debug, Clone)]
enum Part {
    /// source clip: the mob it uses and the track number in that mob
    Clip(MobId, u16),
    /// sequence or track group: the parts below it
    Parts(Vec<u32>),
}

#[derive(Debug)]
pub struct Bin {
    /// Media Composer version that saved the bin
    pub creator: String,
    pub mobs: Vec<Mob>,
    /// indexes into `mobs` of what the bin shows (clips, subclips, sequences)
    pub items: Vec<usize>,
    parts: HashMap<u32, Part>,
    /// media kind of every parsed component, by object index
    kinds: HashMap<u32, u16>,
    by_id: HashMap<MobId, usize>,
}

const TRACK_GROUPS: [&[u8; 4]; 14] = [b"TRKG", b"TKFX", b"PVOL", b"ASPI", b"EQMB", b"WARP", b"MASK", b"STRB", b"SPED", b"REPT", b"RSET", b"TNFX", b"SLCT", b"CMPO"];

struct R<'a> {
    d: &'a [u8],
    p: usize,
    le: bool,
}

type Res<T> = Result<T, String>;

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.d.len()).ok_or("object ends too early")?;
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn peek(&self) -> Option<u8> {
        self.d.get(self.p).copied()
    }
    fn u16(&mut self) -> Res<u16> {
        let b: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }
    fn u32(&mut self) -> Res<u32> {
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }
    fn tag(&mut self, want: u8) -> Res<()> {
        let got = self.u8()?;
        if got == want { Ok(()) } else { Err(format!("unexpected marker {got} (expected {want}) at {}", self.p - 1)) }
    }
    /// Extension blocks: 0x01 followed by a tag number, as long as they come.
    fn ext(&mut self) -> Res<Option<u8>> {
        if self.peek() == Some(0x01) {
            self.p += 1;
            return Ok(Some(self.u8()?));
        }
        Ok(None)
    }
    fn string(&mut self) -> Res<String> {
        let n = self.u16()?;
        if n == 0xFFFF {
            return Ok(String::new());
        }
        let raw = self.take(n as usize)?;
        let raw: Vec<u8> = raw.iter().copied().filter(|&b| b != 0).collect();
        Ok(match std::str::from_utf8(&raw) {
            // names are Mac Roman; pure ASCII and valid UTF-8 read the same way
            Ok(s) if raw.is_ascii() => s.to_string(),
            _ => mac_roman(&raw),
        })
    }
    fn mob_id(&mut self) -> Res<MobId> {
        let mut id = [0u8; 32];
        self.tag(65)?;
        if self.u32()? != 12 {
            return Err("unexpected mob id label length".into());
        }
        id[..12].copy_from_slice(self.take(12)?);
        for i in 12..16 {
            self.tag(68)?;
            id[i] = self.u8()?;
        }
        self.tag(72)?;
        let mut a: [u8; 4] = self.take(4)?.try_into().unwrap();
        self.tag(70)?;
        let mut b: [u8; 2] = self.take(2)?.try_into().unwrap();
        self.tag(70)?;
        let mut c: [u8; 2] = self.take(2)?.try_into().unwrap();
        self.tag(65)?;
        if self.u32()? != 8 {
            return Err("unexpected mob id length".into());
        }
        if !self.le {
            a.reverse();
            b.reverse();
            c.reverse();
        }
        id[16..20].copy_from_slice(&a);
        id[20..22].copy_from_slice(&b);
        id[22..24].copy_from_slice(&c);
        id[24..32].copy_from_slice(self.take(8)?);
        Ok(id)
    }
}

/// Mac Roman upper half (0x80–0xFF).
pub(crate) fn mac_roman(b: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»… ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    let high: Vec<char> = HIGH.chars().collect();
    b.iter().map(|&c| if c < 0x80 { c as char } else { high.get(c as usize - 0x80).copied().unwrap_or('?') }).collect()
}

/// Component header shared by clips, sequences and track groups; returns the name and the media kind.
fn component(r: &mut R) -> Res<(String, u16)> {
    r.tag(0x02)?;
    r.tag(0x03)?;
    r.u32()?; // left bob
    r.u32()?; // right bob
    let kind = r.u16()?;
    r.u32()?; // edit rate mantissa
    r.u16()?; // edit rate exponent
    let name = r.string()?;
    r.string()?; // effect id
    r.u32()?; // attributes
    r.u32()?; // session attributes
    r.u32()?; // precomputed
    while let Some(t) = r.ext()? {
        match t {
            0x01 => {
                r.tag(72)?;
                r.u32()?; // parameter list
            }
            _ => return Err(format!("unknown component extension {t}")),
        }
    }
    Ok((name, kind))
}

/// Track group part after the component header: track number and component of every track.
fn track_group(r: &mut R) -> Res<Vec<(Option<u16>, u32)>> {
    r.tag(0x02)?;
    r.tag(0x08)?;
    r.u8()?; // mc mode
    r.u32()?; // length
    r.u32()?; // number of scalars
    let count = r.u32()?;
    let mut parts = vec![];
    for _ in 0..count {
        let flags = r.u16()?;
        if flags & 0xFC00 != 0 {
            return Err(format!("unknown track flags {flags:#x}"));
        }
        let index = if flags & 1 != 0 { Some(r.u16()?) } else { None };
        if flags & 2 != 0 {
            r.u32()?; // attributes
        }
        if flags & (1 << 9) != 0 {
            r.u32()?; // session attributes
        }
        if flags & 4 != 0 {
            parts.push((index, r.u32()?));
        }
        for bit in [3, 4] {
            if flags & (1 << bit) != 0 {
                r.u32()?; // filler proxy, bob
            }
        }
        for bit in [5, 6] {
            if flags & (1 << bit) != 0 {
                r.u16()?; // control code, sub code
            }
        }
        if flags & (1 << 7) != 0 {
            r.u32()?; // start position
        }
        if flags & (1 << 8) != 0 {
            r.u8()?; // read only
        }
    }
    while let Some(t) = r.ext()? {
        match t {
            0x01 => {
                for _ in 0..count {
                    r.tag(69)?;
                    r.u16()?; // lock number
                }
            }
            _ => return Err(format!("unknown track group extension {t}")),
        }
    }
    Ok(parts)
}

struct Raw {
    mob: Option<(Mob, u32)>, // mob and its descriptor index
    part: Option<Part>,
    /// media kind of a parsed component (1 picture, 2 sound, ...)
    kind: Option<u16>,
    /// descriptor class, locator index, AMA plugin id present
    descriptor: Option<(String, u32, bool)>,
}

fn parse_object(class: &[u8; 4], d: &[u8], le: bool) -> Res<Raw> {
    let mut r = R { d, p: 0, le };
    let mut raw = Raw { mob: None, part: None, kind: None, descriptor: None };
    if class == b"SCLP" {
        raw.kind = Some(component(&mut r)?.1);
        r.tag(0x02)?;
        r.tag(0x01)?;
        r.u32()?; // length
        r.tag(0x02)?;
        r.tag(0x03)?;
        r.u32()?;
        r.u32()?; // old short mob id
        let track = r.u16()?;
        r.u32()?; // start
        let mut id = None;
        while let Some(t) = r.ext()? {
            match t {
                0x01 => id = Some(r.mob_id()?),
                _ => return Err(format!("unknown source clip extension {t}")),
            }
        }
        if let Some(id) = id {
            raw.part = Some(Part::Clip(id, track));
        }
    } else if class == b"SEQU" {
        raw.kind = Some(component(&mut r)?.1);
        r.tag(0x02)?;
        r.tag(0x03)?;
        let n = r.u32()?;
        raw.part = Some(Part::Parts((0..n).map(|_| r.u32()).collect::<Res<_>>()?));
    } else if TRACK_GROUPS.contains(&class) {
        let (name, kind) = component(&mut r)?;
        raw.kind = Some(kind);
        let (track_numbers, tracks): (Vec<Option<u16>>, Vec<u32>) = track_group(&mut r)?.into_iter().unzip();
        if class == b"CMPO" {
            r.tag(0x02)?;
            r.tag(0x02)?;
            r.u32()?;
            r.u32()?; // old short mob id
            r.u32()?; // last modified
            let kind = match r.u8()? {
                1 => MobKind::Composition,
                2 => MobKind::Master,
                3 => MobKind::Source,
                k => MobKind::Other(k),
            };
            let usage = r.u32()? as i32;
            let descriptor = r.u32()?;
            let mut id = None;
            while let Some(t) = r.ext()? {
                match t {
                    0x01 => {
                        r.tag(71)?;
                        r.u32()?; // creation time
                    }
                    0x02 => id = Some(r.mob_id()?),
                    _ => return Err(format!("unknown mob extension {t}")),
                }
            }
            let id = id.ok_or("mob without mob id")?;
            raw.mob = Some((Mob { name, id, kind, usage, descriptor: None, locator: None, ama: false, tracks, track_numbers }, descriptor));
        } else {
            raw.part = Some(Part::Parts(tracks));
        }
    } else if class.starts_with(b"MD") || [b"MULD", b"WAVE", b"AIFC", b"PCMA", b"MPGA", b"DIDD", b"CDCI", b"MPGI", b"JPED", b"RGBA", b"DATD", b"ANCD"].contains(&class) {
        // every descriptor starts with the MDES part: mob kind, locator, ...
        r.tag(0x02)?;
        r.tag(0x03)?;
        r.u8()?;
        let locator = r.u32()?;
        // then intermediate, physical media and extensions; 0x01 = acfUID of an AMA plugin. A layout this reader
        // does not know only loses the AMA mark, never the bin.
        let ama = (|| -> Res<bool> {
            r.u8()?;
            r.u32()?;
            while let Some(t) = r.ext()? {
                match t {
                    0x01 => return Ok(true),
                    0x02 => {
                        r.tag(65)?;
                        let n = r.u32()? as usize;
                        r.take(n)?;
                    }
                    0x03 => {
                        r.tag(72)?;
                        r.u32()?;
                    }
                    _ => return Ok(false),
                }
            }
            Ok(false)
        })()
        .unwrap_or(false);
        raw.descriptor = Some((String::from_utf8_lossy(class).into_owned(), locator, ama));
    }
    Ok(raw)
}

impl Bin {
    pub fn read(path: &Path) -> Res<Bin> {
        Bin::parse(&fs::read(path).map_err(|e| e.to_string())?)
    }

    pub fn parse(data: &[u8]) -> Res<Bin> {
        let le = match data.get(..2) {
            Some([0x06, 0x00]) => true,
            Some([0x00, 0x06]) => false,
            _ => return Err("not an Avid bin".into()),
        };
        let mut r = R { d: data, p: 2, le };
        if r.take(6)? != b"Domain" {
            return Err("not an Avid bin".into());
        }
        let fourcc = |r: &mut R| -> Res<[u8; 4]> {
            let mut c: [u8; 4] = r.take(4)?.try_into().unwrap();
            if r.le {
                c.reverse();
            }
            Ok(c)
        };
        if &fourcc(&mut r)? != b"OBJD" || r.string()? != "AObjDoc" || r.u8()? != 4 {
            return Err("unknown bin header".into());
        }
        r.string()?; // last save as text
        let count = r.u32()? as usize;
        let root = r.u32()?;
        r.u32()?; // byte order marker
        r.u32()?; // last save
        r.u32()?;
        if &fourcc(&mut r)? != b"ATob" || &fourcc(&mut r)? != b"ATve" {
            return Err("unknown bin header".into());
        }
        let creator = r.string()?.trim().to_string();
        r.take(16)?;

        // object table: index 1.. → (class, data)
        let mut objects: Vec<([u8; 4], &[u8])> = vec![(*b"    ", &[])];
        for _ in 0..count {
            let class = fourcc(&mut r)?;
            let size = r.u32()? as usize;
            objects.push((class, r.take(size)?));
        }

        let mut parts = HashMap::new();
        let mut mobs = vec![];
        let mut mob_at = HashMap::new();
        let mut descriptors = HashMap::new();
        let mut kinds = HashMap::new();
        for (i, (class, d)) in objects.iter().enumerate().skip(1) {
            let raw = parse_object(class, d, le).map_err(|e| format!("object {i} ({}): {e}", String::from_utf8_lossy(class)))?;
            if let Some(p) = raw.part {
                parts.insert(i as u32, p);
            }
            if let Some(k) = raw.kind {
                kinds.insert(i as u32, k);
            }
            if let Some(d) = raw.descriptor {
                descriptors.insert(i as u32, d);
            }
            if let Some(m) = raw.mob {
                mob_at.insert(i as u32, mobs.len());
                mobs.push(m);
            }
        }
        let class_of = |i: u32| objects.get(i as usize).filter(|_| i != 0).map(|o| String::from_utf8_lossy(&o.0).into_owned());
        let mobs: Vec<Mob> = mobs
            .into_iter()
            .map(|(mut m, d)| {
                if let Some((class, locator, ama)) = descriptors.get(&d) {
                    m.descriptor = Some(class.clone());
                    m.locator = class_of(*locator);
                    m.ama = *ama;
                }
                m
            })
            .collect();
        // a mob's tracks are parts too (mobs are track groups)
        for (&i, &m) in &mob_at {
            parts.insert(i, Part::Parts(mobs[m].tracks.clone()));
        }

        // the bin object lists what the bin shows
        let (rclass, rdata) = objects.get(root as usize).ok_or("root object missing")?;
        if rclass != b"ABIN" && rclass != b"BINF" {
            return Err(format!("root object is {}", String::from_utf8_lossy(rclass)));
        }
        let mut b = R { d: rdata, p: 0, le };
        b.tag(0x02)?;
        let version = b.u8()?;
        b.u32()?; // view setting
        b.take(8)?; // uid
        let n = match version {
            0x0e => b.u16()? as usize,
            0x0f => b.u32()? as usize,
            v => return Err(format!("unknown bin version {v}")),
        };
        let mut items = vec![];
        for _ in 0..n {
            let obj = b.u32()?;
            b.take(9)?; // x, y, keyframe, user placed
            if let Some(&m) = mob_at.get(&obj) {
                items.push(m);
            }
        }
        let by_id = mobs.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
        Ok(Bin { creator, mobs, items, parts, kinds, by_id })
    }

    pub fn mob(&self, id: &MobId) -> Option<&Mob> {
        self.by_id.get(id).map(|&i| &self.mobs[i])
    }

    /// Name of the source a file mob was made from (tape / camera clip or imported file), if stored in this bin.
    pub fn source_name(&self, file: &Mob) -> Option<String> {
        let mut stack = file.tracks.clone();
        let mut seen = HashSet::new();
        while let Some(p) = stack.pop() {
            if !seen.insert(p) {
                continue;
            }
            match self.parts.get(&p) {
                Some(Part::Parts(v)) => stack.extend(v),
                Some(Part::Clip(id, _)) => match self.mob(id) {
                    Some(m) if m.kind == MobKind::Source && !m.is_file() && !m.name.trim().is_empty() => return Some(m.name.clone()),
                    _ => {}
                },
                None => {}
            }
        }
        None
    }

    /// The tracks of `m` a source clip points at: same track number and media kind. A camera master clip
    /// also has sound tracks; a subclip that only uses its picture must not need the camera sound.
    /// Falls back to all tracks when the clip's track is not found (unknown layout: rather too much than too little).
    fn used_tracks(&self, m: &Mob, track: u16, kind: Option<u16>) -> Vec<u32> {
        let hit: Vec<u32> = m
            .tracks
            .iter()
            .zip(&m.track_numbers)
            .filter(|(p, n)| **n == Some(track) && kind.is_some() && self.kinds.get(p).copied() == kind)
            .map(|(p, _)| *p)
            .collect();
        if hit.is_empty() { m.tracks.clone() } else { hit }
    }

    /// Media files a mob uses: file source mobs reached through its tracks, sequences, groups and clips.
    /// `missing` gets mob ids that are referenced but not stored in this bin.
    pub fn media_of(&self, mob: usize, missing: &mut HashSet<MobId>) -> Vec<MobId> {
        let mut found = vec![];
        let mut seen_mobs = HashSet::new();
        let mut stack: Vec<u32> = self.mobs[mob].tracks.clone();
        let mut seen_parts = HashSet::new();
        while let Some(p) = stack.pop() {
            if !seen_parts.insert(p) {
                continue;
            }
            match self.parts.get(&p) {
                Some(Part::Parts(v)) => stack.extend(v),
                Some(Part::Clip(id, track)) => {
                    // the same mob can be reached for different tracks (subclip picture + its master's sound)
                    if !seen_mobs.insert((*id, *track, self.kinds.get(&p).copied())) {
                        continue;
                    }
                    match self.mob(id) {
                        Some(m) if m.is_file() => found.push(*id),
                        // tape / import sources end the chain
                        Some(m) if m.kind == MobKind::Source => {}
                        Some(m) => stack.extend(self.used_tracks(m, *track, self.kinds.get(&p).copied())),
                        None => {
                            missing.insert(*id);
                        }
                    }
                }
                None => {}
            }
        }
        found.sort();
        found.dedup();
        found
    }
}

/// Mob id as text, e.g. for logs: label-length-instance-material in hex.
pub fn id_hex(id: &MobId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a minimal little-endian bin the way Media Composer lays it out.
    struct W(Vec<u8>);
    impl W {
        fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend(v.to_le_bytes());
            self
        }
        fn str(&mut self, s: &str) -> &mut Self {
            self.raw_str(s.as_bytes())
        }
        /// names are stored in Mac Roman
        fn raw_str(&mut self, s: &[u8]) -> &mut Self {
            self.u16(s.len() as u16);
            self.0.extend(s);
            self
        }
        fn mob_id(&mut self, id: &MobId) -> &mut Self {
            self.u8(65).u32(12);
            self.0.extend(&id[..12]);
            for b in &id[12..16] {
                self.u8(68).u8(*b);
            }
            self.u8(72);
            self.0.extend(&id[16..20]);
            self.u8(70);
            self.0.extend(&id[20..22]);
            self.u8(70);
            self.0.extend(&id[22..24]);
            self.u8(65).u32(8);
            self.0.extend(&id[24..32]);
            self
        }
        fn component(&mut self, name: &[u8]) -> &mut Self {
            self.u8(2).u8(3).u32(0).u32(0).u16(1).u32(25).u16(0).raw_str(name).u16(0xFFFF).u32(0).u32(0).u32(0)
        }
        fn tracks(&mut self, parts: &[u32]) -> &mut Self {
            self.u8(2).u8(8).u8(0).u32(100).u32(0).u32(parts.len() as u32);
            for (i, p) in parts.iter().enumerate() {
                self.u16(1 | 4).u16(i as u16 + 1).u32(*p);
            }
            self
        }
        fn mob(name: &[u8], kind: u8, usage: u32, descriptor: u32, tracks: &[u32], id: &MobId) -> Vec<u8> {
            let mut w = W(vec![]);
            w.component(name).tracks(tracks).u8(2).u8(2).u32(0).u32(0).u32(0).u8(kind).u32(usage).u32(descriptor);
            w.u8(1).u8(2).mob_id(id).u8(3);
            w.0
        }
        fn clip(id: &MobId) -> Vec<u8> {
            W::clip_on(id, 1)
        }
        fn clip_on(id: &MobId, track: u16) -> Vec<u8> {
            let mut w = W(vec![]);
            w.component(b"").u8(2).u8(1).u32(100).u8(2).u8(3).u32(0).u32(0).u16(track).u32(0);
            w.u8(1).u8(1).mob_id(id).u8(3);
            w.0
        }
        /// A whole bin: header, then the objects (1-based; the last one is the root).
        fn bin(objects: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
            let mut w = W(vec![6, 0]);
            w.0.extend(b"Domain");
            w.0.extend(b"DJBO");
            w.str("AObjDoc").u8(4).str("2026/11/04 10:00:00").u32(objects.len() as u32).u32(objects.len() as u32);
            w.u32(0x49494949).u32(0).u32(0);
            w.0.extend(b"boTA");
            w.0.extend(b"evTA");
            w.str("Media Composer 25.12");
            w.0.extend([0u8; 16]);
            for (class, data) in objects {
                w.0.extend(class.iter().rev());
                w.u32(data.len() as u32);
                w.0.extend(data);
            }
            w.0
        }
    }

    fn id(n: u8) -> MobId {
        let mut id = [0u8; 32];
        id[..12].copy_from_slice(&[6, 0x0a, 0x2b, 0x34, 1, 1, 1, 1, 1, 1, 0x0f, 0]);
        id[12] = 0x13;
        id[16] = n;
        id
    }

    #[test]
    fn finds_media_of_clips_and_sequences() {
        let (file, master, seq, gone, master2, render) = (id(1), id(2), id(3), id(4), id(5), id(6));
        let mut sequ = W(vec![]);
        sequ.component(b"").u8(2).u8(3).u32(1).u32(7);
        let mut cdci = W(vec![]);
        cdci.u8(2).u8(3).u8(0).u32(1).u8(0).u32(0);
        let (newer, linked) = (id(8), id(9));
        let mut ama_cdci = W(vec![]);
        ama_cdci.u8(2).u8(3).u8(0).u32(1).u8(0).u32(0).u8(1).u8(1).u8(65).u32(16);
        ama_cdci.0.extend([7u8; 16]);
        let mut abin = W(vec![]);
        abin.u8(2).u8(0x0e).u32(0).u32(0).u32(0).u16(6);
        for obj in [5u32, 3, 8, 10, 13, 22] {
            abin.u32(obj).u16(0).u16(0).u32(0).u8(1);
        }
        let objects: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"MSML", vec![0; 8]),
            (b"CDCI", cdci.0),
            (b"CMPO", W::mob(b"K\x9alsch.mxf", 3, 0, 2, &[15], &file)),
            (b"SCLP", W::clip(&file)),
            (b"CMPO", W::mob(b"Clip \x85", 2, 0, 0, &[4], &master)),
            (b"SEQU", sequ.0),
            (b"SCLP", W::clip(&master)),
            (b"CMPO", W::mob(b"Sequence", 1, 0, 0, &[6], &seq)),
            (b"SCLP", W::clip(&gone)),
            (b"CMPO", W::mob(b"Lost", 2, 0, 0, &[9], &master2)),
            (b"CMPO", W::mob(b"", 3, 0, 2, &[], &render)),
            (b"SCLP", W::clip(&render)),
            // master clip of a render file (usage 1)
            (b"CMPO", W::mob(b"Sequence,Dissolve,1", 2, 1, 0, &[12], &id(7))),
            // 14–18: the camera clip of the file, and a newer media file of the same camera clip
            (b"MDTP", vec![2, 3, 0, 0, 0, 0, 0]),
            (b"SCLP", W::clip(&id(10))),
            (b"CMPO", W::mob(b"Cam A001", 3, 0, 14, &[], &id(10))),
            (b"CMPO", W::mob(b"File", 3, 0, 2, &[18], &newer)),
            (b"SCLP", W::clip(&id(10))),
            // 19–22: an AMA-linked clip (descriptor with acfUID)
            (b"CDCI", ama_cdci.0),
            (b"CMPO", W::mob(b"Linked", 3, 0, 19, &[], &linked)),
            (b"SCLP", W::clip(&linked)),
            (b"CMPO", W::mob(b"Linked", 2, 0, 0, &[21], &id(11))),
            (b"ABIN", abin.0),
        ];
        let w = W(W::bin(&objects));

        let bin = Bin::parse(&w.0).unwrap();
        assert_eq!(bin.creator, "Media Composer 25.12");
        let names: Vec<&str> = bin.items.iter().map(|&i| bin.mobs[i].name.as_str()).collect();
        assert_eq!(names, ["Clip Ö", "Kölsch.mxf", "Sequence", "Lost", "Sequence,Dissolve,1", "Linked"]); // Mac Roman 0x85 = Ö, 0x9A = ö
        assert!(bin.mob(&file).unwrap().is_file());
        assert_eq!(bin.mob(&file).unwrap().locator.as_deref(), Some("MSML"));
        let mut missing = HashSet::new();
        assert_eq!(bin.media_of(bin.items[0], &mut missing), [file]);
        assert_eq!(bin.media_of(bin.items[2], &mut missing), [file]);
        assert!(missing.is_empty());
        assert!(bin.media_of(bin.items[3], &mut missing).is_empty());
        assert_eq!(missing, HashSet::from([gone]));
        assert!(Bin::parse(&w.0[..w.0.len() - 3]).is_err());

        // project scan: the file is listed once per bin under its master clip with every use, the render apart
        use crate::projects::{ClipKind, MissingGroup, scan};
        let project = std::env::temp_dir().join(format!("mfc-avb-{}", std::process::id())).join("P");
        let _ = fs::remove_dir_all(&project);
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("P.avp"), b"").unwrap();
        fs::write(project.join("Bin.avb"), &w.0).unwrap();
        let r = scan(&[project.clone()], &HashSet::new(), &mut |_, _, _| {});
        assert!(bin.mob(&linked).unwrap().ama && !bin.mob(&file).unwrap().ama);
        assert_eq!((r.missing.len(), r.renders, r.ama, r.relink), (1, 1, 1, 0));
        // the AMA clip is listed where it is used
        let ama = &r.ama_by_bin[&("P".to_string(), "Bin.avb".to_string())];
        assert_eq!(ama, &[MissingGroup { name: "Linked".into(), files: 1, relink: false, uses: vec![("Linked".to_string(), ClipKind::Master)] }]);
        assert_eq!(r.missing[0].name, "Clip Ö");
        let groups = &r.by_bin[&("P".to_string(), "Bin.avb".to_string())];
        let uses = vec![("Clip Ö".to_string(), ClipKind::Master), ("Sequence".to_string(), ClipKind::Sequence)];
        assert_eq!(groups, &[MissingGroup { name: "Clip Ö".into(), files: 1, relink: false, uses }]);
        // the newer file of the same camera clip is on the drives: relink, not requested
        let r = scan(&[project.clone()], &HashSet::from([newer]), &mut |_, _, _| {});
        assert_eq!((r.missing.len(), r.relink), (0, 1));
        assert!(r.by_bin.values().next().unwrap()[0].relink);
        assert!(scan(&[project.clone()], &HashSet::from([file, render]), &mut |_, _, _| {}).by_bin.is_empty());
        fs::remove_dir_all(project.parent().unwrap()).unwrap();
    }

    /// A subclip that uses only the picture of a camera master clip does not need the camera sound; a file mob
    /// without any locator (sound that was never imported) is left out of the project scan.
    #[test]
    fn follows_only_the_used_track() {
        let (file, sound, master, sub) = (id(1), id(2), id(3), id(4));
        let mut cdci = W(vec![]);
        cdci.u8(2).u8(3).u8(0).u32(1).u8(0).u32(0);
        let mut pcma = W(vec![]);
        pcma.u8(2).u8(3).u8(0).u32(0).u8(0).u32(0);
        let mut abin = W(vec![]);
        abin.u8(2).u8(0x0e).u32(0).u32(0).u32(0).u16(2);
        for obj in [9u32, 11] {
            abin.u32(obj).u16(0).u16(0).u32(0).u8(1);
        }
        let objects: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"MSML", vec![0; 8]),
            (b"CDCI", cdci.0),
            (b"CMPO", W::mob(b"Cam.mxf", 3, 0, 2, &[], &file)),
            (b"PCMA", pcma.0),
            (b"CMPO", W::mob(b"Audio file", 3, 0, 4, &[], &sound)),
            (b"SCLP", W::clip(&file)),
            (b"SCLP", W::clip(&sound)),
            (b"MSML", vec![0; 8]), // filler object, keeps the indexes below
            (b"CMPO", W::mob(b"Cam", 2, 0, 0, &[6, 7], &master)),
            (b"SCLP", W::clip_on(&master, 1)),
            (b"CMPO", W::mob(b"Sub", 2, 2, 0, &[10], &sub)),
            (b"ABIN", abin.0),
        ];
        let data = W::bin(&objects);
        let bin = Bin::parse(&data).unwrap();
        let mut unresolved = HashSet::new();
        assert_eq!(bin.media_of(bin.items[0], &mut unresolved), [file, sound]);
        assert_eq!(bin.media_of(bin.items[1], &mut unresolved), [file]);
        assert_eq!(bin.mob(&sound).unwrap().locator, None);

        let project = std::env::temp_dir().join(format!("mfc-track-{}", std::process::id())).join("P");
        let _ = fs::remove_dir_all(&project);
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("P.avp"), b"").unwrap();
        fs::write(project.join("Bin.avb"), &data).unwrap();
        let r = crate::projects::scan(&[project.clone()], &HashSet::from([file]), &mut |_, _, _| {});
        assert_eq!((r.missing.len(), r.no_media), (0, 1));
        fs::remove_dir_all(project.parent().unwrap()).unwrap();
    }

    /// An AMA clip whose master clip has a namesake with its media on the drives (linked first, imported later)
    /// is left out of the AMA list.
    #[test]
    fn ama_clip_with_an_online_namesake_is_left_out() {
        let (file, linked) = (id(1), id(2));
        let mut cdci = W(vec![]);
        cdci.u8(2).u8(3).u8(0).u32(1).u8(0).u32(0);
        let mut ama_cdci = W(vec![]);
        ama_cdci.u8(2).u8(3).u8(0).u32(1).u8(0).u32(0).u8(1).u8(1).u8(65).u32(16);
        ama_cdci.0.extend([7u8; 16]);
        let mut abin = W(vec![]);
        abin.u8(2).u8(0x0e).u32(0).u32(0).u32(0).u16(2);
        for obj in [5u32, 9] {
            abin.u32(obj).u16(0).u16(0).u32(0).u8(1);
        }
        let objects: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"MSML", vec![0; 8]),
            (b"CDCI", cdci.0),
            (b"CMPO", W::mob(b"Stars.mxf", 3, 0, 2, &[], &file)),
            (b"SCLP", W::clip(&file)),
            (b"CMPO", W::mob(b"Stars", 2, 0, 0, &[4], &id(3))),
            (b"CDCI", ama_cdci.0),
            (b"CMPO", W::mob(b"Stars.mp4", 3, 0, 6, &[], &linked)),
            (b"SCLP", W::clip(&linked)),
            (b"CMPO", W::mob(b"Stars", 2, 0, 0, &[8], &id(4))),
            (b"ABIN", abin.0),
        ];
        let project = std::env::temp_dir().join(format!("mfc-ama-{}", std::process::id())).join("P");
        let _ = fs::remove_dir_all(&project);
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("P.avp"), b"").unwrap();
        fs::write(project.join("Bin.avb"), W::bin(&objects)).unwrap();
        let r = crate::projects::scan(&[project.clone()], &HashSet::from([file]), &mut |_, _, _| {});
        assert_eq!((r.ama, r.ama_covered, r.ama_by_bin.len()), (0, 1, 0));
        // the imported file is not on the drives: the AMA clip counts
        let r = crate::projects::scan(&[project.clone()], &HashSet::new(), &mut |_, _, _| {});
        assert_eq!((r.ama, r.ama_covered, r.ama_by_bin.len()), (1, 0, 1));
        fs::remove_dir_all(project.parent().unwrap()).unwrap();
    }
}
