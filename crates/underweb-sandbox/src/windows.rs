//! Windows: an AppContainer chosen at process creation, plus a job object.
//!
//! An AppContainer process starts with almost nothing: no network (no
//! `internetClient` capability), no access to the user's files, only what
//! "ALL APPLICATION PACKAGES" may read (Windows, Program Files, fonts).
//! Before starting it, the parent grants the container's SID read access
//! to the package and to the runner's own files, and read/write access to
//! the data directory. Grants are ACL entries on those directories and
//! stay there (as with Chromium's and UWP's containers).
//!
//! The job object kills the child with its parent, forbids it to start
//! other processes (unless the policy allows exec, for the builder) and
//! keeps it off the clipboard and other desktop-wide state.
//!
//! [`lockdown`] cannot shrink a running process into a container, so on
//! Windows it checks that the process really is in one (fail closed) and
//! turns on a few process mitigations.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GENERIC_EXECUTE,
    GENERIC_READ, GENERIC_WRITE, HANDLE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::Authorization::{
    GetSecurityInfo, SE_WINDOW_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, DeriveCapabilitySidsFromName,
    EqualSid, FreeSid, GetAce, GetSecurityDescriptorSacl, GetTokenInformation,
    LABEL_SECURITY_INFORMATION, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID, SECURITY_CAPABILITIES,
    SID_AND_ATTRIBUTES, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TokenIsAppContainer,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, GetProcessWindowStation, GetThreadDesktop,
    GetUserObjectInformationW, OpenDesktopW, OpenWindowStationW, UOI_NAME,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    OpenProcessToken, PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy,
    ProcessStrictHandleCheckPolicy, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    SetProcessMitigationPolicy, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;

use crate::Policy;
use crate::process::Launch;

/// "ALL APPLICATION PACKAGES": what every AppContainer may already read.
const ALL_APP_PACKAGES: &str = "S-1-15-2-1";

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn last_error(what: &str) -> anyhow::Error {
    anyhow::Error::new(io::Error::last_os_error()).context(what.to_string())
}

// ---- lockdown: check and harden -------------------------------------------

pub fn lockdown(_policy: &Policy) -> Result<()> {
    if !in_app_container()? {
        bail!(
            "this process was not started in an AppContainer; on Windows the sandbox \
             is applied by the parent (underweb_sandbox::Launch)"
        );
    }
    // Each policy is one DWORD of flag bits.
    let set = |policy, flags: u32, name: &str| -> Result<()> {
        let ok =
            unsafe { SetProcessMitigationPolicy(policy, &flags as *const u32 as *const c_void, 4) };
        if ok == 0 {
            return Err(last_error(name));
        }
        Ok(())
    };
    // no AppInit DLLs, IMEs or window hooks injected into us
    set(
        ProcessExtensionPointDisablePolicy,
        1,
        "extension point policy",
    )?;
    // no DLLs from network shares (low-integrity ones are fine: every
    // package library the sandboxed builder makes is low integrity)
    set(ProcessImageLoadPolicy, 0b01, "image load policy")?;
    // a bad handle is a crash, not a silent error
    set(ProcessStrictHandleCheckPolicy, 0b11, "strict handle policy")?;
    Ok(())
}

pub fn in_app_container() -> Result<bool> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut is: u32 = 0;
    let mut len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIsAppContainer,
            &mut is as *mut u32 as *mut c_void,
            4,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    Ok(is != 0)
}

// ---- SIDs -----------------------------------------------------------------

/// A SID from one of the APIs below, freed the way that API wants.
struct Sid {
    ptr: PSID,
    local: bool,
}

impl Drop for Sid {
    fn drop(&mut self) {
        unsafe {
            if self.local {
                LocalFree(self.ptr);
            } else {
                FreeSid(self.ptr);
            }
        }
    }
}

/// Held while granting a container access to files and the desktop.
static GRANTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Profiles are created one at a time: concurrent creation fails now and
/// then with E_UNEXPECTED (several packages starting at once).
static PROFILES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn container_sid(name: &str) -> Result<Sid> {
    let _one_at_a_time = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    let mut tries = 0;
    loop {
        match try_container_sid(name) {
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50 * tries));
            }
            r => return r,
        }
    }
}

