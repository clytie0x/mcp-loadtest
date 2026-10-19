//! Kill a spawned server's descendants along with it.
//!
//! Launchers such as `uvx`, `npx`, or a shell script run the real server as
//! their own child. Killing only the launcher orphans that server, so the
//! forced-kill and drop paths first kill every descendant found through
//! parent links. They must be found while the launcher is alive: once it
//! dies they are reparented and can no longer be traced to it.

use std::time::Duration;

use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

/// A process as it was when first seen: its PID plus start time, so a later
/// process that reuses the PID is not mistaken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProcessIdentity {
    pid: u32,
    start_time: u64,
}

impl ProcessIdentity {
    /// Identify the live process `pid`, or `None` if it is already gone.
    pub(super) fn of(pid: u32) -> Option<Self> {
        let mut system = System::new();
        let sys_pid = Pid::from_u32(pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sys_pid]),
            true,
            ProcessRefreshKind::new(),
        );
        system.process(sys_pid).map(|process| Self {
            pid,
            start_time: process.start_time(),
        })
    }
}

/// Descendants that [`kill_descendants`] signalled, and those it could not.
#[derive(Debug, Default)]
pub(super) struct KilledDescendants {
    pub(super) signalled: Vec<ProcessIdentity>,
    pub(super) unsignalled: Vec<u32>,
}

/// Kill every live descendant of `root`.
///
/// Does nothing unless `root` is still the process it identifies, so a scan
/// that runs late (a blocking task cannot be cancelled) never walks a process
/// that reused the PID. A process counts as a descendant only if it started
/// no earlier than its parent, so a stale parent PID (a Windows hazard)
/// cannot pull in an unrelated process either.
pub(super) fn kill_descendants(root: ProcessIdentity) -> KilledDescendants {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
    let mut killed = KilledDescendants::default();
    let root_pid = Pid::from_u32(root.pid);
    if system.process(root_pid).map(|process| process.start_time()) != Some(root.start_time) {
        return killed;
    }
    let mut found: Vec<Pid> = Vec::new();
    let mut frontier = vec![(root_pid, root.start_time)];
    while let Some((parent, parent_start)) = frontier.pop() {
        for (pid, process) in system.processes() {
            if process.parent() == Some(parent)
                && process.thread_kind().is_none()
                && process.start_time() >= parent_start
                && !found.contains(pid)
            {
                found.push(*pid);
                frontier.push((*pid, process.start_time()));
            }
        }
    }
    for pid in found {
        let Some(process) = system.process(pid) else {
            continue;
        };
        if process.kill() {
            killed.signalled.push(ProcessIdentity {
                pid: pid.as_u32(),
                start_time: process.start_time(),
            });
        } else {
            killed.unsignalled.push(pid.as_u32());
        }
    }
    killed
}

/// Wait up to `budget` for `processes` to stop running and return the PIDs
/// that still run. Gone, a zombie awaiting its reaper, or replaced by a
/// process that reused the PID all count as stopped.
pub(super) async fn survivors(processes: &[ProcessIdentity], budget: Duration) -> Vec<u32> {
    let deadline = tokio::time::Instant::now() + budget;
    let pids: Vec<Pid> = processes.iter().map(|p| Pid::from_u32(p.pid)).collect();
    loop {
        // A fresh `System` each poll: on macOS, sysinfo reads a process's
        // status only when it first sees it, so a reused one keeps reporting
        // a process killed after the first poll as running, even once it is
        // a zombie.
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::new(),
        );
        let running: Vec<u32> = processes
            .iter()
            .filter(|identity| {
                system
                    .process(Pid::from_u32(identity.pid))
                    .is_some_and(|process| {
                        process.start_time() == identity.start_time
                            && process.status() != ProcessStatus::Zombie
                    })
            })
            .map(|identity| identity.pid)
            .collect();
        if running.is_empty() || tokio::time::Instant::now() >= deadline {
            return running;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> String {
        std::env::var("MCP_LOADTEST_PYTHON").unwrap_or_else(|_| "python".to_string())
    }

    /// A launcher whose child sleeps; prints the child's PID on stdout.
    fn spawn_launcher() -> (std::process::Child, u32) {
        use std::io::BufRead;
        let mut launcher = std::process::Command::new(python())
            .args([
                "-c",
                "import subprocess, sys, time\n\
                 c = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(600)'])\n\
                 print(c.pid, flush=True)\n\
                 time.sleep(600)",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn launcher");
        let mut line = String::new();
        std::io::BufReader::new(launcher.stdout.as_mut().expect("stdout"))
            .read_line(&mut line)
            .expect("read child pid");
        let child = line.trim().parse().expect("child pid");
        (launcher, child)
    }

    #[tokio::test]
    async fn a_reused_root_pid_is_left_alone() {
        let (mut launcher, child) = spawn_launcher();
        let identity = ProcessIdentity::of(launcher.id()).expect("launcher identity");
        let impostor = ProcessIdentity {
            start_time: identity.start_time + 1,
            ..identity
        };

        let killed = kill_descendants(impostor);
        assert!(killed.signalled.is_empty(), "{killed:?}");
        let child_identity = ProcessIdentity::of(child).expect("child still running");
        assert_eq!(
            survivors(&[child_identity], Duration::ZERO).await,
            vec![child],
            "a mismatched root must not kill anything"
        );

        let killed = kill_descendants(identity);
        assert_eq!(killed.signalled, vec![child_identity]);
        assert!(killed.unsignalled.is_empty());
        assert!(
            survivors(&killed.signalled, Duration::from_secs(5))
                .await
                .is_empty(),
            "the launcher's child survived its kill"
        );
        let _ = launcher.kill();
        let _ = launcher.wait();
    }

    /// The child is first polled while it runs and killed afterwards, so its
    /// status must be re-read: it stays an unreaped zombie of the sleeping
    /// launcher, which a cached status would report as running.
    #[tokio::test]
    async fn a_descendant_killed_after_the_first_poll_is_seen_to_stop() {
        let (mut launcher, child) = spawn_launcher();
        let identity = ProcessIdentity::of(launcher.id()).expect("launcher identity");
        let child_identity = ProcessIdentity::of(child).expect("child running");
        let watch =
            tokio::spawn(async move { survivors(&[child_identity], Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let killed = kill_descendants(identity);
        let left = watch.await.expect("survivors task");
        let _ = launcher.kill();
        let _ = launcher.wait();
        assert_eq!(killed.signalled, vec![child_identity]);
        assert!(
            left.is_empty(),
            "the killed child was reported running: {left:?}"
        );
    }
}
