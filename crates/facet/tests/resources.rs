//! The executable carries its own icon and version resource (`build.rs`): read them back the
//! way Explorer does.

#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HGDIOBJ};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO,
};

fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

fn exe() -> Vec<u16> {
    wide(env!("CARGO_BIN_EXE_facet"))
}

#[test]
fn version_resource_names_facet_and_the_crate_version() {
    let exe = exe();
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(exe.as_ptr()), None);
        assert!(size > 0, "no version resource");
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(exe.as_ptr()), None, size, data.as_mut_ptr().cast()).unwrap();

        let query = |path: &str| -> (*const u8, u32) {
            let path = wide(path);
            let mut pointer = std::ptr::null_mut();
            let mut length = 0u32;
            assert!(
                VerQueryValueW(
                    data.as_ptr().cast(),
                    PCWSTR(path.as_ptr()),
                    &mut pointer,
                    &mut length
                )
                .as_bool(),
                "{} missing",
                String::from_utf16_lossy(&path)
            );
            (pointer as *const u8, length)
        };
        let text = |key: &str| {
            let (pointer, length) = query(&format!("\\StringFileInfo\\040904B0\\{key}"));
            let units = std::slice::from_raw_parts(pointer as *const u16, length as usize);
            String::from_utf16_lossy(units)
                .trim_end_matches('\0')
                .to_owned()
        };
        assert_eq!(text("FileDescription"), "Facet");
        assert_eq!(text("ProductName"), "Facet");
        assert_eq!(text("OriginalFilename"), "facet.exe");
        assert_eq!(text("FileVersion"), env!("CARGO_PKG_VERSION"));

        let (pointer, length) = query("\\");
        assert_eq!(length as usize, std::mem::size_of::<VS_FIXEDFILEINFO>());
        let fixed = &*(pointer as *const VS_FIXEDFILEINFO);
        assert_eq!(fixed.dwSignature, 0xFEEF_04BD);
        let major: u32 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
        let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
        let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
        assert_eq!(fixed.dwFileVersionMS, (major << 16) | minor);
        assert_eq!(fixed.dwFileVersionLS, patch << 16);
    }
}

#[test]
fn icon_loads_at_every_size_including_the_png_image() {
    let exe = exe();
    let mut path = [0u16; 260];
    path[..exe.len()].copy_from_slice(&exe);
    for size in [16, 32, 48, 256] {
        unsafe {
            let mut icon = [HICON::default()];
            let loaded = PrivateExtractIconsW(&path, 0, size, size, Some(&mut icon), None, 0);
            assert_eq!(loaded, 1, "no {size}px icon");
            let mut info = ICONINFO::default();
            GetIconInfo(icon[0], &mut info).unwrap();
            let mut bitmap = BITMAP::default();
            let read = GetObjectW(
                HGDIOBJ(info.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            );
            assert!(read > 0);
            assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (size, size));
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            DestroyIcon(icon[0]).unwrap();
        }
    }
}