fn try_container_sid(name: &str) -> Result<Sid> {
    // profile names: up to 64 chars of letters, digits, '.', '-'
    let profile = format!("underweb.{:016x}", fnv1a(name.as_bytes()));
    let wname = wide(&profile);
    let display = wide(format!("Underweb: {name}"));
    let mut sid: PSID = std::ptr::null_mut();
    let hr = unsafe {
        CreateAppContainerProfile(
            wname.as_ptr(),
            display.as_ptr(),
            display.as_ptr(),
            std::ptr::null(),
            0,
            &mut sid,
        )
    };
    // HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)
    let exists = (0x8007_0000u32 | ERROR_ALREADY_EXISTS) as i32;
    if hr == exists {
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(wname.as_ptr(), &mut sid) };
        if hr < 0 {
            bail!("DeriveAppContainerSidFromAppContainerName({profile}): 0x{hr:08x}");
        }
    } else if hr < 0 {
        bail!("CreateAppContainerProfile({profile}): 0x{hr:08x}");
    }
    Ok(Sid {
        ptr: sid,
        local: false,
    })
}

fn string_sid(s: &str) -> Result<Sid> {
    let mut sid: PSID = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide(s).as_ptr(), &mut sid) } == 0 {
        return Err(last_error("ConvertStringSidToSidW"));
    }
    Ok(Sid {
        ptr: sid,
        local: true,
    })
}

/// The SIDs of a device capability ("webcam", "microphone").
fn capability_sids(name: &str) -> Result<Vec<Sid>> {
    let (mut groups, mut ngroups, mut sids, mut nsids) =
        (std::ptr::null_mut(), 0u32, std::ptr::null_mut(), 0u32);
    let ok = unsafe {
        DeriveCapabilitySidsFromName(
            wide(name).as_ptr(),
            &mut groups,
            &mut ngroups,
            &mut sids,
            &mut nsids,
        )
    };
    if ok == 0 {
        return Err(last_error("DeriveCapabilitySidsFromName"));
    }
    let take = |arr: *mut PSID, n: u32| -> Vec<Sid> {
        let v = (0..n as usize)
            .map(|i| Sid {
                ptr: unsafe { *arr.add(i) },
                local: true,
            })
            .collect();
        unsafe { LocalFree(arr.cast()) };
        v
    };
    let _groups = take(groups, ngroups);
    Ok(take(sids, nsids))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

// ---- filesystem grants ----------------------------------------------------

/// `SE_GROUP_ENABLED` (winnt.h)
const SE_GROUP_ENABLED: u32 = 0x4;

const READ: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

fn trustee(sid: &Sid) -> TRUSTEE_W {
    TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
        ptstrName: sid.ptr.cast(),
    }
}

/// Let `sid` use `path` (and, for directories with `inherit`, everything
/// below it) with `access`. Skips the write when the DACL already allows
/// it to the container or to all app containers (Program Files, which a
/// normal user may not change, is already readable that way).
fn grant(path: &Path, sid: &Sid, access: u32, inherit: bool) -> Result<()> {
    let wpath = wide(path);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let err = unsafe {
        GetNamedSecurityInfoW(
            wpath.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "reading permissions of {}: {}",
            path.display(),
            io::Error::from_raw_os_error(err as i32)
        );
    }
    struct Free(*mut c_void);
    impl Drop for Free {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }
    let _sd = Free(sd);

    // read the entries themselves (GetEffectiveRightsFromAcl guesses
    // wrong for container SIDs and made us skip needed grants)
    if has_entry(dacl, sid.ptr, access)
        || (access == READ && has_entry(dacl, string_sid(ALL_APP_PACKAGES)?.ptr, access))
    {
        return Ok(());
    }

    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: if inherit {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: trustee(sid),
    };
    let mut new: *mut ACL = std::ptr::null_mut();
    let err = unsafe { SetEntriesInAclW(1, &ea, dacl, &mut new) };
    if err != ERROR_SUCCESS {
        bail!(
            "SetEntriesInAclW for {}: {}",
            path.display(),
            io::Error::from_raw_os_error(err as i32)
        );
    }
    let _new = Free(new.cast());
    let err = unsafe {
        SetNamedSecurityInfoW(
            wpath.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new,
            std::ptr::null(),
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "letting the sandbox use {}: {}",
            path.display(),
            io::Error::from_raw_os_error(err as i32)
        );
    }
    Ok(())
}

