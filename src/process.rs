//! Child-process helpers: no console windows, UTF-8 output from Python tools,
//! and killing a whole process tree (yt-dlp.exe is a PyInstaller bootstrapper
//! that spawns a second process, which in turn spawns ffmpeg).

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A tokio command that never flashes a console window and never reads stdin.
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    // yt-dlp is Python: force UTF-8 on pipes so non-ASCII/emoji titles survive.
    cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    cmd
}

/// Same as [`command`] but for the blocking std API (used for fire-and-forget launches).
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub struct Captured {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Run a program to completion and capture its output, with a timeout.
pub async fn run_capture<I, S>(
    program: impl AsRef<OsStr>,
    args: I,
    timeout: Duration,
) -> std::io::Result<Captured>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = command(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn()?;
    let tree = ProcessTree::attach(&child);
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(out) => {
            let out = out?;
            Ok(Captured {
                status: out.status,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
        Err(_) => {
            tree.kill();
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "process did not finish in time",
            ))
        }
    }
}

/// Handle to a child and all of its descendants. Dropping it kills whatever is
/// still running in the tree, so an abandoned job can never leak processes.
pub struct ProcessTree {
    #[cfg(windows)]
    job: Option<job::Job>,
    pid: Option<u32>,
}

impl ProcessTree {
    pub fn attach(child: &tokio::process::Child) -> Self {
        #[cfg(windows)]
        {
            let job = child.raw_handle().and_then(|h| {
                let job = job::Job::new()?;
                job.assign(h as _).then_some(job)
            });
            Self {
                job,
                pid: child.id(),
            }
        }
        #[cfg(not(windows))]
        {
            Self { pid: child.id() }
        }
    }

    /// Kill the child and every process it spawned.
    pub fn kill(&self) {
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate();
                return;
            }
            // Fallback when the job object could not be set up.
            if let Some(pid) = self.pid {
                let _ = std_command("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        #[cfg(not(windows))]
        if let Some(pid) = self.pid {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
    }
}

#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub struct Job(HANDLE);

    // The handle is only used through thread-safe Win32 calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                Some(Self(handle))
            }
        }

        pub fn assign(&self, process: HANDLE) -> bool {
            unsafe { AssignProcessToJobObject(self.0, process) != 0 }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
