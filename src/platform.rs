/// Spawn a server process with a hidden console window on Windows.
///
/// Uses raw `CreateProcessW` with `STARTF_USESHOWWINDOW` + `SW_HIDE` and
/// `CREATE_NEW_CONSOLE` so that ConPTY has a real console session while the
/// window remains invisible.  This replicates the behaviour of
/// `Start-Process -WindowStyle Hidden` in PowerShell.
#[cfg(windows)]
pub fn spawn_server_hidden(exe: &std::path::Path, args: &[String]) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct STARTUPINFOW {
        cb: u32,
        lpReserved: *mut u16,
        lpDesktop: *mut u16,
        lpTitle: *mut u16,
        dwX: u32,
        dwY: u32,
        dwXSize: u32,
        dwYSize: u32,
        dwXCountChars: u32,
        dwYCountChars: u32,
        dwFillAttribute: u32,
        dwFlags: u32,
        wShowWindow: u16,
        cbReserved2: u16,
        lpReserved2: *mut u8,
        hStdInput: isize,
        hStdOutput: isize,
        hStdError: isize,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESS_INFORMATION {
        hProcess: isize,
        hThread: isize,
        dwProcessId: u32,
        dwThreadId: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateProcessW(
            lpApplicationName: *const u16,
            lpCommandLine: *mut u16,
            lpProcessAttributes: *const std::ffi::c_void,
            lpThreadAttributes: *const std::ffi::c_void,
            bInheritHandles: i32,
            dwCreationFlags: u32,
            lpEnvironment: *const std::ffi::c_void,
            lpCurrentDirectory: *const u16,
            lpStartupInfo: *const STARTUPINFOW,
            lpProcessInformation: *mut PROCESS_INFORMATION,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    const STARTF_USESHOWWINDOW: u32 = 0x00000001;
    const SW_HIDE: u16 = 0;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    // Build command line: "exe" arg1 arg2 ...
    // Each argument is quoted to handle spaces.
    let mut cmdline = format!("\"{}\"", exe.display());
    for arg in args {
        if arg.contains(' ') || arg.contains('"') {
            cmdline.push_str(&format!(" \"{}\"", arg.replace('"', "\\\"")));
        } else {
            cmdline.push(' ');
            cmdline.push_str(arg);
        }
    }
    let mut cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_HIDE;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmdline_wide.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // don't inherit handles
            CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        )
    };

    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Close handles – we don't need to wait for the child.
    unsafe {
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }

    Ok(())
}

/// Enable virtual terminal processing on Windows Console Host.
/// This is required for ANSI color codes to work in conhost.exe (legacy console).
#[cfg(windows)]
pub fn enable_virtual_terminal_processing() {
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
    }

    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if !handle.is_null() {
            let mut mode: u32 = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn enable_virtual_terminal_processing() {
    // No-op on non-Windows platforms
}

/// Install a console control handler on Windows to prevent termination on client detach.
#[cfg(windows)]
pub fn install_console_ctrl_handler() {
    type HandlerRoutine = unsafe extern "system" fn(u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: i32) -> i32;
    }

    const CTRL_CLOSE_EVENT: u32 = 2;
    const CTRL_LOGOFF_EVENT: u32 = 5;
    const CTRL_SHUTDOWN_EVENT: u32 = 6;

    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => 1,
            _ => 0,
        }
    }

    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
pub fn install_console_ctrl_handler() {
    // No-op on non-Windows platforms
}