/// Whether `dacl` has an allow entry for exactly `sid` with all of
/// `access` (directly or inherited).
fn has_entry(dacl: *const ACL, sid: PSID, access: u32) -> bool {
    if dacl.is_null() {
        return true; // no DACL: everyone may do everything
    }
    let count = unsafe { (*dacl).AceCount };
    (0..count as u32).any(|i| {
        let mut ace: *mut c_void = std::ptr::null_mut();
        if unsafe { GetAce(dacl, i, &mut ace) } == 0 {
            return false;
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE {
            return false;
        }
        let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        let ace_sid = (&allowed.SidStart as *const u32)
            .cast_mut()
            .cast::<c_void>();
        let same = unsafe { EqualSid(ace_sid, sid) } != 0;
        same && allowed.Mask & access == access
    })
}

/// Label `path` (and what it holds) low integrity, so a container, which
/// runs at low integrity, may write there: Windows refuses low-integrity
/// writes to ordinary (medium) files whatever their DACL says.
fn label_low(path: &Path) -> Result<()> {
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // mandatory label: low, inherited by files and folders, no write-up
    let sddl = wide("S:(ML;OICI;NW;;;LW)");
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error("building the low integrity label"));
    }
    let _free = LocalMem(sd);
    let (mut present, mut defaulted) = (0, 0);
    let mut sacl: *mut ACL = std::ptr::null_mut();
    if unsafe { GetSecurityDescriptorSacl(sd, &mut present, &mut sacl, &mut defaulted) } == 0 {
        return Err(last_error("GetSecurityDescriptorSacl"));
    }
    let err = unsafe {
        SetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            sacl,
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "labelling {} for the sandbox: {}",
            path.display(),
            io::Error::from_raw_os_error(err as i32)
        );
    }
    Ok(())
}

/// Memory Windows handed us to `LocalFree`.
struct LocalMem(*mut c_void);
impl Drop for LocalMem {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// The program, the DLLs next to it, and the directory itself (to list
/// it), without granting all of `target\debug` in a development build.
fn grant_program(program: &Path, sid: &Sid) -> Result<()> {
    grant(program, sid, READ, false)?;
    // Windows' own programs (and their DLLs) are readable by every
    // container already, and a user may not change their permissions
    let system = std::env::var_os("SystemRoot").map(PathBuf::from);
    if system.is_some_and(|root| program.starts_with(root)) {
        return Ok(());
    }
    if let Some(dir) = program.parent() {
        grant(dir, sid, READ, false)?;
        for e in std::fs::read_dir(dir)?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                grant(&p, sid, READ, false)?;
            }
        }
    }
    Ok(())
}

