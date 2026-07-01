//! Unix/Mac platform layer via hand-written libc FFI (no `libc` crate).

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const SIGINT: i32 = 2;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;

type SigHandler = extern "C" fn(i32);

extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(signum: i32, handler: SigHandler) -> SigHandler;
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: i32) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Install handlers for SIGTERM/SIGINT that request a clean shutdown.
pub fn install_shutdown_handler() {
    unsafe {
        signal(SIGTERM, handle_signal);
        signal(SIGINT, handle_signal);
    }
}

/// Whether a shutdown signal has been received.
pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Put the REPL into its own process group so it (and any of its children) can
/// be torn down as a group when the server exits.
pub fn configure_command(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Holds what is needed to terminate the REPL process group.
pub struct ReplGuard {
    pgid: i32,
}

pub fn after_spawn(child: &Child) -> ReplGuard {
    // `setpgid(0,0)` in the child makes its pgid equal to its pid.
    ReplGuard {
        pgid: child.id() as i32,
    }
}

impl ReplGuard {
    /// Terminate the REPL's process group: SIGTERM, then SIGKILL after a grace
    /// period. Best effort; never panics.
    pub fn terminate(&self) {
        unsafe {
            kill(-self.pgid, SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if !group_alive(self.pgid) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            kill(-self.pgid, SIGKILL);
        }
    }
}

fn group_alive(pgid: i32) -> bool {
    unsafe { kill(-pgid, 0) == 0 }
}

/// Whether a process with `pid` exists and has not yet terminated.
///
/// `kill(pid, 0)` also succeeds for a zombie (terminated but not yet reaped by
/// its parent). From a killer's perspective such a process is gone, so on Linux
/// we additionally treat the zombie state as not alive.
pub fn is_alive(pid: u32) -> bool {
    if unsafe { kill(pid as i32, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    if is_zombie(pid) {
        return false;
    }
    true
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    // /proc/<pid>/stat: "pid (comm) state ...". comm may contain ')' and
    // spaces, so the state char is two bytes after the last ')'.
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    match stat.rfind(')') {
        Some(idx) => stat.as_bytes().get(idx + 2) == Some(&b'Z'),
        None => false,
    }
}

/// Terminate a server process by pid: SIGTERM, then SIGKILL after `timeout`.
/// Returns true if the process is gone afterwards.
pub fn terminate_pid(pid: u32, timeout: Duration) -> bool {
    let pid_i = pid as i32;
    if !is_alive(pid) {
        return true;
    }
    unsafe {
        kill(pid_i, SIGTERM);
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        kill(pid_i, SIGKILL);
    }
    thread::sleep(Duration::from_millis(50));
    !is_alive(pid)
}