// ---------------------------------------------------------------------------
// Windows Console API mouse injection
// ---------------------------------------------------------------------------
// ConPTY does NOT translate VT mouse escape sequences (e.g. SGR \x1b[<0;10;5M)
// into MOUSE_EVENT INPUT_RECORDs. Writing them to the PTY master appears as
// garbage text in the child app.
//
// The solution: use WriteConsoleInput to inject native MOUSE_EVENT records
// directly into the child's console input buffer.
//
// Flow:
//   1. On first mouse event targeting a pane, lazily acquire the console handle:
//      FreeConsole() → AttachConsole(child_pid) → CreateFileW("CONIN$") → FreeConsole()
//   2. The handle remains valid after FreeConsole on modern Windows (real kernel handles).
//   3. Use WriteConsoleInputW(handle, MOUSE_EVENT record) for each mouse event.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod mouse_inject {
    use std::ffi::c_void;

    const GENERIC_READ: u32  = 0x80000000;
    const GENERIC_WRITE: u32 = 0x40000000;
    const FILE_SHARE_READ: u32  = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE: isize = -1;

    const MOUSE_EVENT: u16 = 0x0002;
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;

    // dwButtonState flags
    pub const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x0001;
    pub const RIGHTMOST_BUTTON_PRESSED: u32     = 0x0002;
    pub const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x0004; // middle button

    // dwEventFlags
    pub const MOUSE_MOVED: u32       = 0x0001;
    pub const MOUSE_WHEELED: u32     = 0x0004;

    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST_DRAG_INJECT: Mutex<Option<Instant>> = Mutex::new(None);
    const DRAG_THROTTLE: Duration = Duration::from_millis(16); // ~60fps

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct COORD {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MOUSE_EVENT_RECORD {
        mouse_position: COORD,
        button_state: u32,
        control_key_state: u32,
        event_flags: u32,
    }

    #[repr(C)]
    struct INPUT_RECORD {
        event_type: u16,
        _padding: u16,
        event: MOUSE_EVENT_RECORD,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
        fn AttachConsole(process_id: u32) -> i32;
        fn GetConsoleWindow() -> isize;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *const c_void,
        ) -> isize;
        fn WriteConsoleInputW(
            console_input: isize,
            buffer: *const INPUT_RECORD,
            length: u32,
            events_written: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetProcessId(process: isize) -> u32;
        fn GetLastError() -> u32;
    }

    /// Console input mode flags
    const ENABLE_MOUSE_INPUT: u32         = 0x0010;
    const ENABLE_EXTENDED_FLAGS: u32      = 0x0080;
    const ENABLE_QUICK_EDIT_MODE: u32     = 0x0040;

    #[inline]
    fn debug_log(_msg: &str) {
        // Debug logging disabled for performance.
        // To re-enable: write to $TEMP/psmux_mouse_debug.log
    }

    /// Extract the process ID from a portable_pty::Child trait object.
    ///
    /// SAFETY: On Windows with ConPTY (portable_pty 0.2), the concrete type behind
    /// `dyn Child` is `WinChild { proc: OwnedHandle }` where OwnedHandle wraps a
    /// single Windows HANDLE. We read the HANDLE and call GetProcessId.
    pub unsafe fn get_child_pid(child: &dyn portable_pty::Child) -> Option<u32> {
        let data_ptr = child as *const dyn portable_pty::Child as *const u8;
        let handle = *(data_ptr as *const isize);
        debug_log(&format!("get_child_pid: data_ptr={:p} handle=0x{:X}", data_ptr, handle));
        if handle == 0 || handle == -1 {
            debug_log("get_child_pid: INVALID handle");
            return None;
        }
        let pid = GetProcessId(handle);
        let err = GetLastError();
        debug_log(&format!("get_child_pid: GetProcessId(0x{:X}) => pid={} err={}", handle, pid, err));
        if pid == 0 { None } else { Some(pid) }
    }

    /// Inject a mouse event into a child process's console input buffer.
    ///
    /// Performs the full cycle: FreeConsole → AttachConsole(pid) → open CONIN$
    /// → WriteConsoleInputW → CloseHandle → FreeConsole.
    ///
    /// Console handles are pseudo-handles that are invalidated by FreeConsole,
    /// so we must do the entire cycle atomically for each event.
    ///
    /// `reattach`: if true, re-attaches to original console after injection
    /// (needed for app/standalone mode where crossterm uses the console).
    /// Server mode should pass false to avoid conhost cycling.
    pub fn send_mouse_event(
        child_pid: u32,
        col: i16,
        row: i16,
        button_state: u32,
        event_flags: u32,
        reattach: bool,
    ) -> bool {
        // Throttle drag events to ~60fps to avoid excessive console attach/detach cycling
        if event_flags & MOUSE_MOVED != 0 {
            if let Ok(mut guard) = LAST_DRAG_INJECT.lock() {
                if let Some(t) = *guard {
                    if t.elapsed() < DRAG_THROTTLE {
                        return false;
                    }
                }
                *guard = Some(Instant::now());
            }
        }

        unsafe {
            // Check if we currently own a console (app mode yes, server mode no after first call)
            let had_console = reattach && GetConsoleWindow() != 0;

            // Detach from current console (no-op if already detached)
            FreeConsole();

            // Attach to child's pseudo-console
            if AttachConsole(child_pid) == 0 {
                let err = GetLastError();
                debug_log(&format!("send_mouse_event: AttachConsole({}) FAILED err={}", child_pid, err));
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Open the console input buffer
            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                let err = GetLastError();
                debug_log(&format!("send_mouse_event: CreateFileW(CONIN$) FAILED err={}", err));
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Ensure ENABLE_MOUSE_INPUT is set on the console so mouse events
            // are delivered to the foreground process (critical for WSL apps
            // like htop that rely on wsl.exe relaying MOUSE_EVENT records).
            {
                // Re-use the top-level GetConsoleMode/SetConsoleMode declarations
                // (they use *mut c_void for the handle parameter).
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
                    fn SetConsoleMode(hConsoleHandle: *mut c_void, dwMode: u32) -> i32;
                }
                let mut mode: u32 = 0;
                let h = handle as *mut c_void;
                if GetConsoleMode(h, &mut mode) != 0 {
                    let desired = mode | ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS;
                    // Also disable Quick Edit mode which intercepts mouse events
                    let desired = desired & !ENABLE_QUICK_EDIT_MODE;
                    if desired != mode {
                        SetConsoleMode(h, desired);
                    }
                }
            }

            // Write the mouse event
            let record = INPUT_RECORD {
                event_type: MOUSE_EVENT,
                _padding: 0,
                event: MOUSE_EVENT_RECORD {
                    mouse_position: COORD { x: col, y: row },
                    button_state,
                    control_key_state: 0,
                    event_flags,
                },
            };
            let mut written: u32 = 0;
            let result = WriteConsoleInputW(handle, &record, 1, &mut written);
            let write_err = GetLastError();

            debug_log(&format!("send_mouse_event: pid={} ({},{}) btn=0x{:X} flags=0x{:X} => ok={} written={} err={}",
                child_pid, col, row, button_state, event_flags, result, written, write_err));

            // Clean up: close handle, detach from child's console
            CloseHandle(handle);
            FreeConsole();
            // Only re-attach if we had our own console (app/standalone mode)
            // Server mode: leave detached to avoid conhost cycling
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0
        }
    }

    /// Inject a VT escape sequence into a child process's console input buffer
    /// as a series of KEY_EVENT records.
    ///
    /// This bypasses ConPTY's VT input parser entirely — the raw characters of
    /// the escape sequence are delivered directly to the foreground process
    /// (e.g. wsl.exe) as keyboard input.  wsl.exe forwards them to the Linux
    /// PTY, where the terminal application (e.g. htop) interprets them as
    /// mouse events.
    ///
    /// This is more reliable than writing to the PTY master pipe because
    /// ConPTY's input engine may not correctly handle SGR mouse sequences
    /// written to hInput.
    pub fn send_vt_sequence(child_pid: u32, sequence: &[u8]) -> bool {
        unsafe {
            let had_console = GetConsoleWindow() != 0;
            FreeConsole();

            if AttachConsole(child_pid) == 0 {
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            let conin: [u16; 7] = [
                'C' as u16, 'O' as u16, 'N' as u16,
                'I' as u16, 'N' as u16, '$' as u16, 0,
            ];
            let handle = CreateFileW(
                conin.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null(),
            );

            if handle == INVALID_HANDLE || handle == 0 {
                FreeConsole();
                if had_console { AttachConsole(ATTACH_PARENT_PROCESS); }
                return false;
            }

            // Ensure ENABLE_VIRTUAL_TERMINAL_INPUT is set so wsl.exe receives
            // VT sequences as-is through KEY_EVENT records.
            {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetConsoleMode(hConsoleHandle: *mut c_void, lpMode: *mut u32) -> i32;
                    fn SetConsoleMode(hConsoleHandle: *mut c_void, dwMode: u32) -> i32;
                }
                let mut mode: u32 = 0;
                let h = handle as *mut c_void;
                if GetConsoleMode(h, &mut mode) != 0 {
                    let desired = (mode | ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS | 0x0200 /*ENABLE_VIRTUAL_TERMINAL_INPUT*/)
                                  & !ENABLE_QUICK_EDIT_MODE;
                    if desired != mode {
                        SetConsoleMode(h, desired);
                    }
                }
            }

            // Build KEY_EVENT records for each byte of the VT sequence.
            // Each record is a "key down" event with the character set.
            const KEY_EVENT: u16 = 0x0001;

            #[repr(C)]
            #[derive(Copy, Clone)]
            struct KEY_EVENT_RECORD {
                key_down: i32,
                repeat_count: u16,
                virtual_key_code: u16,
                virtual_scan_code: u16,
                u_char: u16,       // UnicodeChar
                control_key_state: u32,
            }

            #[repr(C)]
            struct KEY_INPUT_RECORD {
                event_type: u16,
                _padding: u16,
                event: KEY_EVENT_RECORD,
            }

            // Build the array of input records
            let mut records: Vec<KEY_INPUT_RECORD> = Vec::with_capacity(sequence.len());
            for &byte in sequence {
                records.push(KEY_INPUT_RECORD {
                    event_type: KEY_EVENT,
                    _padding: 0,
                    event: KEY_EVENT_RECORD {
                        key_down: 1,
                        repeat_count: 1,
                        virtual_key_code: 0,
                        virtual_scan_code: 0,
                        u_char: byte as u16,
                        control_key_state: 0,
                    },
                });
            }

            let mut written: u32 = 0;
            let result = WriteConsoleInputW(
                handle,
                records.as_ptr() as *const INPUT_RECORD,
                records.len() as u32,
                &mut written,
            );

            CloseHandle(handle);
            FreeConsole();
            if had_console {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }

            result != 0
        }
    }
}

