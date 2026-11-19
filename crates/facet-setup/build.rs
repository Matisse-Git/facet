//! Facet Setup's build script:
//!
//! * the payload: the `facet.exe` named by `FACET_SETUP_PAYLOAD` is embedded into the setup
//!   (`scripts/build-installer.ps1` sets it to the `dist` build). Without it the setup is built
//!   with an empty payload, so `cargo build --workspace` / clippy / tests still work, and that
//!   setup refuses to run;
//! * the application manifest (asInvoker: no UAC prompt, even though the name says "setup";
//!   per-monitor DPI so the dialogs are crisp), the icon and the version resource, drawn and
//!   written by the same code as the app's (`crates/facet/build.rs`).

#[path = "../facet/src/icon_art.rs"]
mod icon_art;
#[path = "../facet/build/res.rs"]
mod res;

use std::path::PathBuf;

use embed_manifest::manifest::{ActiveCodePage, DpiAwareness, ExecutionLevel, SupportedOS};
use embed_manifest::{embed_manifest, new_manifest};

const ICON_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../facet/build/res.rs");
    println!("cargo:rerun-if-changed=../facet/src/icon_art.rs");
    println!("cargo:rerun-if-env-changed=FACET_SETUP_PAYLOAD");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let payload = match std::env::var_os("FACET_SETUP_PAYLOAD") {
        Some(path) if !path.is_empty() => {
            let path = PathBuf::from(path);
            assert!(
                path.is_file(),
                "FACET_SETUP_PAYLOAD points to {}, which does not exist",
                path.display()
            );
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::canonicalize(&path).expect("payload path")
        }
        _ => {
            let empty = out.join("empty-payload.bin");
            std::fs::write(&empty, []).expect("unable to write the empty payload");
            empty
        }
    };
    println!(
        "cargo:rustc-env=FACET_SETUP_PAYLOAD_FILE={}",
        payload.display()
    );

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = new_manifest("Facet.Setup")
        .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
        .active_code_page(ActiveCodePage::Utf8)
        .dpi_awareness(DpiAwareness::PerMonitorV2)
        .requested_execution_level(ExecutionLevel::AsInvoker);
    embed_manifest(manifest).expect("unable to embed the application manifest");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let path = out.join("facet-setup.res");
    std::fs::write(&path, resources()).expect("unable to write facet-setup.res");
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
            ("FileDescription", "Facet Setup"),
            ("FileVersion", version),
            ("InternalName", "facet-setup"),
            ("OriginalFilename", "facet-setup.exe"),
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
