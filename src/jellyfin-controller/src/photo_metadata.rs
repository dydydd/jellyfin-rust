use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

const MAX_EXIF_BYTES: usize = 1024 * 1024;
const MAX_IFD_ENTRIES: usize = 4096;
const MAX_ISO_BOXES: usize = 4096;
const MAX_ITEM_EXTENTS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PhotoMetadata {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub software: Option<String>,
    pub exposure_time: Option<f64>,
    pub focal_length: Option<f64>,
    pub image_orientation: Option<&'static str>,
    pub aperture: Option<f64>,
    pub shutter_speed: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub iso_speed_rating: Option<i32>,
}

/// Reads only container headers and a bounded EXIF payload. Pixel data is never decoded.
/// `Some(default)` means a supported container was valid but had no usable EXIF.
pub(crate) fn read(path: &Path) -> io::Result<Option<PhotoMetadata>> {
    let mut file = File::open(path)?;
    let mut signature = [0_u8; 12];
    let signature_len = file.read(&mut signature)?;
    file.seek(SeekFrom::Start(0))?;

    if signature_len >= 2 && signature[..2] == [0xff, 0xd8] {
        return jpeg_exif(&mut file)
            .and_then(metadata_from_payload)
            .map(Some);
    }
    if signature_len >= 8 && signature[..8] == *b"\x89PNG\r\n\x1a\n" {
        return png_exif(&mut file)
            .and_then(metadata_from_payload)
            .map(Some);
    }
    if signature_len >= 12 && signature[..4] == *b"RIFF" && signature[8..12] == *b"WEBP" {
        return webp_exif(&mut file)
            .and_then(metadata_from_payload)
            .map(Some);
    }
    if signature_len >= 12 && signature[4..8] == *b"ftyp" && avif_container(&mut file)? {
        return avif_exif(&mut file)
            .and_then(metadata_from_payload)
            .map(Some);
    }
    if signature_len >= 4 && (signature[..4] == *b"II*\0" || signature[..4] == *b"MM\0*") {
        let length = file.metadata()?.len();
        let bounded = usize::try_from(length.min(MAX_EXIF_BYTES as u64)).unwrap_or(MAX_EXIF_BYTES);
        let mut payload = vec![0; bounded];
        file.read_exact(&mut payload)?;
        return parse_tiff(&payload)
            .map(Some)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid TIFF metadata"));
    }
    Ok(None)
}

fn jpeg_exif(file: &mut File) -> io::Result<Option<Vec<u8>>> {
    file.seek(SeekFrom::Start(2))?;
    loop {
        let mut marker = [0_u8; 2];
        if file.read_exact(&mut marker).is_err() {
            return Ok(None);
        }
        while marker[0] != 0xff {
            marker[0] = marker[1];
            if file.read_exact(&mut marker[1..]).is_err() {
                return Ok(None);
            }
        }
        while marker[1] == 0xff {
            file.read_exact(&mut marker[1..])?;
        }
        if matches!(marker[1], 0xd9 | 0xda) {
            return Ok(None);
        }
        if marker[1] == 0x01 || (0xd0..=0xd7).contains(&marker[1]) {
            continue;
        }
        let mut length = [0_u8; 2];
        file.read_exact(&mut length)?;
        let payload_len = usize::from(u16::from_be_bytes(length)).saturating_sub(2);
        if payload_len == 0 {
            continue;
        }
        if marker[1] == 0xe1 && payload_len <= MAX_EXIF_BYTES {
            let mut payload = vec![0; payload_len];
            file.read_exact(&mut payload)?;
            if payload.starts_with(b"Exif\0\0") {
                payload.drain(..6);
                return Ok(Some(payload));
            }
        } else {
            file.seek(SeekFrom::Current(
                i64::try_from(payload_len).unwrap_or(i64::MAX),
            ))?;
        }
    }
}

