pub static LOGO_ICO: &[u8] = include_bytes!("../logo.ico");

pub struct IcoEntry<'a> {
    pub data: &'a [u8],
}

pub fn best_entry(ico: &[u8]) -> Option<IcoEntry<'_>> {
    if ico.len() < 6 || ico[2] != 1 || ico[3] != 0 {
        return None;
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if count == 0 {
        return None;
    }

    let mut best: Option<(usize, IcoEntry<'_>)> = None;
    for i in 0..count {
        let base = 6 + i * 16;
        let entry = ico.get(base..base + 16)?;
        let width = if entry[0] == 0 {
            256
        } else {
            u32::from(entry[0])
        };
        let height = if entry[1] == 0 {
            256
        } else {
            u32::from(entry[1])
        };
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if offset + size > ico.len() {
            continue;
        }
        let area = width as usize * height as usize;
        if best
            .as_ref()
            .map_or(true, |(best_area, _)| area > *best_area)
        {
            best = Some((
                area,
                IcoEntry {
                    data: &ico[offset..offset + size],
                },
            ));
        }
    }
    best.map(|(_, entry)| entry)
}

pub fn decode_bgra_entry(entry: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if entry.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
    let width = i32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
    let height = i32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
    let bit_count = u16::from_le_bytes([entry[14], entry[15]]);
    let compression = u32::from_le_bytes([entry[16], entry[17], entry[18], entry[19]]);

    if width <= 0 || height <= 0 || header_size < 40 || header_size > entry.len() {
        return None;
    }
    if height % 2 != 0 {
        return None;
    }
    let w = width as usize;
    let h = (height / 2) as usize;
    if bit_count != 32 || (compression != 0 && compression != 3) {
        return None;
    }

    let pixels = w * h * 4;
    if header_size + pixels > entry.len() {
        return None;
    }

    let mut rgba = vec![0u8; pixels];
    for row in 0..h {
        let src_row = (h - 1 - row) * w * 4;
        let dst_row = row * w * 4;
        for x in 0..w {
            let s = src_row + x * 4;
            let d = dst_row + x * 4;
            rgba[d] = entry[header_size + s + 2];
            rgba[d + 1] = entry[header_size + s + 1];
            rgba[d + 2] = entry[header_size + s];
            rgba[d + 3] = entry[header_size + s + 3];
        }
    }
    Some((w as u32, h as u32, rgba))
}

pub fn load_app_icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let entry = best_entry(LOGO_ICO)?;
    if entry.data.len() >= 8 && entry.data[..4] == [0x89, b'P', b'N', b'G'] {
        return None;
    }
    decode_bgra_entry(entry.data)
}