#[cfg(not(windows))]
pub mod mouse_inject {
    pub unsafe fn get_child_pid(_child: &dyn portable_pty::Child) -> Option<u32> { None }
    pub fn send_mouse_event(_pid: u32, _col: i16, _row: i16, _btn: u32, _flags: u32, _reattach: bool) -> bool { false }
    pub fn send_vt_sequence(_pid: u32, _sequence: &[u8]) -> bool { false }
}

// ---------------------------------------------------------------------------
// Process tree killing — ensures all descendant processes are terminated
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod process_kill {
    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const INVALID_HANDLE: isize = -1;

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn TerminateProcess(h_process: isize, exit_code: u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    /// Collect all descendant PIDs of `root_pid` (children, grandchildren, etc.).
    /// Uses a breadth-first traversal of the process tree snapshot.
    fn collect_descendants(root_pid: u32) -> Vec<u32> {
        let mut descendants = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 { return descendants; }

            // Build full process table from snapshot
            let mut entries: Vec<(u32, u32)> = Vec::with_capacity(256); // (pid, parent_pid)
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snap, &mut pe) != 0 {
                entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                while Process32NextW(snap, &mut pe) != 0 {
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id));
                }
            }
            CloseHandle(snap);

            // BFS from root_pid
            let mut queue: Vec<u32> = vec![root_pid];
            let mut head = 0;
            while head < queue.len() {
                let parent = queue[head];
                head += 1;
                for &(pid, ppid) in &entries {
                    if ppid == parent && pid != root_pid && !queue.contains(&pid) {
                        queue.push(pid);
                        descendants.push(pid);
                    }
                }
            }
        }
        descendants
    }

    /// Force-terminate a single process by PID.
    fn terminate_pid(pid: u32) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION, 0, pid);
            if h != 0 && h != INVALID_HANDLE {
                let _ = TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }

    /// Kill an entire process tree: all descendants first (leaves → root order),
    /// then the root process itself.  Calls `child.kill()` via portable_pty as a
    /// fallback.  Does NOT call `child.wait()` so `try_wait()` still works for
    /// the reaper (`prune_exited`), which will detect the dead process and clean
    /// up the tree node.
    ///
    /// This mirrors how tmux on Linux sends SIGKILL to the pane's process group.
    pub fn kill_process_tree(child: &mut Box<dyn portable_pty::Child>) {
        // Try to get the PID
        let pid = unsafe { super::mouse_inject::get_child_pid(child.as_ref()) };

        if let Some(root_pid) = pid {
            // Collect all descendants, kill them leaf-first (reverse order)
            let mut descs = collect_descendants(root_pid);
            descs.reverse();
            for &dpid in &descs {
                terminate_pid(dpid);
            }
            // Kill the root process
            terminate_pid(root_pid);
        }

        // Fallback: tell portable_pty to kill the direct child process.
        // Do NOT call child.wait() here — the reaper (prune_exited) needs
        // try_wait() to detect the dead process and remove the tree node.
        let _ = child.kill();
    }
}

