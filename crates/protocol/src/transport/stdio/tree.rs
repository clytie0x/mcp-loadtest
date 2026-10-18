//! Kill a spawned server's descendants along with it.
//!
//! Launchers such as `uvx`, `npx`, or a shell script run the real server as
//! their own child. Killing only the launcher orphans that server, so the
//! forced-kill and drop paths first kill every descendant found through
//! parent links. They must be found while the launcher is alive: once it
//! dies they are reparented and can no longer be traced to it.

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Kill every live descendant of `root` and return how many were signalled.
///
/// A process counts as a descendant only if it started no earlier than its
/// claimed ancestor, so a stale parent PID that `root` happens to reuse (a
/// Windows hazard) cannot pull an unrelated process into the kill.
pub(super) fn kill_descendants(root: u32) -> usize {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
    let Some(root_process) = system.process(Pid::from_u32(root)) else {
        return 0;
    };
    let mut found: Vec<Pid> = Vec::new();
    let mut frontier = vec![(Pid::from_u32(root), root_process.start_time())];
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
    found
        .iter()
        .filter(|pid| system.process(**pid).is_some_and(|process| process.kill()))
        .count()
}
