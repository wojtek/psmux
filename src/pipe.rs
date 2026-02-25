/// Named pipe IPC module for Windows.
///
/// Replaces TCP with Windows named pipes for cross-session communication.
/// Uses SDDL security descriptor with Low Integrity SACL for Session 0
/// (SSH) access — MIC evaluates BEFORE DACL, so without an explicit low
/// integrity label, processes in Session 0 cannot open pipes created in
/// the desktop session.
///
/// FFI style matches `platform.rs` — raw `#[link]` extern blocks, no new
/// crate dependencies.

use std::io;

// ── Windows constants ──────────────────────────────────────────────────
const PIPE_ACCESS_DUPLEX: u32 = 0x00000003;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x00080000;
const PIPE_TYPE_BYTE: u32 = 0x00000000;
const PIPE_READMODE_BYTE: u32 = 0x00000000;
const PIPE_WAIT: u32 = 0x00000000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const OPEN_EXISTING: u32 = 3;
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const INVALID_HANDLE_VALUE: isize = -1;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const NMPWAIT_USE_DEFAULT_WAIT: u32 = 0;
const SDDL_REVISION_1: u32 = 1;
const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;

/// SDDL: Everyone + Anonymous full access, Low integrity label.
/// This allows Session 0 (SSH) processes to open pipes created in desktop sessions.
const PIPE_SDDL: &str = "D:(A;;GA;;;WD)(A;;GA;;;AN)S:(ML;;NW;;;LW)";

// ── FFI declarations ───────────────────────────────────────────────────

#[repr(C)]
#[allow(non_snake_case)]
struct SECURITY_ATTRIBUTES {
    nLength: u32,
    lpSecurityDescriptor: *mut std::ffi::c_void,
    bInheritHandle: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeW(
        lpName: *const u16,
        dwOpenMode: u32,
        dwPipeMode: u32,
        nMaxInstances: u32,
        nOutBufferSize: u32,
        nInBufferSize: u32,
        nDefaultTimeOut: u32,
        lpSecurityAttributes: *const SECURITY_ATTRIBUTES,
    ) -> isize;

    fn ConnectNamedPipe(hNamedPipe: isize, lpOverlapped: *mut std::ffi::c_void) -> i32;
    fn DisconnectNamedPipe(hNamedPipe: isize) -> i32;

    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *const std::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: *const std::ffi::c_void,
    ) -> isize;

    fn ReadFile(
        hFile: isize,
        lpBuffer: *mut u8,
        nNumberOfBytesToRead: u32,
        lpNumberOfBytesRead: *mut u32,
        lpOverlapped: *mut std::ffi::c_void,
    ) -> i32;

    fn WriteFile(
        hFile: isize,
        lpBuffer: *const u8,
        nNumberOfBytesToWrite: u32,
        lpNumberOfBytesWritten: *mut u32,
        lpOverlapped: *mut std::ffi::c_void,
    ) -> i32;

    fn FlushFileBuffers(hFile: isize) -> i32;
    fn CloseHandle(handle: isize) -> i32;

    fn WaitNamedPipeW(lpNamedPipeName: *const u16, nTimeOut: u32) -> i32;

    fn DuplicateHandle(
        hSourceProcessHandle: isize,
        hSourceHandle: isize,
        hTargetProcessHandle: isize,
        lpTargetHandle: *mut isize,
        dwDesiredAccess: u32,
        bInheritHandle: i32,
        dwOptions: u32,
    ) -> i32;

    fn GetCurrentProcess() -> isize;
    fn GetLastError() -> u32;
}

#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        StringSecurityDescriptor: *const u16,
        StringSDRevision: u32,
        SecurityDescriptor: *mut *mut std::ffi::c_void,
        SecurityDescriptorSize: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

// ── Helper: wide string conversion ─────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── PipeStream ─────────────────────────────────────────────────────────

/// A bidirectional named pipe stream wrapping a Windows HANDLE.
/// Implements `Read` and `Write` for use with `BufReader`/`BufWriter`.
pub struct PipeStream {
    handle: isize,
}

// PipeStream is safe to send between threads — the handle is a kernel object.
unsafe impl Send for PipeStream {}
unsafe impl Sync for PipeStream {}

impl PipeStream {
    /// Wrap a raw handle into a PipeStream. Takes ownership.
    pub fn from_handle(handle: isize) -> Self {
        PipeStream { handle }
    }

    /// Clone the pipe handle via DuplicateHandle.
    /// This enables the same read/write split pattern used with TcpStream::try_clone().
    pub fn try_clone(&self) -> io::Result<Self> {
        let mut new_handle: isize = 0;
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle,
                GetCurrentProcess(),
                &mut new_handle,
                0,
                0, // don't inherit
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(PipeStream { handle: new_handle })
        }
    }

    /// No-op — named pipes don't have Nagle's algorithm.
    /// Provided for API compatibility with code that calls `stream.set_nodelay(true)`.
    pub fn set_nodelay(&self, _nodelay: bool) -> io::Result<()> {
        Ok(())
    }

    /// No-op — named pipes use blocking reads; EOF on disconnect.
    /// Provided for API compatibility with `stream.set_read_timeout(...)`.
    pub fn set_read_timeout(&self, _timeout: Option<std::time::Duration>) -> io::Result<()> {
        Ok(())
    }

    /// Shut down the pipe (close our handle). Named pipes don't have half-close,
    /// so this just closes the handle entirely.
    pub fn shutdown(&self) -> io::Result<()> {
        // This is a no-op since Drop will close the handle.
        // We don't actually close here to avoid double-close.
        Ok(())
    }
}

