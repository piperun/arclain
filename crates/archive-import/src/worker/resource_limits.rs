use crate::ImportError;

#[cfg(windows)]
pub(super) struct ResourceGuard(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Drop for ResourceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
#[cfg(windows)]
pub(super) fn establish(bytes: u64) -> Result<ResourceGuard, ImportError> {
    use windows_sys::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(3);
    }
    let limit = usize::try_from(bytes).map_err(|_| ImportError::ResourceLimit)?;
    // SAFETY: initialized structures and owned valid handles; no untrusted
    // pointers are passed. The job lives for the entire worker operation.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(ImportError::ResourceLimit);
        }
        let guard = ResourceGuard(job);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = limit;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of_val(&info) as u32,
        ) == 0
            || AssignProcessToJobObject(job, GetCurrentProcess()) == 0
        {
            return Err(ImportError::ResourceLimit);
        }
        Ok(guard)
    }
}
#[cfg(target_os = "linux")]
pub(super) struct ResourceGuard;
#[cfg(target_os = "linux")]
pub(super) fn establish(bytes: u64) -> Result<ResourceGuard, ImportError> {
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    let core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: pointers reference initialized values with the libc layout.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_CORE, &core) } != 0
    {
        return Err(ImportError::ResourceLimit);
    }
    Ok(ResourceGuard)
}
#[cfg(not(any(windows, target_os = "linux")))]
pub(super) struct ResourceGuard;
#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn establish(_: u64) -> Result<ResourceGuard, ImportError> {
    Err(ImportError::ResourceLimit)
}
