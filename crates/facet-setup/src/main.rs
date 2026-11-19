//! Facet Setup: installs Facet for the current user, updates an existing install in place, and
//! uninstalls it (it copies itself next to Facet as `uninstall.exe` for Settings → Apps).
//!
//! ```text
//! facet-setup.exe                 install or update, then offer to open Facet
//! facet-setup.exe --quiet         the same without any dialog (exit code 0 = success)
//! uninstall.exe --uninstall       ask, then remove Facet (settings in %APPDATA%\Facet stay)
//! uninstall.exe --uninstall --quiet
//! ```
//!
//! Per-user, no admin rights: Facet goes to `%LOCALAPPDATA%\Programs\Facet`, a shortcut to the
//! Start menu, and the Settings → Apps entry to `HKCU\…\Uninstall\Facet`.
//!
//! Updating while Facet is open works: Windows lets a running program file be renamed (not
//! deleted), so the old `facet.exe` is moved aside as `*.old` and the new one written in its
//! place. The open window keeps running the old version until it is closed; the leftover file
//! is deleted by the next setup run. The setup then says to reopen Facet, and does not offer to
//! open it (a second launch would only bring the old window to the front, see `instance.rs`).

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("Facet Setup runs on Windows only.");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    std::process::exit(setup::main());
}