fn png_exif(file: &mut File) -> io::Result<Option<Vec<u8>>> {
    file.seek(SeekFrom::Start(8))?;
    loop {
        let mut header = [0_u8; 8];
        if file.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_be_bytes(header[..4].try_into().unwrap()))
            .unwrap_or(usize::MAX);
        let chunk_type = &header[4..];
        if chunk_type == b"eXIf" && length <= MAX_EXIF_BYTES {
            let mut payload = vec![0; length];
            file.read_exact(&mut payload)?;
            return Ok(Some(payload));
        }
        let skip = length.saturating_add(4);
        file.seek(SeekFrom::Current(i64::try_from(skip).unwrap_or(i64::MAX)))?;
        if chunk_type == b"IEND" {
            return Ok(None);
        }
    }
}

fn webp_exif(file: &mut File) -> io::Result<Option<Vec<u8>>> {
    file.seek(SeekFrom::Start(12))?;
    loop {
        let mut header = [0_u8; 8];
        if file.read_exact(&mut header).is_err() {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_le_bytes(header[4..].try_into().unwrap()))
            .unwrap_or(usize::MAX);
        if &header[..4] == b"EXIF" && length <= MAX_EXIF_BYTES {
            let mut payload = vec![0; length];
            file.read_exact(&mut payload)?;
            return Ok(Some(payload));
        }
        let padded = length.saturating_add(length % 2);
        file.seek(SeekFrom::Current(i64::try_from(padded).unwrap_or(i64::MAX)))?;
    }
}

#[derive(Debug, Clone, Copy)]
struct IsoBox {
    data_start: u64,
    end: u64,
    kind: [u8; 4],
}

fn iso_box(file: &mut File, start: u64, parent_end: u64) -> io::Result<Option<IsoBox>> {
    if parent_end.saturating_sub(start) < 8 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(start))?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header)?;
    let short_size = u32::from_be_bytes(header[..4].try_into().unwrap());
    let kind = header[4..8].try_into().unwrap();
    let (size, header_size) = match short_size {
        0 => (parent_end.saturating_sub(start), 8_u64),
        1 => {
            let mut extended = [0_u8; 8];
            file.read_exact(&mut extended)?;
            (u64::from_be_bytes(extended), 16)
        }
        size => (u64::from(size), 8),
    };
    let end = start.checked_add(size).ok_or_else(invalid_avif)?;
    if size < header_size || end > parent_end {
        return Err(invalid_avif());
    }
    Ok(Some(IsoBox {
        data_start: start + header_size,
        end,
        kind,
    }))
}

fn find_iso_box(
    file: &mut File,
    start: u64,
    end: u64,
    wanted: &[u8; 4],
) -> io::Result<Option<IsoBox>> {
    let mut position = start;
    for _ in 0..MAX_ISO_BOXES {
        let Some(header) = iso_box(file, position, end)? else {
            return Ok(None);
        };
        if &header.kind == wanted {
            return Ok(Some(header));
        }
        if header.end <= position {
            return Err(invalid_avif());
        }
        position = header.end;
    }
    Err(invalid_avif())
}

fn bounded_box_payload(file: &mut File, header: IsoBox) -> io::Result<Vec<u8>> {
    let length = usize::try_from(header.end.saturating_sub(header.data_start))
        .map_err(|_| invalid_avif())?;
    if length > MAX_EXIF_BYTES {
        return Err(invalid_avif());
    }
    let mut payload = vec![0_u8; length];
    file.seek(SeekFrom::Start(header.data_start))?;
    file.read_exact(&mut payload)?;
    Ok(payload)
}

fn avif_container(file: &mut File) -> io::Result<bool> {
    let file_end = file.metadata()?.len();
    let Some(ftyp) = find_iso_box(file, 0, file_end, b"ftyp")? else {
        return Ok(false);
    };
    let payload = bounded_box_payload(file, ftyp)?;
    if payload.len() < 8 {
        return Err(invalid_avif());
    }
    Ok(matches!(&payload[..4], b"avif" | b"avis")
        || payload[8..]
            .chunks_exact(4)
            .any(|brand| matches!(brand, b"avif" | b"avis")))
}

