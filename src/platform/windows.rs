//! Windows platform layer via hand-written kernel32 FFI (no `windows` crate).
//!
//! - The REPL is assigned to a job object created with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so it is reaped when the server
//!   process dies, even on a forceful kill.
//! - A console control handler turns CTRL_BREAK (and friends) into a clean
//!   shutdown request.
//! - `--kill` sends `CTRL_BREAK_EVENT`, then falls back to `TerminateProcess`.

use std::ffi::c_void;
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

type Handle = *mut c_void;
type Bool = i32;
type Dword = u32;

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CTRL_BREAK_EVENT: u32 = 1;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
const PROCESS_TERMINATE: u32 = 0x0001;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_TIMEOUT: u32 = 0x0000_0102;

#[repr(C)]
#[derive(Clone, Copy)]
struct JobBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct JobExtendedLimitInformation {
    basic_limit_information: JobBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

type PhandlerRoutine = Option<extern "system" fn(Dword) -> Bool>;

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *mut c_void, len: u32) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn OpenProcess(access: Dword, inherit: Bool, pid: Dword) -> Handle;
    fn TerminateProcess(process: Handle, exit_code: u32) -> Bool;
    fn WaitForSingleObject(handle: Handle, millis: Dword) -> Dword;
    fn GenerateConsoleCtrlEvent(event: Dword, group: Dword) -> Bool;
    fn SetConsoleCtrlHandler(handler: PhandlerRoutine, add: Bool) -> Bool;
    fn CloseHandle(handle: Handle) -> Bool;
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "system" fn ctrl_handler(_ctrl: Dword) -> Bool {
    SHUTDOWN.store(true, Ordering::SeqCst);
    1 // TRUE: handled
}

pub fn install_shutdown_handler() {
    unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Put the REPL into its own process group so it can be targeted by a console
/// control event.
pub fn configure_command(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

pub struct ReplGuard {
    job: Handle,
    pid: u32,
}

// The job handle is only ever used from the thread that owns the guard.
unsafe impl Send for ReplGuard {}

pub fn after_spawn(child: &Child) -> ReplGuard {
    use std::os::windows::io::AsRawHandle;
    let process = child.as_raw_handle() as Handle;
    let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
    if !job.is_null() {
        unsafe {
            let mut info: JobExtendedLimitInformation = std::mem::zeroed();
            info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &mut info as *mut _ as *mut c_void,
                std::mem::size_of::<JobExtendedLimitInformation>() as u32,
            );
            AssignProcessToJobObject(job, process);
        }
    }
    ReplGuard {
        job,
        pid: child.id(),
    }
}

impl ReplGuard {
    pub fn terminate(&self) {
        unsafe {
            // Best-effort graceful stop, then force-kill via kill-on-close.
            GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid);
        }
        thread::sleep(Duration::from_millis(200));
        if !self.job.is_null() {
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}

pub fn is_alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let r = WaitForSingleObject(h, 0);
        CloseHandle(h);
        r == WAIT_TIMEOUT
    }
}

pub fn terminate_pid(pid: u32, timeout: Duration) -> bool {
    if !is_alive(pid) {
        return true;
    }
    unsafe {
        GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid);
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return !is_alive(pid);
        }
        TerminateProcess(h, 1);
        WaitForSingleObject(h, 2000);
        CloseHandle(h);
    }
    !is_alive(pid)
}