/// Let the container use this session's window station and desktop.
/// user32.dll (which cairo and pango load) attaches to them as a process
/// starts, and fails with STATUS_DLL_INIT_FAILED if it may not. An
/// interactive desktop admits all containers already; a service session's
/// (CI) does not. Chromium grants its sandboxed processes the same.
fn grant_desktop(sid: &Sid) -> Result<()> {
    let name_of = |h: HANDLE| -> Result<Vec<u16>> {
        let mut buf = vec![0u16; 256];
        let mut needed = 0u32;
        let ok = unsafe {
            GetUserObjectInformationW(
                h,
                UOI_NAME,
                buf.as_mut_ptr().cast(),
                (buf.len() * 2) as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(last_error("GetUserObjectInformationW"));
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        buf.truncate(n);
        buf.push(0);
        Ok(buf)
    };
    let station = name_of(unsafe { GetProcessWindowStation() })?;
    let desktop = name_of(unsafe { GetThreadDesktop(GetCurrentThreadId()) })?;

    let ws = unsafe { OpenWindowStationW(station.as_ptr(), 0, READ_CONTROL | WRITE_DAC) };
    if ws.is_null() {
        return Err(last_error("opening the window station"));
    }
    let r = grant_object(ws, sid, GENERIC_READ | GENERIC_EXECUTE);
    unsafe { CloseWindowStation(ws) };
    r?;
    // a desktop name is looked up in this process's window station
    let desk = unsafe { OpenDesktopW(desktop.as_ptr(), 0, 0, READ_CONTROL | WRITE_DAC) };
    if desk.is_null() {
        return Err(last_error("opening the desktop"));
    }
    let r = grant_object(desk, sid, GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE);
    unsafe { CloseDesktop(desk) };
    r
}

/// `grant` for a window station or desktop handle.
fn grant_object(h: HANDLE, sid: &Sid, access: u32) -> Result<()> {
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let err = unsafe {
        GetSecurityInfo(
            h,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "reading the desktop's permissions: {}",
            io::Error::from_raw_os_error(err as i32)
        );
    }
    let _sd = LocalMem(sd);
    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: trustee(sid),
    };
    let mut new: *mut ACL = std::ptr::null_mut();
    let err = unsafe { SetEntriesInAclW(1, &ea, dacl, &mut new) };
    if err != ERROR_SUCCESS {
        bail!(
            "SetEntriesInAclW for the desktop: {}",
            io::Error::from_raw_os_error(err as i32)
        );
    }
    let _new = LocalMem(new.cast());
    let err = unsafe {
        SetSecurityInfo(
            h,
            SE_WINDOW_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new,
            std::ptr::null(),
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "letting the sandbox use the desktop: {}",
            io::Error::from_raw_os_error(err as i32)
        );
    }
    Ok(())
}

// ---- spawning -------------------------------------------------------------

pub struct Process {
    process: OwnedHandle,
    _job: OwnedHandle,
    pid: u32,
    stderr: Option<File>,
    status: Option<ExitStatus>,
}

/// One argument, quoted the way the C runtime's argv parser undoes it.
fn quote(arg: &OsStr, out: &mut Vec<u16>) {
    let a: Vec<u16> = arg.encode_wide().collect();
    let needs = a.is_empty()
        || a.iter()
            .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16);
    if !needs {
        out.extend(a);
        return;
    }
    out.push(b'"' as u16);
    let mut backslashes = 0;
    for &c in &a {
        if c == b'\\' as u16 {
            backslashes += 1;
        } else {
            if c == b'"' as u16 {
                out.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
    out.push(b'"' as u16);
}

fn command_line(l: &Launch) -> Vec<u16> {
    let mut line = Vec::new();
    quote(l.program.as_os_str(), &mut line);
    for a in &l.args {
        line.push(b' ' as u16);
        quote(a, &mut line);
    }
    line.push(0);
    line
}

/// `K=V\0...\0\0`, sorted as Windows expects, with what every Windows
/// process needs added if missing.
fn environment(l: &Launch) -> Vec<u16> {
    let mut vars: Vec<(OsString, OsString)> = l.env.clone();
    // Windows itself, and (for an AppContainer) the per-user folders the
    // system redirects into the container's own: CreateProcess fails
    // with ERROR_ENVVAR_NOT_FOUND without them
    let needed: &[&str] = if l.container.is_some() {
        &[
            "SystemRoot",
            "windir",
            "LOCALAPPDATA",
            "APPDATA",
            "USERPROFILE",
            "TEMP",
            "TMP",
        ]
    } else {
        &["SystemRoot", "windir"]
    };
    for k in needed.iter().copied() {
        if !vars.iter().any(|(n, _)| n.eq_ignore_ascii_case(k))
            && let Some(v) = std::env::var_os(k)
        {
            vars.push((k.into(), v));
        }
    }
    vars.sort_by_key(|(k, _)| k.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(k.encode_wide());
        block.push(b'=' as u16);
        block.extend(v.encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn inheritable_copy(h: HANDLE) -> Result<OwnedHandle> {
    let me = unsafe { GetCurrentProcess() };
    let mut out: HANDLE = std::ptr::null_mut();
    if unsafe { DuplicateHandle(me, h, me, &mut out, 0, 1, DUPLICATE_SAME_ACCESS) } == 0 {
        return Err(last_error("DuplicateHandle"));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(out) })
}

pub fn spawn(l: &Launch) -> Result<Process> {
    // children inherit the error mode: a DLL the loader can't find or
    // open must end the child with a status, not wait on an error
    // dialog nobody will see
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    // container, and its access to the files it needs
    let container = match &l.container {
        Some(c) => {
            // one at a time: a grant reads a DACL and writes it back, and
            // two containers granted the same folder at once (shared DLL
            // and toolchain folders) would lose one entry
            let _grants = GRANTS.lock().unwrap_or_else(|e| e.into_inner());
            let sid = container_sid(&c.name)?;
            grant_program(&l.program, &sid)?;
            if let Err(e) = grant_desktop(&sid) {
                // only fatal if user32 needs it; say why if it does
                eprintln!("underweb: {e:#} (a sandboxed program may fail to start)");
            }
            let policy = c.policy.canonical()?;
            for d in crate::system_read_paths(policy.allow_exec)
                .iter()
                .chain(&policy.read_only)
            {
                grant(d, &sid, READ, true)?;
            }
            for d in &policy.read_write {
                grant(d, &sid, FILE_ALL_ACCESS, true)?;
                label_low(d)?;
            }
            let mut caps = Vec::new();
            for name in &c.capabilities {
                caps.extend(capability_sids(name)?);
            }
            Some((sid, caps, policy.allow_exec))
        }
        None => None,
    };
    let allow_exec = container.as_ref().is_none_or(|c| c.2);

    // stderr: ours, or a pipe back to us
    let mut stderr_read = None;
    let child_stderr = if l.pipe_stderr {
        let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
        if unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) } == 0 {
            return Err(last_error("CreatePipe"));
        }
        stderr_read = Some(File::from(unsafe { OwnedHandle::from_raw_handle(r) }));
        let w = unsafe { OwnedHandle::from_raw_handle(w) };
        Some(inheritable_copy(w.as_raw_handle())?)
    } else {
        let h = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if h.is_null() || h as isize == -1 {
            None
        } else {
            inheritable_copy(h).ok()
        }
    };

    let mut inherit: Vec<HANDLE> = l.inherit.iter().map(|h| *h as HANDLE).collect();
    if let Some(h) = &child_stderr {
        inherit.push(h.as_raw_handle());
    }

    // attribute list
    let attrs = 3;
    let mut size = 0usize;
    unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), attrs, 0, &mut size) };
    let mut list_buf = vec![0u8; size];
    let list = list_buf.as_mut_ptr().cast::<c_void>();
    if unsafe { InitializeProcThreadAttributeList(list, attrs, 0, &mut size) } == 0 {
        return Err(last_error("InitializeProcThreadAttributeList"));
    }
    struct ListGuard(*mut c_void);
    impl Drop for ListGuard {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }
    let _guard = ListGuard(list);

    let mut cap_attrs: Vec<SID_AND_ATTRIBUTES> = container
        .iter()
        .flat_map(|(_, caps, _)| caps)
        .map(|s| SID_AND_ATTRIBUTES {
            Sid: s.ptr,
            Attributes: SE_GROUP_ENABLED,
        })
        .collect();
    let mut security = SECURITY_CAPABILITIES {
        AppContainerSid: std::ptr::null_mut(),
        Capabilities: std::ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let update = |attr: u32, value: *const c_void, len: usize, what: &str| -> Result<()> {
        let ok = unsafe {
            windows_sys::Win32::System::Threading::UpdateProcThreadAttribute(
                list,
                0,
                attr as usize,
                value,
                len,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(last_error(what));
        }
        Ok(())
    };
    if let Some((sid, _, _)) = &container {
        security.AppContainerSid = sid.ptr;
        security.Capabilities = if cap_attrs.is_empty() {
            std::ptr::null_mut()
        } else {
            cap_attrs.as_mut_ptr()
        };
        security.CapabilityCount = cap_attrs.len() as u32;
        update(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            (&security as *const SECURITY_CAPABILITIES).cast(),
            size_of::<SECURITY_CAPABILITIES>(),
            "security capabilities attribute",
        )?;
    }
    if !inherit.is_empty() {
        update(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherit.as_ptr().cast(),
            inherit.len() * size_of::<HANDLE>(),
            "handle list attribute",
        )?;
    }
    let child_policy: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    if !allow_exec {
        update(
            PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
            (&child_policy as *const u32).cast(),
            size_of::<u32>(),
            "child process attribute",
        )?;
    }

    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdError = child_stderr
        .as_ref()
        .map_or(std::ptr::null_mut(), |h| h.as_raw_handle());
    si.lpAttributeList = list;

    let app = wide(&l.program);
    let mut cmdline = command_line(l);
    let env = environment(l);
    // A container gets no console at all (as in Chromium): starting the
    // hidden console host for it fails in a service session (CI) with
    // STATUS_DLL_INIT_FAILED. Its stderr is passed explicitly anyway.
    let console = if container.is_some() {
        DETACHED_PROCESS
    } else {
        CREATE_NO_WINDOW
    };
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            (!inherit.is_empty()) as i32,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | console,
            env.as_ptr().cast(),
            std::ptr::null(),
            &si.StartupInfo,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(last_error(&format!("starting {}", l.program.display())));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(pi.hProcess) };
    let thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread) };
    drop(child_stderr);

    // into the job before it runs a single instruction
    let job = match job(allow_exec) {
        Ok(j) => j,
        Err(e) => {
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
            return Err(e);
        }
    };
    if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) } == 0 {
        let e = last_error("AssignProcessToJobObject");
        unsafe { TerminateProcess(process.as_raw_handle(), 1) };
        return Err(e);
    }
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        let e = last_error("ResumeThread");
        unsafe { TerminateProcess(process.as_raw_handle(), 1) };
        return Err(e);
    }
    Ok(Process {
        process,
        _job: job,
        pid: pi.dwProcessId,
        stderr: stderr_read,
        status: None,
    })
}

