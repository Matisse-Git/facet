//! Windows resources for the executable:
//!
//! * the application manifest: per-monitor-v2 DPI awareness, long-path awareness, UTF-8 code
//!   page, and the Windows 10/11 compatibility declaration;
//! * the icon (Explorer, the taskbar before the window exists, shortcuts) drawn by
//!   `src/icon_art.rs`, the same drawing as the window icon;
//! * the version resource (Explorer's Details tab, Task Manager's name).
//!
//! The icon and version go into a `.res` written by `build/res.rs`, so no `rc.exe` is needed.

#[path = "src/icon_art.rs"]
mod icon_art;
#[path = "build/res.rs"]
mod res;

use embed_manifest::manifest::{ActiveCodePage, DpiAwareness, Setting, SupportedOS};
use embed_manifest::{embed_manifest, new_manifest};

/// Explorer and the shell pick the size closest to what they draw; 20/40 cover 125% scaling.
const ICON_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/res.rs");
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = new_manifest("Facet")
        .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
        .active_code_page(ActiveCodePage::Utf8)
        .dpi_awareness(DpiAwareness::PerMonitorV2)
        .long_path_aware(Setting::Enabled);
    embed_manifest(manifest).expect("unable to embed the application manifest");

    // `link.exe` reads `.res` files; other linkers would need a COFF object instead.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let path = out.join("facet.res");
    std::fs::write(&path, resources()).expect("unable to write facet.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}

fn resources() -> Vec<u8> {
    let pixels: Vec<(u32, Vec<u8>)> = ICON_SIZES
        .iter()
        .map(|&size| (size, icon_art::render(size)))
        .collect();
    let images: Vec<res::IconImage> = pixels
        .iter()
        .map(|(size, rgba)| res::IconImage { size: *size, rgba })
        .collect();

    let version = env!("CARGO_PKG_VERSION");
    let numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    let numbers = [numbers[0], numbers[1], numbers[2], 0];

    let mut resources = res::Resources::new();
    resources.icon_group(1, &images, |image| png(image.size, image.rgba));
    resources.version(&res::Version {
        numbers,
        strings: &[
            ("CompanyName", "Facet"),
            ("FileDescription", "Facet"),
            ("FileVersion", version),
            ("InternalName", "facet"),
            ("OriginalFilename", "facet.exe"),
            ("ProductName", "Facet"),
            ("ProductVersion", version),
        ],
    });
    resources.into_bytes()
}

fn png(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
    writer.finish().expect("png end");
    bytes
}
