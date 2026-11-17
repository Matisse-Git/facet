//! Embeds the Windows application manifest: per-monitor-v2 DPI awareness, long-path awareness,
//! UTF-8 code page, and the Windows 10/11 compatibility declaration.

use embed_manifest::manifest::{ActiveCodePage, DpiAwareness, Setting, SupportedOS};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let manifest = new_manifest("Facet")
            .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10)
            .active_code_page(ActiveCodePage::Utf8)
            .dpi_awareness(DpiAwareness::PerMonitorV2)
            .long_path_aware(Setting::Enabled);
        embed_manifest(manifest).expect("unable to embed the application manifest");
    }
}