fn job(allow_exec: bool) -> Result<OwnedHandle> {
    let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if h.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    let job = unsafe { OwnedHandle::from_raw_handle(h) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    if !allow_exec {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
    }
    let ok = unsafe {
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error("job limits"));
    }
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };
    let ok = unsafe {
        SetInformationJobObject(
            h,
            JobObjectBasicUIRestrictions,
            (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error("job UI limits"));
    }
    Ok(job)
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.process.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait_ms(&mut self, ms: u32) -> io::Result<Option<ExitStatus>> {
        if let Some(s) = self.status {
            return Ok(Some(s));
        }
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), ms) } {
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.status = Some(ExitStatus::from_raw(code));
                Ok(self.status)
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_ms(INFINITE)
            .map(|s| s.expect("an infinite wait ended"))
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_ms(0)
    }

    pub fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|f| Box::new(f) as Box<dyn Read + Send>)
    }

    pub fn handle(&self) -> io::Result<OwnedHandle> {
        self.process.try_clone()
    }
}

// ---- where the libraries are ----------------------------------------------

/// Directories on `PATH` holding the GTK-stack and GStreamer DLLs the
/// runner and media service load (gvsbuild's and GStreamer's `bin`), plus
/// GStreamer's plugin directory. A release keeps these DLLs next to the
/// programs instead, and this finds nothing.
pub fn library_dirs() -> Vec<PathBuf> {
    const MARKERS: &[&str] = &[
        "cairo-2.dll",
        "pango-1.0-0.dll",
        "glib-2.0-0.dll",
        "gstreamer-1.0-0.dll",
    ];
    let mut dirs: Vec<PathBuf> = Vec::new();
    let path = std::env::var_os("PATH").unwrap_or_default();
    for d in std::env::split_paths(&path) {
        if MARKERS.iter().any(|m| d.join(m).is_file()) && !dirs.contains(&d) {
            if d.join("gstreamer-1.0-0.dll").is_file()
                && let Some(root) = d.parent()
            {
                let plugins = root.join("lib").join("gstreamer-1.0");
                if plugins.is_dir() && !dirs.contains(&plugins) {
                    dirs.push(plugins);
                }
            }
            dirs.push(d);
        }
    }
    dirs
}

