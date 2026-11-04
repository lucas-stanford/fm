//! Killing the tree a child started, and only that tree.

use std::process::{Child, Command, ExitStatus};

pub(super) use imp::{Tree, prepare, signal_name};

#[cfg(all(windows, test))]
pub(super) use imp::{CREATE_NO_WINDOW, creation_flags};

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    use win32job::{ExtendedLimitInfo, Job};

    use super::{Child, Command, ExitStatus};

    /// `CREATE_NO_WINDOW`: a console child gets a hidden console instead of a new window.
    pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The creation flags every child is started with.
    pub(crate) fn creation_flags() -> u32 {
        CREATE_NO_WINDOW
    }

    pub(crate) fn prepare(cmd: &mut Command) {
        cmd.creation_flags(creation_flags());
    }

    /// The child's Job Object. Dropping it ends every process in it.
    pub(crate) struct Tree {
        job: Option<Job>,
        pid: u32,
    }

    impl Tree {
        pub(crate) fn attach(child: &Child) -> Self {
            let job = (|| {
                let mut info = ExtendedLimitInfo::new();
                info.limit_kill_on_job_close();
                let job = Job::create_with_limit_info(&info).ok()?;
                job.assign_process(child.as_raw_handle() as isize).ok()?;
                Some(job)
            })();
            Self {
                job,
                pid: child.id(),
            }
        }

        /// End the child and everything it started.
        pub(crate) fn kill(&mut self, child: &mut Child) {
            if self.job.take().is_none() && matches!(child.try_wait(), Ok(None)) {
                // No job: walk our child's own tree. Its PID cannot be reused while `child` holds
                // its handle, so this names our process and nothing else.
                let _ = Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &self.pid.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .status();
            }
            let _ = child.kill();
        }

        /// The child has ended: whatever it left running ends with the job.
        pub(crate) fn finish(&mut self, _child: &mut Child, _pipes_open: bool) {
            self.job.take();
        }
    }

    pub(crate) fn signal_name(_status: &ExitStatus) -> Option<String> {
        None
    }
}

#[cfg(unix)]
mod imp {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    use super::{Child, Command, ExitStatus};
    use crate::process::KILL_GRACE;

    pub(crate) fn prepare(cmd: &mut Command) {
        cmd.process_group(0);
    }

    /// The child's process group, which it leads.
    pub(crate) struct Tree {
        pgid: Option<Pid>,
    }

    impl Tree {
        pub(crate) fn attach(child: &Child) -> Self {
            Self {
                pgid: i32::try_from(child.id()).ok().map(Pid::from_raw),
            }
        }

        /// `SIGTERM` to the group, `SIGKILL` after [`KILL_GRACE`] or as soon as the child is gone.
        pub(crate) fn kill(&mut self, child: &mut Child) {
            let Some(pgid) = self.pgid else {
                let _ = child.kill();
                return;
            };
            let _ = killpg(pgid, Signal::SIGTERM);
            let until = Instant::now() + KILL_GRACE;
            while Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
                thread::sleep(Duration::from_millis(10));
            }
            // The group id stays allocated while any member lives, so this reaches only our tree.
            let _ = killpg(pgid, Signal::SIGKILL);
            let _ = child.kill();
            self.pgid = None;
        }

        /// The child has ended. Members still holding its output pipes are killed with the group.
        pub(crate) fn finish(&mut self, _child: &mut Child, pipes_open: bool) {
            if let (Some(pgid), true) = (self.pgid.take(), pipes_open) {
                let _ = killpg(pgid, Signal::SIGKILL);
            }
        }
    }

    pub(crate) fn signal_name(status: &ExitStatus) -> Option<String> {
        let sig = status.signal()?;
        Some(
            Signal::try_from(sig)
                .map(|s| s.as_str().to_string())
                .unwrap_or_else(|_| format!("signal {sig}")),
        )
    }
}
