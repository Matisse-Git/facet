//! Deciding from a path how a file's changes should be shown: as a text diff, as an image
//! comparison, or as a metadata card. Known binary formats never go near the text differ,
//! whatever their content looks like.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Ask git for a text diff (git may still report the content as binary).
    Text,
    /// A raster image Facet can decode and compare visually.
    Image,
    /// A known binary format, with a human name for it.
    Binary(&'static str),
}

/// Image formats decoded for visual comparison.
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "tga", "bmp", "webp"];

/// Binary formats shown as metadata only: (extension, description).
const BINARIES: &[(&str, &str)] = &[
    // Unreal
    ("uasset", "Unreal asset"),
    ("umap", "Unreal level"),
    ("uexp", "Unreal asset data"),
    ("ubulk", "Unreal bulk data"),
    ("pak", "Unreal package"),
    // Unity
    ("unitypackage", "Unity package"),
    // 3D and 2D sources
    ("fbx", "FBX model"),
    ("blend", "Blender scene"),
    ("max", "3ds Max scene"),
    ("ma", "Maya scene"),
    ("mb", "Maya scene"),
    ("abc", "Alembic cache"),
    ("glb", "glTF binary model"),
    ("psd", "Photoshop document"),
    ("psb", "Photoshop large document"),
    ("xcf", "GIMP image"),
    ("kra", "Krita document"),
    ("spp", "Substance Painter project"),
    ("sbsar", "Substance archive"),
    // Images without a built-in viewer
    ("dds", "DirectDraw Surface texture"),
    ("exr", "OpenEXR image"),
    ("hdr", "Radiance HDR image"),
    ("tif", "TIFF image"),
    ("tiff", "TIFF image"),
    ("gif", "GIF image"),
    ("ico", "Icon"),
    // Audio and video
    ("wav", "WAV audio"),
    ("ogg", "Ogg audio"),
    ("mp3", "MP3 audio"),
    ("flac", "FLAC audio"),
    ("bnk", "Wwise sound bank"),
    ("wem", "Wwise audio"),
    ("mp4", "MP4 video"),
    ("mov", "QuickTime video"),
    ("webm", "WebM video"),
    // Fonts, documents, archives, executables
    ("ttf", "TrueType font"),
    ("otf", "OpenType font"),
    ("pdf", "PDF document"),
    ("zip", "ZIP archive"),
    ("7z", "7-Zip archive"),
    ("rar", "RAR archive"),
    ("gz", "Gzip archive"),
    ("exe", "Windows executable"),
    ("dll", "Windows library"),
    ("pdb", "Debug symbols"),
    ("lib", "Static library"),
    ("so", "Shared library"),
    ("dylib", "Shared library"),
    ("bin", "Binary data"),
];

/// The lower-cased extension of `path` (text after the last dot of the file name).
pub fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

pub fn classify(path: &str) -> FileKind {
    let Some(ext) = extension(path) else {
        return FileKind::Text;
    };
    if IMAGES.contains(&ext.as_str()) {
        return FileKind::Image;
    }
    BINARIES
        .iter()
        .find(|(known, _)| *known == ext)
        .map_or(FileKind::Text, |(_, description)| {
            FileKind::Binary(description)
        })
}

/// "1.5 MB"-style size, binary units as Explorer shows them.
pub fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return if bytes == 1 {
            "1 byte".to_owned()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_assets_are_never_text() {
        assert_eq!(
            classify("Content/Maps/Level.umap"),
            FileKind::Binary("Unreal level")
        );
        assert_eq!(
            classify("Content/Hero.UASSET"),
            FileKind::Binary("Unreal asset")
        );
        for path in ["a/model.fbx", "art/hero.psd", "scene.blend"] {
            assert!(matches!(classify(path), FileKind::Binary(_)), "{path}");
        }
    }

    #[test]
    fn images_and_text() {
        for path in ["a.png", "b.JPG", "c.jpeg", "d.tga", "e.bmp", "f.webp"] {
            assert_eq!(classify(path), FileKind::Image, "{path}");
        }
        for path in ["src/main.rs", "README", ".gitignore", "dir.png/file", "x."] {
            assert_eq!(classify(path), FileKind::Text, "{path}");
        }
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("a/b/c.TXT").as_deref(), Some("txt"));
        assert_eq!(extension("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension(".gitignore"), None);
        assert_eq!(extension("a.b/noext"), None);
    }

    #[test]
    fn sizes() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(1023), "1023 bytes");
        assert_eq!(size_label(1024), "1.00 KB");
        assert_eq!(size_label(1536), "1.50 KB");
        assert_eq!(size_label(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(size_label(250 * 1024 * 1024), "250 MB");
        assert_eq!(size_label(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