#[cfg(windows)]
mod setup {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO,
        MESSAGEBOX_STYLE,
    };

    /// The `dist` build of `facet.exe`, embedded by `build.rs` (empty in everyday builds).
    const PAYLOAD: &[u8] = include_bytes!(env!("FACET_SETUP_PAYLOAD_FILE"));
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    /// The commit the payload was built from (`scripts/build-installer.ps1`).
    const BUILD: Option<&str> = option_env!("FACET_BUILD_ID");

    const TITLE: &str = "Facet Setup";
    const EXE: &str = "facet.exe";
    const UNINSTALLER: &str = "uninstall.exe";
    const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Facet";

    struct Options {
        uninstall: bool,
        quiet: bool,
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let options = Options {
            uninstall: args.iter().any(|arg| arg == "--uninstall"),
            quiet: args.iter().any(|arg| arg == "--quiet" || arg == "/S"),
        };
        // SAFETY: once, on this thread, before any COM call.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let result = if options.uninstall {
            uninstall(&options)
        } else {
            install(&options)
        };
        match result {
            Ok(()) => 0,
            Err(message) => {
                if options.quiet {
                    eprintln!("{message}");
                } else {
                    message_box(&message, MB_OK | MB_ICONERROR);
                }
                1
            }
        }
    }

    fn display_version() -> String {
        match BUILD {
            Some(build) if !build.is_empty() => format!("{VERSION} ({build})"),
            _ => VERSION.to_owned(),
        }
    }

    fn install_dir() -> Result<PathBuf, String> {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or("The LOCALAPPDATA folder is not set for this user.")?;
        Ok(PathBuf::from(local).join("Programs").join("Facet"))
    }

    fn shortcut_path() -> Result<PathBuf, String> {
        let roaming =
            std::env::var_os("APPDATA").ok_or("The APPDATA folder is not set for this user.")?;
        Ok(PathBuf::from(roaming)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("Facet.lnk"))
    }

    // ---------------------------------------------------------------------------------------
    // Install / update

    fn install(options: &Options) -> Result<(), String> {
        if PAYLOAD.is_empty() {
            return Err(
                "This copy of Facet Setup was built without Facet inside it.\n\n\
                 Build the installer with scripts\\build-installer.ps1 (or run update.cmd)."
                    .to_owned(),
            );
        }
        let dir = install_dir()?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("Couldn't create {}: {error}", dir.display()))?;
        remove_leftovers(&dir);

        let previous = installed_version();
        let exe = dir.join(EXE);
        let still_running = replace_file(&exe, PAYLOAD)?;

        // The uninstaller is this setup itself; skip when this *is* the installed uninstaller.
        let me = std::env::current_exe().map_err(|error| error.to_string())?;
        let uninstaller = dir.join(UNINSTALLER);
        if !same_file(&me, &uninstaller) {
            let bytes = std::fs::read(&me)
                .map_err(|error| format!("Couldn't read {}: {error}", me.display()))?;
            replace_file(&uninstaller, &bytes)?;
        }

        create_shortcut(&shortcut_path()?, &exe, &dir)?;
        register(&dir, &exe, &uninstaller)?;

        let now = display_version();
        let what = match &previous {
            None => format!("Facet {now} is installed."),
            Some(old) if *old == now => format!("Facet {now} was reinstalled."),
            Some(old) => format!("Facet was updated from {old} to {now}."),
        };
        if options.quiet {
            println!("{what}");
            if still_running {
                println!("Facet is open: close it and open it again to use the new version.");
            }
            return Ok(());
        }
        if still_running {
            message_box(
                &format!(
                    "{what}\n\nFacet is still open and running the previous version. \
                     Close it and open it again to use the new one."
                ),
                MB_OK | MB_ICONINFORMATION,
            );
        } else if message_box(
            &format!("{what}\n\nYou'll find it in the Start menu. Open Facet now?"),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES.0
        {
            let home = std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.clone());
            Command::new(&exe)
                .current_dir(home)
                .spawn()
                .map_err(|error| format!("Couldn't open Facet: {error}"))?;
        }
        Ok(())
    }

    /// Write `bytes` to `target`, moving a file already there aside first (which works while it
    /// runs). Returns whether the old file is still in use, i.e. Facet is open.
    fn replace_file(target: &Path, bytes: &[u8]) -> Result<bool, String> {
        let staging = target.with_extension("new");
        std::fs::write(&staging, bytes)
            .map_err(|error| format!("Couldn't write {}: {error}", staging.display()))?;

        let mut aside = None;
        if target.exists() {
            let old = leftover_name(target);
            if let Err(error) = std::fs::rename(target, &old) {
                let _ = std::fs::remove_file(&staging);
                return Err(format!(
                    "Couldn't replace {}: {error}\n\nClose Facet and run the setup again.",
                    target.display()
                ));
            }
            aside = Some(old);
        }
        if let Err(error) = std::fs::rename(&staging, target) {
            if let Some(old) = &aside {
                let _ = std::fs::rename(old, target);
            }
            let _ = std::fs::remove_file(&staging);
            return Err(format!("Couldn't write {}: {error}", target.display()));
        }
        Ok(match aside {
            Some(old) => std::fs::remove_file(old).is_err(),
            None => false,
        })
    }

    fn leftover_name(target: &Path) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default();
        let name = target
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        target.with_file_name(format!("{name}-{stamp}.old"))
    }

    /// Program files moved aside by earlier updates whose process has exited by now.
    fn remove_leftovers(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("old") || ext == "new")
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    fn create_shortcut(link: &Path, exe: &Path, dir: &Path) -> Result<(), String> {
        let fail = |error: windows::core::Error| {
            format!(
                "Couldn't create the Start menu shortcut {}: {error}",
                link.display()
            )
        };
        if let Some(parent) = link.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // SAFETY: COM is initialized on this thread; every argument outlives its call.
        unsafe {
            let shell: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(fail)?;
            shell.SetPath(&HSTRING::from(exe)).map_err(fail)?;
            shell
                .SetWorkingDirectory(&HSTRING::from(dir))
                .map_err(fail)?;
            shell
                .SetDescription(&HSTRING::from("Facet: a fast Git GUI"))
                .map_err(fail)?;
            shell
                .SetIconLocation(&HSTRING::from(exe), 0)
                .map_err(fail)?;
            let file: IPersistFile = shell.cast().map_err(fail)?;
            file.Save(&HSTRING::from(link), true).map_err(fail)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Settings → Apps entry

    enum Value {
        Text(String),
        Number(u32),
    }

    fn register(dir: &Path, exe: &Path, uninstaller: &Path) -> Result<(), String> {
        let size_kb = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|meta| meta.len())
                    .sum::<u64>()
                    / 1024
            })
            .unwrap_or_default();
        let today = today();
        let values = [
            ("DisplayName", Value::Text("Facet".into())),
            ("DisplayVersion", Value::Text(display_version())),
            ("Publisher", Value::Text("Facet".into())),
            ("DisplayIcon", Value::Text(format!("{},0", exe.display()))),
            ("InstallLocation", Value::Text(dir.display().to_string())),
            (
                "UninstallString",
                Value::Text(format!("\"{}\" --uninstall", uninstaller.display())),
            ),
            (
                "QuietUninstallString",
                Value::Text(format!("\"{}\" --uninstall --quiet", uninstaller.display())),
            ),
            ("InstallDate", Value::Text(today)),
            ("NoModify", Value::Number(1)),
            ("NoRepair", Value::Number(1)),
            (
                "EstimatedSize",
                Value::Number(u32::try_from(size_kb).unwrap_or(u32::MAX)),
            ),
        ];

        let mut key = HKEY::default();
        // SAFETY: `key` is written by the call and closed below.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(UNINSTALL_KEY),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "Couldn't register Facet in Settings → Apps (error {}).",
                status.0
            ));
        }
        let mut result = Ok(());
        for (name, value) in values {
            let (kind, data) = match value {
                Value::Text(text) => (REG_SZ, wide_bytes(&text)),
                Value::Number(number) => (REG_DWORD, number.to_le_bytes().to_vec()),
            };
            // SAFETY: `key` is open; `data` is in the layout `kind` names.
            let status =
                unsafe { RegSetValueExW(key, &HSTRING::from(name), None, kind, Some(&data)) };
            if status != ERROR_SUCCESS {
                result = Err(format!(
                    "Couldn't write {name} for Settings → Apps (error {}).",
                    status.0
                ));
                break;
            }
        }
        // SAFETY: opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        result
    }

    /// The installed version as Settings → Apps shows it, if Facet is installed.
    fn installed_version() -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        // SAFETY: `buffer` holds `size` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(UNINSTALL_KEY),
                &HSTRING::from("DisplayVersion"),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }

    /// `YYYYMMDD`, as Settings → Apps expects `InstallDate`.
    fn today() -> String {
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_secs() / 86_400)
            .unwrap_or_default() as i64;
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}{month:02}{day:02}")
    }

    fn wide_bytes(text: &str) -> Vec<u8> {
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    // ---------------------------------------------------------------------------------------
    // Uninstall

    fn uninstall(options: &Options) -> Result<(), String> {
        let dir = install_dir()?;
        if !options.quiet
            && message_box(
                "Remove Facet from this PC?\n\nYour repositories and Facet's settings are not \
                 touched.",
                MB_YESNO | MB_ICONQUESTION,
            ) != IDYES.0
        {
            return Ok(());
        }

        // Facet must be closed. A window opened before an update runs a `*.old` file: one that
        // can't be deleted is still running.
        remove_leftovers(&dir);
        let leftover_running = std::fs::read_dir(&dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("old"))
            })
        });
        if leftover_running {
            return Err("Facet is open. Close it, then uninstall again.".to_owned());
        }
        // Then try moving the program aside and deleting it.
        let exe = dir.join(EXE);
        if exe.exists() {
            let aside = leftover_name(&exe);
            std::fs::rename(&exe, &aside)
                .map_err(|error| format!("Couldn't remove {}: {error}", exe.display()))?;
            if std::fs::remove_file(&aside).is_err() {
                let _ = std::fs::rename(&aside, &exe);
                return Err("Facet is open. Close it, then uninstall again.".to_owned());
            }
        }

        if let Ok(link) = shortcut_path() {
            let _ = std::fs::remove_file(link);
        }
        // SAFETY: plain call with a valid key and path.
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
        }

        // This program may be running from the folder it removes: delete everything else now
        // and leave the folder to a detached `cmd` that waits for this process to exit.
        let me = std::env::current_exe().ok();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if me.as_deref().is_some_and(|me| same_file(me, &path)) {
                    continue;
                }
                let _ = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
            }
        }
        let running_inside = me
            .as_deref()
            .and_then(Path::parent)
            .is_some_and(|parent| same_file(parent, &dir));
        if running_inside {
            let temp = std::env::temp_dir();
            let _ = Command::new("cmd.exe")
                .raw_arg(format!(
                    "/d /s /c \"ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"\"",
                    dir.display()
                ))
                .current_dir(temp)
                .creation_flags(CREATE_NO_WINDOW.0)
                .spawn();
        } else {
            let _ = std::fs::remove_dir(&dir);
        }

        if options.quiet {
            println!("Facet was removed.");
        } else {
            message_box("Facet was removed.", MB_OK | MB_ICONINFORMATION);
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------

    fn message_box(text: &str, style: MESSAGEBOX_STYLE) -> i32 {
        // SAFETY: both strings outlive the call.
        unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(TITLE), style).0 }
    }
}