impl io::Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut bytes_read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            // ERROR_BROKEN_PIPE (109) = other end disconnected = EOF
            if err == 109 {
                Ok(0)
            } else {
                Err(io::Error::from_raw_os_error(err as i32))
            }
        } else {
            Ok(bytes_read as usize)
        }
    }
}

impl io::Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes_written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                self.handle,
                buf.as_ptr(),
                buf.len() as u32,
                &mut bytes_written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(bytes_written as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let ok = unsafe { FlushFileBuffers(self.handle) };
        if ok == 0 {
            // Ignore ERROR_INVALID_HANDLE on flush (pipe already closed)
            let err = unsafe { GetLastError() };
            if err == 6 { Ok(()) } else { Err(io::Error::from_raw_os_error(err as i32)) }
        } else {
            Ok(())
        }
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle); }
    }
}

// ── Pipe security descriptor ───────────────────────────────────────────

/// RAII wrapper for a security descriptor allocated by
/// ConvertStringSecurityDescriptorToSecurityDescriptorW.
struct PipeSecurityDescriptor {
    ptr: *mut std::ffi::c_void,
}

impl PipeSecurityDescriptor {
    fn new() -> io::Result<Self> {
        let sddl = to_wide(PIPE_SDDL);
        let mut sd: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(PipeSecurityDescriptor { ptr: sd })
        }
    }
}

impl Drop for PipeSecurityDescriptor {
    fn drop(&mut self) {
        // The SD was allocated by LocalAlloc inside the API — free with LocalFree.
        // We intentionally do NOT forget this; the old code used mem::forget because
        // it stored the SD in a stack-local that outlived the pipe, but here we own it.
        unsafe { LocalFree(self.ptr); }
    }
}

// ── Public API ─────────────────────────────────────────────────────────

/// Sanitize a session name for use in a pipe path.
/// Allows alphanumeric, underscore, and hyphen. Max 64 chars.
pub fn sanitize_session_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect()
}

/// Compute the pipe name for a session.
/// Format: `\\.\pipe\psmux-{name}`
pub fn pipe_name_for_session(name: &str) -> String {
    let safe = sanitize_session_name(name);
    format!("\\\\.\\pipe\\psmux-{}", safe)
}

/// Create a server-side named pipe instance with cross-session security.
///
/// The pipe uses the SDDL descriptor with a Low Integrity SACL so that
/// Session 0 (SSH) processes can connect.
///
/// `first_instance`: set true for the very first pipe instance to ensure
/// we own the name. Subsequent instances (for handling concurrent clients)
/// should pass false.
pub fn create_server_pipe(session_name: &str, first_instance: bool) -> io::Result<isize> {
    let pipe_path = pipe_name_for_session(session_name);
    let wide_name = to_wide(&pipe_path);

    let sd = PipeSecurityDescriptor::new()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.ptr,
        bInheritHandle: 0,
    };

    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first_instance {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }

    let handle = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            65536, // out buffer
            65536, // in buffer
            0,     // default timeout
            &sa,
        )
    };

    // sd is dropped here, freeing the security descriptor via LocalFree.

    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

/// Wait for a client to connect to a server pipe instance.
/// Blocks until a client connects.
pub fn wait_for_connection(handle: isize) -> io::Result<()> {
    let ok = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        // ERROR_PIPE_CONNECTED (535) = client already connected before we called
        if err == 535 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(err as i32))
        }
    } else {
        Ok(())
    }
}

/// Disconnect a client from a server pipe instance (allows reuse).
pub fn disconnect_pipe(handle: isize) {
    unsafe { DisconnectNamedPipe(handle); }
}

/// Connect to a named pipe as a client.
/// Returns the raw HANDLE on success.
pub fn connect_to_pipe(session_name: &str, timeout_ms: u32) -> io::Result<isize> {
    let pipe_path = pipe_name_for_session(session_name);
    let wide_name = to_wide(&pipe_path);

    // Try to open the pipe directly first
    let handle = unsafe {
        CreateFileW(
            wide_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0, // no sharing
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null(),
        )
    };

    if handle != INVALID_HANDLE_VALUE {
        return Ok(handle);
    }

    let err = unsafe { GetLastError() };
    if err != ERROR_PIPE_BUSY {
        return Err(io::Error::from_raw_os_error(err as i32));
    }

    // Pipe is busy — wait for an instance to become available
    let ok = unsafe { WaitNamedPipeW(wide_name.as_ptr(), timeout_ms) };
    if ok == 0 {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe wait timed out"));
    }

    // Retry open after wait
    let handle = unsafe {
        CreateFileW(
            wide_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

/// Quick check whether a named pipe exists for a session.
/// Tries to open the pipe; if it gets ERROR_PIPE_BUSY or succeeds, the pipe exists.
/// Returns false if ERROR_FILE_NOT_FOUND.
pub fn pipe_exists(session_name: &str) -> bool {
    let pipe_path = pipe_name_for_session(session_name);
    let wide_name = to_wide(&pipe_path);

    let handle = unsafe {
        CreateFileW(
            wide_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null(),
        )
    };

    if handle != INVALID_HANDLE_VALUE {
        // Successfully opened — pipe exists and was available. Close it.
        unsafe { CloseHandle(handle); }
        true
    } else {
        let err = unsafe { GetLastError() };
        // PIPE_BUSY means the pipe exists but all instances are in use
        err == ERROR_PIPE_BUSY
    }
}
