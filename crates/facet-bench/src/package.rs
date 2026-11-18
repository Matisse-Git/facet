//! `facet-bench package`: the MSIX package around a shipping build (SPEC M4).
//!
//! ```text
//! cargo build --profile dist -p facet
//! cargo run --release -p facet-bench -- package [--exe PATH] [--publisher "CN=..."] [DIR]
//! ```
//!
//! Lays out `DIR/layout` (the executable, `AppxManifest.xml`, logos drawn by the same code as
//! the app icon) and packs it with the Windows SDK's `makeappx.exe`, which also validates the
//! manifest. The package is **unsigned**: installing it needs it signed with a certificate
//! whose subject equals `--publisher` (`signtool sign /fd SHA256 /a /f cert.pfx ...`), which
//! is the owner's call. `DIR` defaults to `target/msix`, `--exe` to `target/dist/facet.exe`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

// Shared with the app; only `render_on` is needed here (`render` draws the window icon).
#[allow(dead_code)]
#[path = "../../facet/src/icon_art.rs"]
mod icon_art;

const DEFAULT_OUT: &str = "target/msix";
const DEFAULT_EXE: &str = "target/dist/facet.exe";
const DEFAULT_PUBLISHER: &str = "CN=Facet";
/// Windows 11 21H2: Mica and the rounded-corner DWM attributes Facet uses.
const MIN_VERSION: &str = "10.0.22000.0";
const MAX_TESTED: &str = "10.0.26100.0";

/// Logos the manifest names: (file, width, height, icon size inside).
const LOGOS: [(&str, u32, u32, u32); 4] = [
    ("StoreLogo.png", 50, 50, 50),
    ("Square44x44Logo.png", 44, 44, 44),
    ("Square150x150Logo.png", 150, 150, 150),
    ("Wide310x150Logo.png", 310, 150, 150),
];

pub fn run(args: &[String]) -> Result<()> {
    let options = crate::parse_options(args, &[], &[], &["--exe", "--publisher", "--makeappx"])?;
    // `parse_options` fills in the benchmark repository when no DIR is given.
    let out = if options.dir == Path::new(crate::DEFAULT_DIR) {
        PathBuf::from(DEFAULT_OUT)
    } else {
        options.dir.clone()
    };
    let exe = PathBuf::from(options.text("--exe").unwrap_or(DEFAULT_EXE));
    if !exe.is_file() {
        return Err(format!(
            "{} not found; build it first with `cargo build --profile dist -p facet`",
            exe.display()
        ));
    }
    let publisher = options.text("--publisher").unwrap_or(DEFAULT_PUBLISHER);
    let makeappx = match options.text("--makeappx") {
        Some(path) => PathBuf::from(path),
        None => find_makeappx()
            .ok_or("makeappx.exe not found; install the Windows SDK or pass --makeappx PATH")?,
    };

    let layout = out.join("layout");
    if layout.exists() {
        std::fs::remove_dir_all(&layout)
            .map_err(|error| format!("clearing {}: {error}", layout.display()))?;
    }
    let assets = layout.join("Assets");
    std::fs::create_dir_all(&assets).map_err(|error| format!("{}: {error}", assets.display()))?;
    std::fs::copy(&exe, layout.join("facet.exe"))
        .map_err(|error| format!("copying {}: {error}", exe.display()))?;
    for (file, width, height, icon) in LOGOS {
        let png = png(width, height, &icon_art::render_on(width, height, icon))?;
        std::fs::write(assets.join(file), png).map_err(|error| format!("{file}: {error}"))?;
    }
    let version = package_version(env!("CARGO_PKG_VERSION"))?;
    std::fs::write(
        layout.join("AppxManifest.xml"),
        manifest(&version, publisher),
    )
    .map_err(|error| format!("AppxManifest.xml: {error}"))?;

    let package = out.join(format!("Facet_{version}_x64.msix"));
    let status = Command::new(&makeappx)
        .arg("pack")
        .arg("/o")
        .arg("/d")
        .arg(&layout)
        .arg("/p")
        .arg(&package)
        .status()
        .map_err(|error| format!("{}: {error}", makeappx.display()))?;
    if !status.success() {
        return Err(format!("makeappx failed: {status}"));
    }
    let size = std::fs::metadata(&package).map_or(0, |meta| meta.len());
    println!(
        "{} ({:.1} MB, unsigned; publisher {publisher})",
        package.display(),
        size as f64 / 1e6
    );
    Ok(())
}

