//! The background jobs a shell command leaves running, followed until they
//! end so that the host is told.
//!
//! A command's jobs share its process group, which leads a session of its
//! own: the group is empty once every job ended. The jobs are not the host's
//! children, so nothing signals their end: the group is checked every
//! [`POLL`]. A job that leaves the group (`setsid`, a daemon) is not
//! followed; members under other credentials (`sudo -b`) keep the group
//! running.

use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::ends::Ends;

/// How often a followed group is checked.
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(250);

/// How long [`ShellJobs::close`] waits for the groups it stops.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// A shell tool's followed jobs, shared by its clones.
#[derive(Debug)]
pub(crate) struct ShellJobs {
    pub(crate) ends: Ends,
    /// Cancelled by close and detach: following stops.
    closed: CancellationToken,
    /// Cancelled by detach, before `closed`: the groups are let go.
    detached: CancellationToken,
    /// How many groups are followed.
    followed: watch::Sender<usize>,
}

impl Default for ShellJobs {
    fn default() -> Self {
        Self {
            ends: Ends::new(),
            closed: CancellationToken::new(),
            detached: CancellationToken::new(),
            followed: watch::channel(0).0,
        }
    }
}

impl ShellJobs {
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Stop following; stop every followed group unless `detach`, then wait
    /// [`CLOSE_WAIT`] at most for them. Whether every group was let go.
    pub(crate) async fn close(&self, detach: bool) -> bool {
        if detach {
            self.detached.cancel();
        }
        self.closed.cancel();
        let mut followed = self.followed.subscribe();
        let finished = tokio::time::timeout(CLOSE_WAIT, followed.wait_for(|count| *count == 0))
            .await
            .is_ok();
        self.ends.close();
        finished
    }
}

#[cfg(unix)]
mod unix {
    use std::sync::Arc;
    use std::time::Instant;

    use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};

    use super::{CLOSE_WAIT, POLL, ShellJobs};
    use crate::{JobEnd, ProcessEnd};

    /// Whether the group `pgid` has a member left. One the host may not
    /// signal still runs.
    fn running(pgid: Pid) -> bool {
        !matches!(test_kill_process_group(pgid), Err(rustix::io::Errno::SRCH))
    }

    impl ShellJobs {
        /// Follow the group `pgid` a command that started at `started` left
        /// running, until it ends; `end` is told then. A group already empty
        /// left no job: nothing is told.
        pub(crate) fn follow(self: &Arc<Self>, pgid: Pid, end: JobEnd, started: Instant) {
            if !running(pgid) {
                return;
            }
            // A command that returned as the tool closed: its jobs go as
            // the others went, untold.
            if self.closed.is_cancelled() {
                if !self.detached.is_cancelled() {
                    let _ = kill_process_group(pgid, Signal::KILL);
                }
                return;
            }
            self.followed.send_modify(|count| *count += 1);
            tokio::spawn(Arc::clone(self).watch(pgid, end, started));
        }

        async fn watch(self: Arc<Self>, pgid: Pid, mut end: JobEnd, started: Instant) {
            let ended = loop {
                tokio::select! {
                    biased;
                    () = self.closed.cancelled() => break false,
                    () = tokio::time::sleep(POLL) => {}
                }
                if !running(pgid) {
                    break true;
                }
            };
            if ended {
                end.end = ProcessEnd::Exited;
            } else if self.detached.is_cancelled() {
                self.followed.send_modify(|count| *count -= 1);
                return;
            } else {
                end.end = stop(pgid).await;
            }
            end.elapsed_ms = crate::ends::millis(started.elapsed());
            self.ends.send(&end);
            self.followed.send_modify(|count| *count -= 1);
        }
    }

    /// Stop the group `pgid` and wait for it to empty: `stopped`, or
    /// `failed` when members the host may not signal run on.
    async fn stop(pgid: Pid) -> ProcessEnd {
        let _ = kill_process_group(pgid, Signal::KILL);
        let deadline = Instant::now() + CLOSE_WAIT;
        while running(pgid) && Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if running(pgid) {
            ProcessEnd::Failed
        } else {
            ProcessEnd::Stopped
        }
    }
}