/// `PATH` for a child: its own directory, the library directories, and
/// Windows itself.
pub fn child_path(program: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = program
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    dirs.extend(library_dirs());
    if let Some(root) = std::env::var_os("SystemRoot") {
        let root = PathBuf::from(root);
        dirs.push(root.join("System32"));
        dirs.push(root);
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let q = |s: &str| {
            let mut out = Vec::new();
            quote(OsStr::new(s), &mut out);
            String::from_utf16(&out).unwrap()
        };
        assert_eq!(q("plain"), "plain");
        assert_eq!(q("two words"), "\"two words\"");
        assert_eq!(q(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(q(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(q(""), "\"\"");
    }

    /// A container (cmd.exe, nothing of ours) can read a granted folder
    /// and not an ungranted one, and can't start programs.
    #[test]
    fn container_reads_only_granted_files() {
        use crate::{Container, Launch, Policy};
        let base = std::env::temp_dir().join(format!("uw-container-{}", std::process::id()));
        let (inside, outside) = (base.join("inside"), base.join("outside"));
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(inside.join("a.txt"), "INSIDE-OK").unwrap();
        std::fs::write(outside.join("b.txt"), "OUTSIDE-LEAK").unwrap();
        let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let run = |script: String| -> (String, Option<i32>) {
            let mut l = Launch::new(&cmd);
            l.arg("/C").arg(script);
            l.pipe_stderr = true;
            l.container = Some(Container {
                name: "underweb-sandbox-test".into(),
                policy: Policy {
                    read_only: vec![inside.clone()],
                    read_write: vec![],
                    allow_exec: false,
                },
                capabilities: vec![],
            });
            let mut p = l.spawn().expect("starting cmd.exe in a container");
            let mut out = String::new();
            p.take_stderr().unwrap().read_to_string(&mut out).unwrap();
            (out, p.wait().unwrap().code())
        };
        let (out, code) = run(format!("type {} 1>&2", inside.join("a.txt").display()));
        eprintln!("granted read: {code:?} {out}");
        assert!(
            out.contains("INSIDE-OK"),
            "granted file unreadable: {code:?} {out}"
        );
        let (out, code) = run(format!("type {} 1>&2", outside.join("b.txt").display()));
        eprintln!("ungranted read: {code:?} {out}");
        assert!(!out.contains("OUTSIDE-LEAK"), "read outside the grant");
        let (out, code) = run("whoami 1>&2".into());
        eprintln!("exec: {code:?} {out}");
        assert_ne!(code, Some(0), "started a program: {out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A container allowed to start programs (the builder) can run one
    /// from a granted user folder (as cargo runs rustc from ~/.rustup).
    #[test]
    fn exec_container_runs_programs_from_granted_folders() {
        use crate::{Container, Launch, Policy};
        let dir = std::env::temp_dir().join(format!("uw-exec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let system32 = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let tool = dir.join("tool.exe");
        // a copy of cmd.exe, which needs no language files beside it
        std::fs::copy(system32.join("cmd.exe"), &tool).unwrap();
        let mut l = Launch::new(system32.join("cmd.exe"));
        l.arg("/C").arg(format!("{} /C exit 0", tool.display()));
        l.pipe_stderr = true;
        l.container = Some(Container {
            name: "underweb-sandbox-exec-test".into(),
            policy: Policy {
                read_only: vec![dir.clone()],
                read_write: vec![],
                allow_exec: true,
            },
            capabilities: vec![],
        });
        let mut p = l.spawn().expect("starting cmd.exe in a container");
        let mut out = String::new();
        p.take_stderr().unwrap().read_to_string(&mut out).unwrap();
        let code = p.wait().unwrap().code();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            code,
            Some(0),
            "running {} in the container: {out}",
            tool.display()
        );
    }

    #[test]
    fn container_names_are_stable() {
        assert_eq!(fnv1a(b"demo"), fnv1a(b"demo"));
        assert_ne!(fnv1a(b"demo"), fnv1a(b"demo2"));
    }
}