/// The newest SDK's x64 `makeappx.exe`.
fn find_makeappx() -> Option<PathBuf> {
    let root = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join(r"Windows Kits\10\bin");
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(&root)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            let numbers: Vec<u32> = name
                .split('.')
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()
                .ok()?;
            let tool = entry.path().join(r"x64\makeappx.exe");
            tool.is_file().then_some((numbers, tool))
        })
        .collect();
    versions.sort();
    versions.pop().map(|(_, tool)| tool)
}

/// MSIX versions are four numbers; the crate's `major.minor.patch` gets a `.0`.
fn package_version(crate_version: &str) -> Result<String> {
    let core = crate_version
        .split(['-', '+'])
        .next()
        .unwrap_or(crate_version);
    let mut parts: Vec<u16> = core
        .split('.')
        .map(|part| {
            part.parse()
                .map_err(|_| format!("bad version {crate_version}"))
        })
        .collect::<Result<_>>()?;
    if parts.len() != 3 {
        return Err(format!("bad version {crate_version}"));
    }
    parts.push(0);
    Ok(parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join("."))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A full-trust desktop app with an execution alias, so `facet <folder>` works from a terminal
/// the way the plain executable does.
fn manifest(version: &str, publisher: &str) -> String {
    let publisher = xml_escape(publisher);
    let mut xml = String::new();
    let _ = write!(
        xml,
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap uap3 desktop rescap">
  <Identity Name="Facet" Publisher="{publisher}" Version="{version}" ProcessorArchitecture="x64" />
  <Properties>
    <DisplayName>Facet</DisplayName>
    <PublisherDisplayName>Facet</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="{MIN_VERSION}" MaxVersionTested="{MAX_TESTED}" />
  </Dependencies>
  <Resources>
    <Resource Language="en-us" />
  </Resources>
  <Applications>
    <Application Id="Facet" Executable="facet.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Facet" Description="A fast native Git GUI"
        BackgroundColor="transparent"
        Square150x150Logo="Assets\Square150x150Logo.png"
        Square44x44Logo="Assets\Square44x44Logo.png">
        <uap:DefaultTile Wide310x150Logo="Assets\Wide310x150Logo.png" />
      </uap:VisualElements>
      <Extensions>
        <uap3:Extension Category="windows.appExecutionAlias" Executable="facet.exe" EntryPoint="Windows.FullTrustApplication">
          <uap3:AppExecutionAlias>
            <desktop:ExecutionAlias Alias="facet.exe" />
          </uap3:AppExecutionAlias>
        </uap3:Extension>
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"#
    );
    xml
}

fn png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(rgba)
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_version_has_four_parts() {
        assert_eq!(package_version("0.1.0").unwrap(), "0.1.0.0");
        assert_eq!(package_version("1.2.3-beta.1").unwrap(), "1.2.3.0");
        assert!(package_version("1.2").is_err());
        assert!(package_version("1.x.3").is_err());
    }

    #[test]
    fn manifest_escapes_the_publisher_and_names_every_logo() {
        let xml = manifest("0.1.0.0", r#"CN=A & B, O="C""#);
        assert!(xml.contains(r#"Publisher="CN=A &amp; B, O=&quot;C&quot;""#));
        assert!(xml.contains(r#"Version="0.1.0.0""#));
        for (file, ..) in LOGOS {
            assert!(xml.contains(&format!(r"Assets\{file}")), "{file}");
        }
    }

    #[test]
    fn logos_encode_at_their_sizes() {
        for (file, width, height, icon) in LOGOS {
            let rgba = icon_art::render_on(width, height, icon);
            let bytes = png(width, height, &rgba).unwrap();
            let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
            let reader = decoder.read_info().unwrap();
            let info = reader.info();
            assert_eq!((info.width, info.height), (width, height), "{file}");
        }
    }
}