fn avif_exif(file: &mut File) -> io::Result<Option<Vec<u8>>> {
    let file_end = file.metadata()?.len();
    let Some(meta) = find_iso_box(file, 0, file_end, b"meta")? else {
        return Ok(None);
    };
    let child_start = meta.data_start.checked_add(4).ok_or_else(invalid_avif)?;
    if child_start > meta.end {
        return Err(invalid_avif());
    }
    let Some(iinf) = find_iso_box(file, child_start, meta.end, b"iinf")? else {
        return Ok(None);
    };
    let Some(item_id) = avif_exif_item_id(file, iinf)? else {
        return Ok(None);
    };
    let Some(iloc) = find_iso_box(file, child_start, meta.end, b"iloc")? else {
        return Ok(None);
    };
    let idat = find_iso_box(file, child_start, meta.end, b"idat")?;
    let Some(location) = avif_item_location(&bounded_box_payload(file, iloc)?, item_id) else {
        return Ok(None);
    };
    let mut payload = Vec::new();
    for extent in location.extents {
        let origin = match location.construction_method {
            0 => 0,
            1 => idat
                .map(|box_header| box_header.data_start)
                .ok_or_else(invalid_avif)?,
            _ => return Ok(None),
        };
        let start = origin
            .checked_add(location.base_offset)
            .and_then(|offset| offset.checked_add(extent.offset))
            .ok_or_else(invalid_avif)?;
        let end = start.checked_add(extent.length).ok_or_else(invalid_avif)?;
        if end > file_end {
            return Err(invalid_avif());
        }
        let length = usize::try_from(extent.length).map_err(|_| invalid_avif())?;
        if length == 0 || payload.len().saturating_add(length) > MAX_EXIF_BYTES + 4 {
            return Err(invalid_avif());
        }
        let old_len = payload.len();
        payload.resize(old_len + length, 0);
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut payload[old_len..])?;
    }
    let offset = payload
        .get(..4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes)
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(invalid_avif)?;
    let tiff_start = 4_usize.checked_add(offset).ok_or_else(invalid_avif)?;
    let tiff = payload.get(tiff_start..).ok_or_else(invalid_avif)?;
    if tiff.len() > MAX_EXIF_BYTES {
        return Err(invalid_avif());
    }
    Ok(Some(tiff.to_vec()))
}

fn avif_exif_item_id(file: &mut File, iinf: IsoBox) -> io::Result<Option<u32>> {
    let payload = bounded_box_payload(file, iinf)?;
    let version = *payload.first().ok_or_else(invalid_avif)?;
    let count_size = if version == 0 { 2 } else { 4 };
    let child_offset = 4_usize.checked_add(count_size).ok_or_else(invalid_avif)?;
    let child_start = iinf
        .data_start
        .checked_add(u64::try_from(child_offset).unwrap())
        .ok_or_else(invalid_avif)?;
    let mut position = child_start;
    for _ in 0..MAX_ISO_BOXES {
        let Some(header) = iso_box(file, position, iinf.end)? else {
            return Ok(None);
        };
        if header.kind == *b"infe" {
            let infe = bounded_box_payload(file, header)?;
            if let Some(item_id) = parse_exif_infe(&infe) {
                return Ok(Some(item_id));
            }
        }
        position = header.end;
    }
    Err(invalid_avif())
}

