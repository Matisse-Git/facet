//! A tiny writer for Win32 `.res` files (the compiled form of an `.rc` script), so the build
//! does not need `rc.exe` from the Windows SDK. `link.exe` takes a `.res` as a plain input.
//!
//! Format: a sequence of entries, each a 32-byte header (ordinal type and name) followed by
//! the data padded to 4 bytes, after one empty leading entry that marks the file as 32-bit.

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const LANG_EN_US: u16 = 0x0409;
const CODEPAGE_UNICODE: u16 = 1200;

pub struct Resources {
    bytes: Vec<u8>,
    next_icon: u16,
}

/// One image of the icon: `size`×`size`, straight-alpha RGBA.
pub struct IconImage<'a> {
    pub size: u32,
    pub rgba: &'a [u8],
}

pub struct Version<'a> {
    /// major, minor, patch, build
    pub numbers: [u16; 4],
    pub strings: &'a [(&'a str, &'a str)],
}

impl Resources {
    pub fn new() -> Self {
        let mut resources = Self {
            bytes: Vec::new(),
            next_icon: 1,
        };
        resources.entry(0, 0, 0, 0, &[]);
        resources
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn entry(&mut self, kind: u16, name: u16, flags: u16, language: u16, data: &[u8]) {
        let b = &mut self.bytes;
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&32u32.to_le_bytes()); // header size
        b.extend_from_slice(&[0xFF, 0xFF]);
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&[0xFF, 0xFF]);
        b.extend_from_slice(&name.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // data version
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&language.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // version
        b.extend_from_slice(&0u32.to_le_bytes()); // characteristics
        b.extend_from_slice(data);
        pad(b);
    }

    /// Add an icon group (resource id `group`). Images of 256 px are stored as PNG (`png`),
    /// smaller ones as 32-bit DIBs, which every part of Windows reads.
    pub fn icon_group(
        &mut self,
        group: u16,
        images: &[IconImage],
        png: impl Fn(&IconImage) -> Vec<u8>,
    ) {
        let mut directory = Vec::new();
        directory.extend_from_slice(&0u16.to_le_bytes()); // reserved
        directory.extend_from_slice(&1u16.to_le_bytes()); // type: icon
        directory.extend_from_slice(&(images.len() as u16).to_le_bytes());
        for image in images {
            let data = if image.size >= 256 {
                png(image)
            } else {
                dib(image)
            };
            let id = self.next_icon;
            self.next_icon += 1;
            self.entry(RT_ICON, id, 0x1010, LANG_EN_US, &data);
            // GRPICONDIRENTRY: 256 is written as 0.
            let side = if image.size >= 256 {
                0
            } else {
                image.size as u8
            };
            directory.extend_from_slice(&[side, side, 0, 0]);
            directory.extend_from_slice(&1u16.to_le_bytes()); // planes
            directory.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&id.to_le_bytes());
        }
        self.entry(RT_GROUP_ICON, group, 0x1030, LANG_EN_US, &directory);
    }

    /// Add the `VERSIONINFO` resource (id 1): the numbers Explorer's Details tab shows and
    /// the `StringFileInfo` strings (FileDescription is the name Task Manager uses).
    pub fn version(&mut self, version: &Version) {
        let [major, minor, patch, build] = version.numbers.map(u32::from);
        let mut fixed = Vec::new();
        for value in [
            0xFEEF_04BD,           // signature
            0x0001_0000,           // structure version
            (major << 16) | minor, // file version
            (patch << 16) | build, //
            (major << 16) | minor, // product version
            (patch << 16) | build, //
            0x3F,                  // flags mask
            0,                     // flags
            0x0004_0004,           // VOS_NT_WINDOWS32
            1,                     // VFT_APP
            0,                     // subtype
            0,                     // date
            0,                     //
        ] {
            fixed.extend_from_slice(&u32::to_le_bytes(value));
        }

        let strings: Vec<Vec<u8>> = version
            .strings
            .iter()
            .map(|(key, value)| {
                let text = utf16z(value);
                block(key, 1, (text.len() / 2) as u16, &text, &[])
            })
            .collect();
        let table = format!("{LANG_EN_US:04X}{CODEPAGE_UNICODE:04X}");
        let table = block(&table, 1, 0, &[], &strings);
        let string_info = block("StringFileInfo", 1, 0, &[], &[table]);
        let mut translation = Vec::new();
        translation.extend_from_slice(&LANG_EN_US.to_le_bytes());
        translation.extend_from_slice(&CODEPAGE_UNICODE.to_le_bytes());
        let var = block("Translation", 0, 4, &translation, &[]);
        let var_info = block("VarFileInfo", 1, 0, &[], &[var]);
        let root = block(
            "VS_VERSION_INFO",
            0,
            fixed.len() as u16,
            &fixed,
            &[string_info, var_info],
        );
        self.entry(RT_VERSION, 1, 0x0030, LANG_EN_US, &root);
    }
}

/// A version-info block: length, value length, type, key, value, children; each part
/// aligned to 4 bytes. The length covers the block without trailing padding.
fn block(key: &str, kind: u16, value_length: u16, value: &[u8], children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend_from_slice(&value_length.to_le_bytes());
    b.extend_from_slice(&kind.to_le_bytes());
    b.extend_from_slice(&utf16z(key));
    pad(&mut b);
    b.extend_from_slice(value);
    for child in children {
        pad(&mut b);
        b.extend_from_slice(child);
    }
    let length = b.len() as u16;
    b[..2].copy_from_slice(&length.to_le_bytes());
    b
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn pad(bytes: &mut Vec<u8>) {
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

/// An icon image as a DIB: BITMAPINFOHEADER (height doubled for the mask), bottom-up BGRA,
/// then the 1-bit AND mask (set where fully transparent).
fn dib(image: &IconImage) -> Vec<u8> {
    let size = image.size;
    let mask_stride = size.div_ceil(32) * 4;
    let mut b = Vec::new();
    for value in [40u32, size, size * 2] {
        b.extend_from_slice(&value.to_le_bytes());
    }
    b.extend_from_slice(&1u16.to_le_bytes()); // planes
    b.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    let image_size = size * size * 4 + mask_stride * size;
    for value in [0u32, image_size, 0, 0, 0, 0] {
        b.extend_from_slice(&value.to_le_bytes());
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            let [r, g, bl, a] = [
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            ];
            b.extend_from_slice(&[bl, g, r, a]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_stride as usize];
        for x in 0..size {
            if image.rgba[((y * size + x) * 4 + 3) as usize] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        b.extend_from_slice(&row);
    }
    b
}