#[cfg(not(windows))]
pub mod process_kill {
    /// On non-Windows, fall back to simple kill (no wait — let the reaper handle it).
    pub fn kill_process_tree(child: &mut Box<dyn portable_pty::Child>) {
        let _ = child.kill();
    }
}

// ---------------------------------------------------------------------------
// Process info queries — get CWD and process name from PID (for format vars)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub mod process_info {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const MAX_PATH: usize = 260;
    const TH32CS_SNAPPROCESS: u32 = 0x00000002;
    const INVALID_HANDLE: isize = -1;

    #[allow(non_snake_case)]
    #[repr(C)]
    struct PROCESS_BASIC_INFORMATION {
        Reserved1: isize,
        PebBaseAddress: isize, // pointer to PEB
        Reserved2: [isize; 2],
        UniqueProcessId: isize,
        Reserved3: isize,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    struct UNICODE_STRING {
        Length: u16,
        MaximumLength: u16,
        Buffer: isize, // pointer to wide string
    }

    #[repr(C)]
    struct PROCESSENTRY32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn QueryFullProcessImageNameW(h: isize, flags: u32, name: *mut u16, size: *mut u32) -> i32;
        fn ReadProcessMemory(
            h_process: isize,
            base_address: isize,
            buffer: *mut u8,
            size: usize,
            bytes_read: *mut usize,
        ) -> i32;
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        fn Process32FirstW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
        fn Process32NextW(h_snapshot: isize, lppe: *mut PROCESSENTRY32W) -> i32;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: isize,
            process_information_class: u32,
            process_information: *mut u8,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    /// Get the executable name of a process by PID (e.g. "pwsh" or "vim").
    pub fn get_process_name(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 || h == -1 { return None; }
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
            CloseHandle(h);
            if ok == 0 { return None; }
            let full_path = OsString::from_wide(&buf[..size as usize])
                .to_string_lossy()
                .into_owned();
            let name = std::path::Path::new(&full_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())?;
            Some(name)
        }
    }

    /// Get the current working directory of a process by PID.
    /// Reads the PEB → ProcessParameters → CurrentDirectory from the target process.
    pub fn get_process_cwd(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if h == 0 || h == -1 { return None; }
            let result = read_process_cwd(h);
            CloseHandle(h);
            result
        }
    }

    /// Read CWD from a process handle via NtQueryInformationProcess + ReadProcessMemory.
    unsafe fn read_process_cwd(h: isize) -> Option<String> {
        // Step 1: Get PEB address
        let mut pbi: PROCESS_BASIC_INFORMATION = std::mem::zeroed();
        let mut ret_len: u32 = 0;
        let status = NtQueryInformationProcess(
            h,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut u8,
            std::mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            &mut ret_len,
        );
        if status != 0 { return None; }
        let peb_addr = pbi.PebBaseAddress;
        if peb_addr == 0 { return None; }

        // Step 2: Read ProcessParameters pointer from PEB.
        // PEB layout (x64): offset 0x20 = ProcessParameters pointer
        // PEB layout (x86): offset 0x10 = ProcessParameters pointer
        let params_ptr_offset = if std::mem::size_of::<usize>() == 8 { 0x20 } else { 0x10 };
        let mut process_params_ptr: isize = 0;
        let mut bytes_read: usize = 0;
        let ok = ReadProcessMemory(
            h,
            peb_addr + params_ptr_offset,
            &mut process_params_ptr as *mut isize as *mut u8,
            std::mem::size_of::<isize>(),
            &mut bytes_read,
        );
        if ok == 0 || process_params_ptr == 0 { return None; }

        // Step 3: Read CurrentDirectory.DosPath (UNICODE_STRING) from RTL_USER_PROCESS_PARAMETERS.
        // x64 offset: 0x38 = CurrentDirectory.DosPath
        // x86 offset: 0x24 = CurrentDirectory.DosPath
        let cwd_offset = if std::mem::size_of::<usize>() == 8 { 0x38 } else { 0x24 };
        let mut cwd_ustr: UNICODE_STRING = std::mem::zeroed();
        let ok = ReadProcessMemory(
            h,
            process_params_ptr + cwd_offset,
            &mut cwd_ustr as *mut UNICODE_STRING as *mut u8,
            std::mem::size_of::<UNICODE_STRING>(),
            &mut bytes_read,
        );
        if ok == 0 || cwd_ustr.Length == 0 || cwd_ustr.Buffer == 0 { return None; }

        // Step 4: Read the actual CWD wide string
        let char_count = (cwd_ustr.Length / 2) as usize;
        let mut wchars: Vec<u16> = vec![0u16; char_count];
        let ok = ReadProcessMemory(
            h,
            cwd_ustr.Buffer,
            wchars.as_mut_ptr() as *mut u8,
            cwd_ustr.Length as usize,
            &mut bytes_read,
        );
        if ok == 0 { return None; }

        let path = OsString::from_wide(&wchars)
            .to_string_lossy()
            .into_owned();
        // Remove trailing backslash (tmux convention)
        Some(path.trim_end_matches('\\').to_string())
    }

    /// Append a line to ~/.psmux/autorename.log (first 100 entries only).
    fn autorename_log(msg: &str) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        if n > 100 { return; }
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
        let path = format!("{}/.psmux/autorename.log", home);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
        }
    }

    /// Get the name of the foreground process in the pane.
    /// Walks the process tree from the shell PID to find the deepest
    /// non-system descendant (the user's foreground command).
    pub fn get_foreground_process_name(pid: u32) -> Option<String> {
        // Walk the process tree to find the foreground child.
        let result = find_foreground_child_pid(pid);
        match result {
            Some(target) if target != pid => {
                let name = get_process_name(target);
                autorename_log(&format!("pid={} fg_child={} name={:?}", pid, target, name));
                if let Some(n) = name {
                    return Some(n);
                }
            }
            Some(_) => {
                autorename_log(&format!("pid={} fg_child=self (no children)", pid));
            }
            None => {
                autorename_log(&format!("pid={} fg_child=None (BFS found nothing)", pid));
            }
        }
        // Fallback: shell's own process name.
        let shell_name = get_process_name(pid);
        autorename_log(&format!("pid={} fallback_name={:?}", pid, shell_name));
        shell_name
    }

    /// Get the CWD of the foreground process in the pane.
    pub fn get_foreground_cwd(pid: u32) -> Option<String> {
        if let Some(target) = find_foreground_child_pid(pid) {
            if target != pid {
                if let Some(cwd) = get_process_cwd(target) {
                    return Some(cwd);
                }
            }
        }
        get_process_cwd(pid)
    }

    /// Known system/infrastructure processes that should be skipped when
    /// walking the process tree to find the user's foreground command.
    fn is_system_exe(name: &str) -> bool {
        matches!(name,
            "conhost.exe" | "csrss.exe" | "dwm.exe" | "services.exe"
            | "svchost.exe" | "wininit.exe" | "winlogon.exe"
            | "openconsole.exe" | "runtimebroker.exe"
        )
    }

    /// Walk the process tree from `root_pid` downward and return the PID of
    /// the process most likely to be the user's foreground command.
    ///
    /// Strategy: BFS all descendants, then pick the deepest non-system leaf.
    /// When multiple candidates exist at the same depth, prefer the largest
    /// PID (heuristic for "most recently created").
    fn find_foreground_child_pid(root_pid: u32) -> Option<u32> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 {
                autorename_log(&format!("root={} SNAPSHOT FAILED", root_pid));
                return None;
            }

            // Collect (pid, ppid, exe_name_lower) for every process.
            let mut entries: Vec<(u32, u32, String)> = Vec::with_capacity(512);
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snap, &mut pe) != 0 {
                let name = exe_name_from_entry(&pe);
                entries.push((pe.th32_process_id, pe.th32_parent_process_id, name));
                while Process32NextW(snap, &mut pe) != 0 {
                    let name = exe_name_from_entry(&pe);
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id, name));
                }
            }
            CloseHandle(snap);

            autorename_log(&format!("root={} snapshot_entries={}", root_pid, entries.len()));

            // Log direct children of root_pid
            let direct: Vec<_> = entries.iter()
                .filter(|(_, ppid, _)| *ppid == root_pid)
                .collect();
            for (pid, _, name) in &direct {
                autorename_log(&format!("  direct_child: pid={} name={}", pid, name));
            }

            // BFS: collect all descendants with their depth.
            // Each entry is (pid, exe_name, depth).
            let mut descendants: Vec<(u32, String, u32)> = Vec::new();
            let mut queue: Vec<(u32, u32)> = vec![(root_pid, 0)]; // (pid, depth)
            let mut head = 0;
            while head < queue.len() {
                let (parent, depth) = queue[head];
                head += 1;
                for (pid, ppid, name) in &entries {
                    if *ppid == parent && *pid != root_pid
                        && !descendants.iter().any(|(p, _, _)| p == pid)
                    {
                        descendants.push((*pid, name.clone(), depth + 1));
                        queue.push((*pid, depth + 1));
                    }
                }
            }

            autorename_log(&format!("root={} descendants={}", root_pid, descendants.len()));
            for (pid, name, depth) in &descendants {
                autorename_log(&format!("  desc: pid={} name={} depth={}", pid, name, depth));
            }

            if descendants.is_empty() {
                return None;
            }

            // A "leaf" is a descendant that has no children in our descendant set.
            let desc_pids: std::collections::HashSet<u32> =
                descendants.iter().map(|(p, _, _)| *p).collect();
            let leaves: Vec<(u32, &str, u32)> = descendants.iter()
                .filter(|(pid, _, _)| {
                    // No entry in the process table has this pid as parent
                    // while also being in our descendant set.
                    !entries.iter().any(|(ep, eppid, _)| *eppid == *pid && desc_pids.contains(ep))
                })
                .map(|(pid, name, depth)| (*pid, name.as_str(), *depth))
                .collect();

            // Choose from leaves if available, otherwise from all descendants.
            let pool: Vec<(u32, &str, u32)> = if !leaves.is_empty() {
                leaves
            } else {
                descendants.iter().map(|(p, n, d)| (*p, n.as_str(), *d)).collect()
            };

            // Prefer non-system candidates.
            let user_pool: Vec<&(u32, &str, u32)> = pool.iter()
                .filter(|(_, name, _)| !is_system_exe(name))
                .collect();

            let selection = if !user_pool.is_empty() { user_pool } else { pool.iter().collect() };

            // Deepest first, then largest PID as tiebreaker.
            let result = selection.iter()
                .max_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)))
                .map(|(pid, _, _)| *pid);

            autorename_log(&format!("root={} selected={:?}", root_pid, result));
            result
        }
    }

    /// Extract the lowercased executable name from a PROCESSENTRY32W.
    fn exe_name_from_entry(pe: &PROCESSENTRY32W) -> String {
        let nul = pe.sz_exe_file.iter().position(|&c| c == 0).unwrap_or(pe.sz_exe_file.len());
        String::from_utf16_lossy(&pe.sz_exe_file[..nul]).to_lowercase()
    }

    /// Check if an executable name is a VT bridge process (WSL, SSH, etc.)
    /// that requires VT mouse injection instead of Win32 console injection.
    fn is_vt_bridge_exe(name: &str) -> bool {
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        matches!(stem, "wsl" | "ssh" | "ubuntu" | "debian" | "kali"
                      | "fedoraremix" | "opensuse-leap" | "sles" | "arch")
            || stem.starts_with("wsl")
    }

    /// Walk the process tree from `root_pid` and check if any descendant
    /// is a VT bridge process (wsl.exe, ssh.exe, etc.).
    /// This is used for mouse injection: VT bridge processes need VT mouse
    /// sequences written to the PTY master, not Win32 MOUSE_EVENT records.
    pub fn has_vt_bridge_descendant(root_pid: u32) -> bool {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE || snap == 0 { return false; }

            let mut entries: Vec<(u32, u32, String)> = Vec::with_capacity(256);
            let mut pe: PROCESSENTRY32W = std::mem::zeroed();
            pe.dw_size = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snap, &mut pe) != 0 {
                let name = exe_name_from_entry(&pe);
                entries.push((pe.th32_process_id, pe.th32_parent_process_id, name));
                while Process32NextW(snap, &mut pe) != 0 {
                    let name = exe_name_from_entry(&pe);
                    entries.push((pe.th32_process_id, pe.th32_parent_process_id, name));
                }
            }
            CloseHandle(snap);

            // BFS from root_pid to check all descendants
            let mut queue: Vec<u32> = vec![root_pid];
            let mut head = 0;
            while head < queue.len() {
                let parent = queue[head];
                head += 1;
                for (pid, ppid, name) in &entries {
                    if *ppid == parent && *pid != root_pid
                        && !queue.contains(pid)
                    {
                        if is_vt_bridge_exe(name) {
                            return true;
                        }
                        queue.push(*pid);
                    }
                }
            }
            false
        }
    }
}

#[cfg(not(windows))]
pub mod process_info {
    pub fn get_process_name(_pid: u32) -> Option<String> { None }
    pub fn get_process_cwd(_pid: u32) -> Option<String> { None }
    pub fn get_foreground_process_name(_pid: u32) -> Option<String> { None }
    pub fn get_foreground_cwd(_pid: u32) -> Option<String> { None }
    pub fn has_vt_bridge_descendant(_root_pid: u32) -> bool { false }
}