fn parse_exif_infe(bytes: &[u8]) -> Option<u32> {
    match *bytes.first()? {
        2 => {
            let item_id = u32::from(u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?));
            (bytes.get(8..12)? == b"Exif").then_some(item_id)
        }
        3 => {
            let item_id = u32::from_be_bytes(bytes.get(4..8)?.try_into().ok()?);
            (bytes.get(10..14)? == b"Exif").then_some(item_id)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct AvifExtent {
    offset: u64,
    length: u64,
}

#[derive(Debug)]
struct AvifItemLocation {
    construction_method: u16,
    base_offset: u64,
    extents: Vec<AvifExtent>,
}

fn avif_item_location(bytes: &[u8], wanted_item_id: u32) -> Option<AvifItemLocation> {
    let version = *bytes.first()?;
    if version > 2 {
        return None;
    }
    let mut cursor = 4_usize;
    let sizes = *bytes.get(cursor)?;
    cursor += 1;
    let offset_size = usize::from(sizes >> 4);
    let length_size = usize::from(sizes & 0x0f);
    let sizes = *bytes.get(cursor)?;
    cursor += 1;
    let base_offset_size = usize::from(sizes >> 4);
    let index_size = if version == 0 {
        0
    } else {
        usize::from(sizes & 0x0f)
    };
    if [offset_size, length_size, base_offset_size, index_size]
        .into_iter()
        .any(|size| size > 8)
    {
        return None;
    }
    let item_count = if version < 2 {
        usize::try_from(read_be_uint(bytes, &mut cursor, 2)?).ok()?
    } else {
        usize::try_from(read_be_uint(bytes, &mut cursor, 4)?).ok()?
    };
    if item_count > MAX_IFD_ENTRIES {
        return None;
    }
    for _ in 0..item_count {
        let item_id = if version < 2 {
            u32::try_from(read_be_uint(bytes, &mut cursor, 2)?).ok()?
        } else {
            u32::try_from(read_be_uint(bytes, &mut cursor, 4)?).ok()?
        };
        let construction_method = if version == 0 {
            0
        } else {
            u16::try_from(read_be_uint(bytes, &mut cursor, 2)? & 0x0fff).ok()?
        };
        let data_reference_index = read_be_uint(bytes, &mut cursor, 2)?;
        let base_offset = read_be_uint(bytes, &mut cursor, base_offset_size)?;
        let extent_count = usize::try_from(read_be_uint(bytes, &mut cursor, 2)?).ok()?;
        if extent_count > MAX_ITEM_EXTENTS {
            return None;
        }
        let mut extents = Vec::with_capacity(extent_count);
        for _ in 0..extent_count {
            if index_size != 0 {
                read_be_uint(bytes, &mut cursor, index_size)?;
            }
            extents.push(AvifExtent {
                offset: read_be_uint(bytes, &mut cursor, offset_size)?,
                length: read_be_uint(bytes, &mut cursor, length_size)?,
            });
        }
        if item_id == wanted_item_id && data_reference_index == 0 && !extents.is_empty() {
            return Some(AvifItemLocation {
                construction_method,
                base_offset,
                extents,
            });
        }
    }
    None
}

fn read_be_uint(bytes: &[u8], cursor: &mut usize, size: usize) -> Option<u64> {
    let end = cursor.checked_add(size)?;
    let value = bytes
        .get(*cursor..end)?
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
    *cursor = end;
    Some(value)
}

fn invalid_avif() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid AVIF metadata")
}

fn parse_exif_payload(payload: Vec<u8>) -> Option<PhotoMetadata> {
    let payload = payload.strip_prefix(b"Exif\0\0").unwrap_or(&payload);
    parse_tiff(payload)
}

fn metadata_from_payload(payload: Option<Vec<u8>>) -> io::Result<PhotoMetadata> {
    payload.map_or_else(
        || Ok(PhotoMetadata::default()),
        |payload| {
            parse_exif_payload(payload).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid embedded EXIF metadata")
            })
        },
    )
}

#[derive(Debug, Clone, Copy)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    field_type: u16,
    count: u32,
    value: [u8; 4],
}

struct Tiff<'a> {
    bytes: &'a [u8],
    endian: Endian,
}

