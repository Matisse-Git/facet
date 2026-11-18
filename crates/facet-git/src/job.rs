//! Windows Job Objects around git processes (PLAN.md, G6).
//!
//! Killing `git.exe` does not kill what it started: hooks run through `sh`, `ssh`, credential
//! helpers, `git-remote-https`. Every command therefore runs inside its own job created with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`:
//!
//! * cancelling terminates the job, which ends the whole tree at once;
//! * dropping a running command (tab closed, app exiting) closes the job handle, which the
//!   kernel turns into the same kill;
//! * a command that finishes normally first clears the kill limit, so whatever git
//!   deliberately detached (`git gc --auto` after a fetch) keeps running.
//!
//! Processes git starts after it was assigned inherit the job (nested jobs: Windows 8+).
//! There is a window of a few microseconds between spawning `git.exe` and assigning it, in
//! which git cannot have started anything yet: it is still loading its DLLs.

#[cfg(windows)]
mod imp {
    use std::os::windows::io::RawHandle;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Exit code given to processes killed by cancellation (`ERROR_CANCELLED`).
    const CANCELLED_EXIT_CODE: u32 = 1223;

    pub(crate) struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle; it may be used from any thread.
    unsafe impl Send for Job {}
    // SAFETY: all operations on the handle are thread-safe kernel calls.
    unsafe impl Sync for Job {}

    impl Job {
        /// A new anonymous job whose processes die when the last handle to it closes.
        pub(crate) fn new() -> std::io::Result<Self> {
            // SAFETY: no security attributes, no name; the handle is owned by `Job`.
            let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }?;
            let job = Self(handle);
            job.set_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
            Ok(job)
        }

        fn set_limits(&self, flags: JOB_OBJECT_LIMIT) -> std::io::Result<()> {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = flags;
            // SAFETY: `info` is a properly sized, initialised structure of the declared class.
            unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            }?;
            Ok(())
        }

        /// Put a freshly spawned process (and so everything it starts) into the job.
        pub(crate) fn assign(&self, process: RawHandle) -> std::io::Result<()> {
            // SAFETY: `process` is the live handle of a child we own.
            unsafe { AssignProcessToJobObject(self.0, HANDLE(process)) }?;
            Ok(())
        }

        /// Kill every process in the job now.
        pub(crate) fn terminate(&self) {
            // SAFETY: valid job handle; failure (job already empty) is harmless.
            let _ = unsafe { TerminateJobObject(self.0, CANCELLED_EXIT_CODE) };
        }

        /// The command finished normally: let detached children outlive the handle.
        pub(crate) fn release(self) {
            let _ = self.set_limits(JOB_OBJECT_LIMIT(0));
            // `Drop` closes the handle; without the kill limit that ends nothing.
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle and close it exactly once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// Elsewhere, process groups would do the same job; Facet ships for Windows only, so
    /// this keeps the crate compiling and falls back to killing git itself.
    pub(crate) struct Job;

    impl Job {
        pub(crate) fn new() -> std::io::Result<Self> {
            Err(std::io::Error::other("job objects are Windows-only"))
        }
        pub(crate) fn terminate(&self) {}
        pub(crate) fn release(self) {}
    }
}

pub(crate) use imp::Job;
