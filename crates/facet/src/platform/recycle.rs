//! Moving a folder to the Recycle Bin, and never deleting it for good.
//!
//! The shell deletes permanently when an item cannot be recycled (a network share, a removable
//! drive without a Recycle Bin, a folder bigger than the bin's size limit). It announces that
//! decision to the progress sink before acting: `PreDeleteItem` arrives *without*
//! `TSF_DELETE_RECYCLE_IF_POSSIBLE`. The sink here aborts the operation at that point, so the
//! folder stays where it is and the caller gets [`RecycleError::WouldDelete`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{implement, Ref, Result as WinResult, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::E_ABORT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
    IShellItem, SHCreateItemFromParsingName, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE,
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, TSF_DELETE_RECYCLE_IF_POSSIBLE,
};

/// Why a folder was not moved to the Recycle Bin. In every case it is still where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecycleError {
    /// Windows would have deleted it permanently (no Recycle Bin on that drive, or too big).
    WouldDelete,
    /// Anything else, in Windows' words (a file in use, access denied…).
    Failed(String),
}

impl std::fmt::Display for RecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WouldDelete => f.write_str(
                "Windows can't put this folder in the Recycle Bin (its drive has none, or the \
                 folder is larger than the Recycle Bin allows) and would delete it permanently \
                 instead. Facet never does that, so the folder was left where it is.",
            ),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Move `folder` to the Recycle Bin without any shell UI. **Blocks** until done (large folders
/// take a while), so call it from its own thread, never the UI thread.
pub fn move_to_recycle_bin(folder: &Path) -> Result<(), RecycleError> {
    let refused = Arc::new(AtomicBool::new(false));
    // SAFETY: standard COM usage on a thread initialised as a single-threaded apartment for
    // the duration of the call; interface pointers are owned by `windows` smart pointers.
    let result = unsafe {
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let result = (|| -> WinResult<bool> {
            let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            operation.SetOperationFlags(
                FOF_ALLOWUNDO
                    | FOFX_RECYCLEONDELETE
                    | FOF_NOCONFIRMATION
                    | FOF_NOERRORUI
                    | FOF_SILENT
                    | FOFX_EARLYFAILURE,
            )?;
            let sink: IFileOperationProgressSink = RecycleOnly {
                refused: refused.clone(),
            }
            .into();
            operation.Advise(&sink)?;
            let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(folder), None)?;
            operation.DeleteItem(&item, None::<&IFileOperationProgressSink>)?;
            let performed = operation.PerformOperations();
            let aborted = operation.GetAnyOperationsAborted()?.as_bool();
            performed.map(|()| !aborted)
        })();
        if initialized {
            CoUninitialize();
        }
        result
    };
    if refused.load(Ordering::SeqCst) {
        return Err(RecycleError::WouldDelete);
    }
    match result {
        Ok(true) if !folder.exists() => Ok(()),
        Ok(true) => Err(RecycleError::Failed(
            "Windows reported success, but the folder is still there.".to_owned(),
        )),
        Ok(false) => Err(RecycleError::Failed("The move was cancelled.".to_owned())),
        Err(error) => Err(RecycleError::Failed(error.message())),
    }
}

/// Lets an item go to the Recycle Bin and stops the operation before any permanent delete.
#[implement(IFileOperationProgressSink)]
struct RecycleOnly {
    refused: Arc<AtomicBool>,
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for RecycleOnly_Impl {
    fn PreDeleteItem(&self, dwflags: u32, _item: Ref<IShellItem>) -> WinResult<()> {
        if dwflags & TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32 == 0 {
            self.refused.store(true, Ordering::SeqCst);
            return Err(E_ABORT.into());
        }
        Ok(())
    }

    fn StartOperations(&self) -> WinResult<()> {
        Ok(())
    }
    fn FinishOperations(&self, _result: HRESULT) -> WinResult<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> WinResult<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> WinResult<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _done: u32) -> WinResult<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> WinResult<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> WinResult<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> WinResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_is_reported_and_nothing_else_happens() {
        let missing = std::env::temp_dir().join("facet-recycle-test-does-not-exist");
        assert!(!missing.exists());
        assert!(matches!(
            move_to_recycle_bin(&missing),
            Err(RecycleError::Failed(_))
        ));
    }

    /// Puts a small folder in the Recycle Bin of the machine running it, so it only runs on
    /// request: `cargo test -p facet recycle -- --ignored`.
    #[test]
    #[ignore = "writes to the Recycle Bin"]
    fn a_folder_goes_to_the_recycle_bin() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("facet recycle test");
        std::fs::create_dir_all(folder.join(".git").join("objects")).unwrap();
        let object = folder.join(".git").join("objects").join("read-only");
        std::fs::write(&object, "x").unwrap();
        let mut permissions = std::fs::metadata(&object).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&object, permissions).unwrap();

        move_to_recycle_bin(&folder).unwrap();
        assert!(!folder.exists());
    }
}