impl Tiff<'_> {
    fn u16(&self, offset: usize) -> Option<u16> {
        let bytes: [u8; 2] = self
            .bytes
            .get(offset..offset.checked_add(2)?)?
            .try_into()
            .ok()?;
        Some(match self.endian {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        })
    }

    fn u32(&self, offset: usize) -> Option<u32> {
        let bytes: [u8; 4] = self
            .bytes
            .get(offset..offset.checked_add(4)?)?
            .try_into()
            .ok()?;
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        })
    }

    fn i32(&self, offset: usize) -> Option<i32> {
        let bytes: [u8; 4] = self
            .bytes
            .get(offset..offset.checked_add(4)?)?
            .try_into()
            .ok()?;
        Some(match self.endian {
            Endian::Little => i32::from_le_bytes(bytes),
            Endian::Big => i32::from_be_bytes(bytes),
        })
    }

    fn entry(&self, ifd_offset: u32, wanted_tag: u16) -> Option<Entry> {
        let ifd_offset = usize::try_from(ifd_offset).ok()?;
        let count = usize::from(self.u16(ifd_offset)?).min(MAX_IFD_ENTRIES);
        for index in 0..count {
            let offset = ifd_offset
                .checked_add(2)?
                .checked_add(index.checked_mul(12)?)?;
            if self.u16(offset)? != wanted_tag {
                continue;
            }
            return Some(Entry {
                field_type: self.u16(offset + 2)?,
                count: self.u32(offset + 4)?,
                value: self.bytes.get(offset + 8..offset + 12)?.try_into().ok()?,
            });
        }
        None
    }

    fn value_offset(&self, entry: Entry, unit_size: usize) -> Option<usize> {
        let byte_len = usize::try_from(entry.count).ok()?.checked_mul(unit_size)?;
        if byte_len <= 4 {
            None
        } else {
            let raw = match self.endian {
                Endian::Little => u32::from_le_bytes(entry.value),
                Endian::Big => u32::from_be_bytes(entry.value),
            };
            usize::try_from(raw).ok()
        }
    }

    fn inline_u16(&self, entry: Entry) -> u16 {
        let bytes = [entry.value[0], entry.value[1]];
        match self.endian {
            Endian::Little => u16::from_le_bytes(bytes),
            Endian::Big => u16::from_be_bytes(bytes),
        }
    }

    fn unsigned(&self, entry: Entry) -> Option<u32> {
        match entry.field_type {
            3 => Some(u32::from(self.inline_u16(entry))),
            4 => Some(match self.endian {
                Endian::Little => u32::from_le_bytes(entry.value),
                Endian::Big => u32::from_be_bytes(entry.value),
            }),
            _ => None,
        }
    }

    fn ascii(&self, entry: Entry) -> Option<String> {
        if entry.field_type != 2 || entry.count == 0 {
            return None;
        }
        let len = usize::try_from(entry.count).ok()?.min(MAX_EXIF_BYTES);
        let bytes = if len <= 4 {
            entry.value.get(..len)?
        } else {
            let offset = self.value_offset(entry, 1)?;
            self.bytes.get(offset..offset.checked_add(len)?)?
        };
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let value = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
        (!value.is_empty()).then_some(value)
    }

    fn byte(&self, entry: Entry) -> Option<u8> {
        matches!(entry.field_type, 1 | 7).then_some(entry.value[0])
    }

    fn rational_at(&self, entry: Entry, index: usize) -> Option<f64> {
        if !matches!(entry.field_type, 5 | 10) || index >= usize::try_from(entry.count).ok()? {
            return None;
        }
        let base = self
            .value_offset(entry, 8)?
            .checked_add(index.checked_mul(8)?)?;
        let (numerator, denominator) = if entry.field_type == 10 {
            (f64::from(self.i32(base)?), f64::from(self.i32(base + 4)?))
        } else {
            (f64::from(self.u32(base)?), f64::from(self.u32(base + 4)?))
        };
        (denominator != 0.0)
            .then_some(numerator / denominator)
            .filter(|value| value.is_finite())
    }
}

