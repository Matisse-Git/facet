//! Facet — a fast native Git GUI for Windows 11.
//!
//! See `SPEC.md` for requirements and `PLAN.md` for architecture and status.

// No console window in release builds; keep it in debug builds for diagnostics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod app_work;
mod filekind;
mod format;
mod fuzzy;
mod highlight;
mod icon;
mod loader;
mod platform;
mod settings;
mod textdiff;
mod theme;
mod ui;
mod watcher;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;

use crate::settings::Settings;

fn main() -> eframe::Result {
    let startup = Instant::now();
    let settings = Settings::load();

    // Background work (git processes, disk writes) runs here; the UI thread never blocks.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("facet-worker")
        .enable_all()
        .build()
        .expect("failed to start the background runtime");

    // `facet <path>` opens that repository. Anything that looks like a flag is ignored.
    let open_on_launch = std::env::args_os()
        .skip(1)
        .find(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(PathBuf::from);

    // A transparent swapchain is what lets the Mica backdrop show through: on DX12 that needs
    // a DirectComposition-backed swapchain rather than the default HWND one.
    let mut wgpu_setup = WgpuSetupCreateNew::without_display_handle();
    wgpu_setup.instance_descriptor.backends =
        wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
    wgpu_setup
        .instance_descriptor
        .backend_options
        .dx12
        .presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    // Adapter choice is left at eframe's default (high performance; override with
    // WGPU_POWER_PREF). Forcing the low-power adapter looked attractive for a git client, but
    // on a machine whose monitor hangs off the discrete card it presents across adapters and
    // pins a thread at 100% CPU. See PLAN.md, "Gotchas".

    // Favor a small footprint over allocator throughput: Facet draws a few thousand triangles
    // per frame, so wgpu's large pre-sized memory blocks buy nothing.
    let default_descriptor = Arc::clone(&wgpu_setup.device_descriptor);
    wgpu_setup.device_descriptor = Arc::new(move |adapter| wgpu::DeviceDescriptor {
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..default_descriptor(adapter)
    });

    let viewport = egui::ViewportBuilder::default()
        .with_title("Facet")
        .with_app_id("Facet")
        .with_icon(icon::app_icon())
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([720.0, 480.0])
        .with_transparent(true);

    let options = eframe::NativeOptions {
        viewport,
        // First launch: centered. Later launches restore the saved placement (see `Chrome`).
        centered: settings.window.is_none(),
        // Window placement is persisted by Facet itself; eframe's version does not account
        // for the custom title bar.
        persist_window: false,
        wgpu_options: WgpuConfiguration {
            wgpu_setup: WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "Facet",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::FacetApp::new(
                cc,
                settings,
                runtime,
                startup,
                open_on_launch,
            )))
        }),
    )
}