fn parse_tiff(bytes: &[u8]) -> Option<PhotoMetadata> {
    let endian = match bytes.get(..2)? {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    let tiff = Tiff { bytes, endian };
    if tiff.u16(2)? != 42 {
        return None;
    }
    let ifd0 = tiff.u32(4)?;
    let exif_ifd = tiff
        .entry(ifd0, 0x8769)
        .and_then(|entry| tiff.unsigned(entry));
    let gps_ifd = tiff
        .entry(ifd0, 0x8825)
        .and_then(|entry| tiff.unsigned(entry));

    let mut metadata = PhotoMetadata {
        camera_make: tiff.entry(ifd0, 0x010f).and_then(|entry| tiff.ascii(entry)),
        camera_model: tiff.entry(ifd0, 0x0110).and_then(|entry| tiff.ascii(entry)),
        software: tiff.entry(ifd0, 0x0131).and_then(|entry| tiff.ascii(entry)),
        image_orientation: tiff
            .entry(ifd0, 0x0112)
            .and_then(|entry| tiff.unsigned(entry))
            .and_then(orientation_name),
        ..PhotoMetadata::default()
    };
    if let Some(exif_ifd) = exif_ifd {
        metadata.exposure_time = tiff
            .entry(exif_ifd, 0x829a)
            .and_then(|entry| tiff.rational_at(entry, 0));
        metadata.aperture = tiff
            .entry(exif_ifd, 0x9202)
            .and_then(|entry| tiff.rational_at(entry, 0));
        metadata.shutter_speed = tiff
            .entry(exif_ifd, 0x9201)
            .and_then(|entry| tiff.rational_at(entry, 0));
        metadata.focal_length = tiff
            .entry(exif_ifd, 0x920a)
            .and_then(|entry| tiff.rational_at(entry, 0));
        metadata.iso_speed_rating = tiff
            .entry(exif_ifd, 0x8827)
            .and_then(|entry| tiff.unsigned(entry))
            .and_then(|value| i32::try_from(value).ok());
    }
    if let Some(gps_ifd) = gps_ifd {
        metadata.latitude = gps_coordinate(&tiff, gps_ifd, 0x0001, 0x0002, b'S');
        metadata.longitude = gps_coordinate(&tiff, gps_ifd, 0x0003, 0x0004, b'W');
        metadata.altitude = tiff
            .entry(gps_ifd, 0x0006)
            .and_then(|entry| tiff.rational_at(entry, 0))
            .map(|altitude| {
                if tiff
                    .entry(gps_ifd, 0x0005)
                    .and_then(|entry| tiff.byte(entry))
                    == Some(1)
                {
                    -altitude
                } else {
                    altitude
                }
            });
    }
    Some(metadata)
}

fn gps_coordinate(
    tiff: &Tiff<'_>,
    ifd: u32,
    reference_tag: u16,
    coordinate_tag: u16,
    negative_reference: u8,
) -> Option<f64> {
    let coordinate = tiff.entry(ifd, coordinate_tag)?;
    let degrees = tiff.rational_at(coordinate, 0)?;
    let minutes = tiff.rational_at(coordinate, 1)?;
    let seconds = tiff.rational_at(coordinate, 2)?;
    let value = degrees + minutes / 60.0 + seconds / 3600.0;
    let negative = tiff
        .entry(ifd, reference_tag)
        .and_then(|entry| tiff.ascii(entry))
        .is_some_and(|reference| {
            reference
                .as_bytes()
                .first()
                .is_some_and(|value| value.eq_ignore_ascii_case(&negative_reference))
        });
    Some(if negative { -value } else { value })
}

const fn orientation_name(value: u32) -> Option<&'static str> {
    match value {
        1 => Some("TopLeft"),
        2 => Some("TopRight"),
        3 => Some("BottomRight"),
        4 => Some("BottomLeft"),
        5 => Some("LeftTop"),
        6 => Some("RightTop"),
        7 => Some("RightBottom"),
        8 => Some("LeftBottom"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{PhotoMetadata, parse_tiff, read};
    use std::{fs, path::PathBuf};

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn entry(bytes: &mut [u8], offset: usize, tag: u16, field_type: u16, count: u32, value: u32) {
        put_u16(bytes, offset, tag);
        put_u16(bytes, offset + 2, field_type);
        put_u32(bytes, offset + 4, count);
        put_u32(bytes, offset + 8, value);
    }

    fn rational(bytes: &mut [u8], offset: usize, numerator: u32, denominator: u32) {
        put_u32(bytes, offset, numerator);
        put_u32(bytes, offset + 4, denominator);
    }

    fn iso_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len() + 8).unwrap();
        let mut bytes = Vec::with_capacity(size as usize);
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn avif_with_exif(tiff: &[u8]) -> Vec<u8> {
        let mut ftyp_payload = Vec::new();
        ftyp_payload.extend_from_slice(b"mif1");
        ftyp_payload.extend_from_slice(&0_u32.to_be_bytes());
        ftyp_payload.extend_from_slice(b"mif1");
        ftyp_payload.extend_from_slice(b"avif");
        let ftyp = iso_box(b"ftyp", &ftyp_payload);

        let mut infe_payload = vec![2, 0, 0, 0];
        infe_payload.extend_from_slice(&1_u16.to_be_bytes());
        infe_payload.extend_from_slice(&0_u16.to_be_bytes());
        infe_payload.extend_from_slice(b"Exif");
        infe_payload.extend_from_slice(b"Exif metadata\0");
        let infe = iso_box(b"infe", &infe_payload);
        let mut iinf_payload = vec![0, 0, 0, 0];
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes());
        iinf_payload.extend_from_slice(&infe);
        let iinf = iso_box(b"iinf", &iinf_payload);

        let exif_length = u32::try_from(tiff.len() + 4).unwrap();
        let iloc_len = 8 + 4 + 2 + 2 + 2 + 2 + 2 + 4 + 4;
        let meta_len = 8 + 4 + iinf.len() + iloc_len;
        let exif_offset = u32::try_from(ftyp.len() + meta_len + 8).unwrap();
        let mut iloc_payload = vec![0, 0, 0, 0, 0x44, 0];
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes());
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes());
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes());
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes());
        iloc_payload.extend_from_slice(&exif_offset.to_be_bytes());
        iloc_payload.extend_from_slice(&exif_length.to_be_bytes());
        let iloc = iso_box(b"iloc", &iloc_payload);

        let mut meta_payload = vec![0, 0, 0, 0];
        meta_payload.extend_from_slice(&iinf);
        meta_payload.extend_from_slice(&iloc);
        let meta = iso_box(b"meta", &meta_payload);
        let mut exif_payload = vec![0, 0, 0, 0];
        exif_payload.extend_from_slice(tiff);
        let mdat = iso_box(b"mdat", &exif_payload);

        [ftyp, meta, mdat].concat()
    }

    fn complete_tiff() -> Vec<u8> {
        let mut bytes = vec![0_u8; 512];
        bytes[..8].copy_from_slice(b"II*\0\x08\0\0\0");
        put_u16(&mut bytes, 8, 6);
        entry(&mut bytes, 10, 0x010f, 2, 6, 280);
        entry(&mut bytes, 22, 0x0110, 2, 8, 286);
        entry(&mut bytes, 34, 0x0131, 2, 7, 294);
        entry(&mut bytes, 46, 0x0112, 3, 1, 6);
        entry(&mut bytes, 58, 0x8769, 4, 1, 100);
        entry(&mut bytes, 70, 0x8825, 4, 1, 180);

        put_u16(&mut bytes, 100, 5);
        entry(&mut bytes, 102, 0x829a, 5, 1, 320);
        entry(&mut bytes, 114, 0x920a, 5, 1, 328);
        entry(&mut bytes, 126, 0x9202, 5, 1, 336);
        entry(&mut bytes, 138, 0x9201, 10, 1, 344);
        entry(&mut bytes, 150, 0x8827, 3, 1, 640);

        put_u16(&mut bytes, 180, 6);
        entry(
            &mut bytes,
            182,
            0x0001,
            2,
            2,
            u32::from_le_bytes(*b"S\0\0\0"),
        );
        entry(&mut bytes, 194, 0x0002, 5, 3, 360);
        entry(
            &mut bytes,
            206,
            0x0003,
            2,
            2,
            u32::from_le_bytes(*b"E\0\0\0"),
        );
        entry(&mut bytes, 218, 0x0004, 5, 3, 384);
        entry(&mut bytes, 230, 0x0005, 1, 1, 1);
        entry(&mut bytes, 242, 0x0006, 5, 1, 408);

        bytes[280..286].copy_from_slice(b"Canon\0");
        bytes[286..294].copy_from_slice(b"EOS R5\0\0");
        bytes[294..301].copy_from_slice(b"Camera\0");
        rational(&mut bytes, 320, 1, 125);
        rational(&mut bytes, 328, 50, 1);
        rational(&mut bytes, 336, 28, 10);
        put_u32(&mut bytes, 344, (-7_i32).cast_unsigned());
        put_u32(&mut bytes, 348, 1);
        rational(&mut bytes, 360, 37, 1);
        rational(&mut bytes, 368, 48, 1);
        rational(&mut bytes, 376, 30, 1);
        rational(&mut bytes, 384, 122, 1);
        rational(&mut bytes, 392, 24, 1);
        rational(&mut bytes, 400, 15, 1);
        rational(&mut bytes, 408, 125, 10);
        bytes
    }

    #[test]
    fn parses_official_photo_fields_from_exif_ifds() {
        let metadata = parse_tiff(&complete_tiff()).unwrap();
        assert_eq!(
            metadata,
            PhotoMetadata {
                camera_make: Some("Canon".to_owned()),
                camera_model: Some("EOS R5".to_owned()),
                software: Some("Camera".to_owned()),
                exposure_time: Some(0.008),
                focal_length: Some(50.0),
                image_orientation: Some("RightTop"),
                aperture: Some(2.8),
                shutter_speed: Some(-7.0),
                latitude: Some(-(37.0 + 48.0 / 60.0 + 30.0 / 3600.0)),
                longitude: Some(122.0 + 24.0 / 60.0 + 15.0 / 3600.0),
                altitude: Some(-12.5),
                iso_speed_rating: Some(640),
            }
        );
    }

    #[test]
    fn reads_exif_from_jpeg_without_pixel_data() {
        let tiff = complete_tiff();
        let length = u16::try_from(tiff.len() + 8).unwrap();
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        let path = PathBuf::from(std::env::temp_dir()).join(format!(
            "jellyfin-photo-metadata-{}-{}.jpg",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, jpeg).unwrap();
        let metadata = read(&path).unwrap().unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(metadata.camera_model.as_deref(), Some("EOS R5"));
        assert_eq!(metadata.iso_speed_rating, Some(640));
    }

    #[test]
    fn reads_bounded_exif_item_from_avif_without_decoding_pixels() {
        let avif = avif_with_exif(&complete_tiff());
        let path = PathBuf::from(std::env::temp_dir()).join(format!(
            "jellyfin-photo-metadata-{}-{}.avif",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, avif).unwrap();
        let metadata = read(&path).unwrap().unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(metadata.camera_make.as_deref(), Some("Canon"));
        assert_eq!(metadata.camera_model.as_deref(), Some("EOS R5"));
        assert_eq!(metadata.image_orientation, Some("RightTop"));
        assert_eq!(metadata.iso_speed_rating, Some(640));
    }
}
