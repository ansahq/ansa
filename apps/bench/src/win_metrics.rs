//! Windows-only process and machine metrics.
//!
//! Every `unsafe` block in `ansa-bench` lives in this module. Calls use the
//! documented Win32 contracts; structures are `repr(C)`, zero-initialized with
//! `MaybeUninit`, and every opened handle is closed on all paths.
//!
//! Working-set, private-bytes and PSS numbers are observations, not budgets.

use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const PROCESS_VM_READ: u32 = 0x0010;
const MEM_COMMIT: u32 = 0x1000;
const RELATION_PROCESSOR_CORE: u32 = 0;
const PAGE_SIZE: u64 = 4096;
const MAX_PAGES_PER_QUERY: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct FileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
struct MemoryCountersEx {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[repr(C)]
struct SystemLogicalProcessorInformation {
    processor_mask: usize,
    relationship: u32,
    _padding: u32,
    _data: [u64; 2],
}

#[repr(C)]
struct ProcessEntry32W {
    dw_size: u32,
    cnt_usage: u32,
    process_id: u32,
    default_heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_process_id: u32,
    pri_class_base: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    _padding: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    region_type: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WorkingSetExInformation {
    virtual_address: *mut c_void,
    virtual_attributes: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessTimes(
        process: Handle,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    fn GetLogicalProcessorInformation(
        buffer: *mut SystemLogicalProcessorInformation,
        returned_length: *mut u32,
    ) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn VirtualQueryEx(
        process: Handle,
        address: *const c_void,
        buffer: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
}

#[link(name = "psapi")]
unsafe extern "system" {
    fn GetProcessMemoryInfo(process: Handle, counters: *mut MemoryCountersEx, cb: u32) -> i32;
    fn QueryWorkingSetEx(process: Handle, buffer: *mut c_void, length: u32) -> i32;
}

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
    /// Windows working set, the closest available equivalent to RSS.
    pub working_set: u64,
    pub private_bytes: u64,
    pub pss: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TreeMemory {
    pub processes: Vec<ProcessMemory>,
    pub pss: Option<u64>,
    pub pss_complete: bool,
}

/// Total physical memory in bytes, if the OS reports it.
pub fn total_physical_memory_bytes() -> Option<u64> {
    let mut status = MaybeUninit::<MemoryStatusEx>::zeroed();
    unsafe {
        let status_ptr = status.as_mut_ptr();
        (*status_ptr).length = size_of::<MemoryStatusEx>() as u32;
        if GlobalMemoryStatusEx(status_ptr) == 0 {
            None
        } else {
            Some((*status_ptr).total_phys)
        }
    }
}

/// Number of logical processors reported to this process.
pub fn logical_processor_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(0)
}

/// Number of physical cores, counted from `RelationProcessorCore` entries.
pub fn physical_core_count() -> Option<u32> {
    let mut length = 0u32;
    unsafe {
        let _ = GetLogicalProcessorInformation(std::ptr::null_mut(), &mut length);
    }
    if length == 0 {
        return None;
    }
    let entry_size = size_of::<SystemLogicalProcessorInformation>();
    let entries = (length as usize).div_ceil(entry_size);
    let mut buffer: Vec<SystemLogicalProcessorInformation> = Vec::with_capacity(entries);
    // The struct is plain integer data, so zeroed initialization is valid.
    buffer.resize_with(entries, || unsafe { MaybeUninit::zeroed().assume_init() });
    let mut returned = length;
    let ok = unsafe { GetLogicalProcessorInformation(buffer.as_mut_ptr(), &mut returned) != 0 };
    if !ok {
        return None;
    }
    let count = (returned as usize) / entry_size;
    Some(
        buffer
            .iter()
            .take(count)
            .filter(|entry| entry.relationship == RELATION_PROCESSOR_CORE)
            .count() as u32,
    )
}

/// Kernel + user CPU time for this process, in 100-nanosecond units.
pub fn process_cpu_100ns() -> Option<u64> {
    unsafe {
        let mut creation = MaybeUninit::<FileTime>::zeroed();
        let mut exit = MaybeUninit::<FileTime>::zeroed();
        let mut kernel = MaybeUninit::<FileTime>::zeroed();
        let mut user = MaybeUninit::<FileTime>::zeroed();
        if GetProcessTimes(
            GetCurrentProcess(),
            creation.as_mut_ptr(),
            exit.as_mut_ptr(),
            kernel.as_mut_ptr(),
            user.as_mut_ptr(),
        ) == 0
        {
            return None;
        }
        let kernel = ((*kernel.as_ptr()).high as u64) << 32 | (*kernel.as_ptr()).low as u64;
        let user = ((*user.as_ptr()).high as u64) << 32 | (*user.as_ptr()).low as u64;
        Some(kernel.saturating_add(user))
    }
}

/// Memory for this process and its direct children.
pub fn process_tree_memory() -> Option<TreeMemory> {
    let self_memory = memory_for(unsafe { GetCurrentProcess() }, "ansa-bench", unsafe {
        GetCurrentProcessId()
    })?;
    let mut processes = vec![self_memory];
    let mut pss_total = processes[0].pss;
    let mut pss_complete = processes[0].pss.is_some();

    for child in direct_children() {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, child) };
        if handle.is_null() {
            pss_complete = false;
            continue;
        }
        if let Some(memory) = memory_for(handle, &format!("child-{child}"), child) {
            pss_total = match (pss_total, memory.pss) {
                (Some(total), Some(pss)) => Some(total.saturating_add(pss)),
                _ => {
                    pss_complete = false;
                    pss_total
                }
            };
            processes.push(memory);
        } else {
            pss_complete = false;
        }
        unsafe {
            let _ = CloseHandle(handle);
        }
    }

    Some(TreeMemory {
        processes,
        pss: if pss_complete { pss_total } else { None },
        pss_complete,
    })
}

fn memory_for(handle: Handle, name: &str, pid: u32) -> Option<ProcessMemory> {
    let mut counters = MaybeUninit::<MemoryCountersEx>::zeroed();
    unsafe {
        let counters_ptr = counters.as_mut_ptr();
        (*counters_ptr).cb = size_of::<MemoryCountersEx>() as u32;
        if GetProcessMemoryInfo(handle, counters_ptr, (*counters_ptr).cb) == 0 {
            return None;
        }
        Some(ProcessMemory {
            pid,
            name: name.to_string(),
            working_set: (*counters_ptr).working_set_size as u64,
            private_bytes: (*counters_ptr).private_usage as u64,
            pss: proportional_set_size(handle),
        })
    }
}

fn direct_children() -> Vec<u32> {
    let parent = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return Vec::new();
    }
    let mut entry = MaybeUninit::<ProcessEntry32W>::zeroed();
    let mut children = Vec::new();
    unsafe {
        let entry_ptr = entry.as_mut_ptr();
        (*entry_ptr).dw_size = size_of::<ProcessEntry32W>() as u32;
        if Process32FirstW(snapshot, entry_ptr) != 0 {
            loop {
                if (*entry_ptr).parent_process_id == parent {
                    children.push((*entry_ptr).process_id);
                }
                if Process32NextW(snapshot, entry_ptr) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    children
}

/// Sums proportional set size from `QueryWorkingSetEx` page attributes.
///
/// Only valid (resident) pages contribute. Shared pages are divided by their
/// reported share count. Large-page granularity is not accounted for, so this
/// is an estimate rather than an exact accounting number.
fn proportional_set_size(handle: Handle) -> Option<u64> {
    let mut address = 0usize;
    let mut total = 0u64;
    let mut queried_any = false;
    let maximum = usize::MAX;

    while address < maximum {
        let mut info = MaybeUninit::<MemoryBasicInformation>::zeroed();
        let written = unsafe {
            VirtualQueryEx(
                handle,
                address as *const c_void,
                info.as_mut_ptr(),
                size_of::<MemoryBasicInformation>(),
            )
        };
        if written == 0 {
            break;
        }
        let info = unsafe { info.assume_init() };
        if info.region_size == 0 {
            break;
        }
        let base = info.base_address as usize;
        let end = base.saturating_add(info.region_size);
        if info.state == MEM_COMMIT {
            let pages = (info.region_size as u64).div_ceil(PAGE_SIZE) as usize;
            let mut page = 0usize;
            while page < pages {
                let batch = (pages - page).min(MAX_PAGES_PER_QUERY);
                let mut queries: Vec<WorkingSetExInformation> = (0..batch)
                    .map(|offset| WorkingSetExInformation {
                        virtual_address: (base + (page + offset) * PAGE_SIZE as usize)
                            as *mut c_void,
                        virtual_attributes: 0,
                    })
                    .collect();
                let bytes = batch * size_of::<WorkingSetExInformation>();
                let ok = unsafe {
                    QueryWorkingSetEx(handle, queries.as_mut_ptr() as *mut c_void, bytes as u32)
                } != 0;
                if ok {
                    queried_any = true;
                    for query in &queries {
                        let flags = query.virtual_attributes as u64;
                        if flags & 1 == 0 {
                            continue;
                        }
                        let shared = flags & (1 << 15) != 0;
                        if shared {
                            let share_count = (flags >> 1) & 0x7;
                            total = total.saturating_add(PAGE_SIZE / (share_count + 1));
                        } else {
                            total = total.saturating_add(PAGE_SIZE);
                        }
                    }
                }
                page += batch;
            }
        }
        if end <= address {
            break;
        }
        address = end;
    }

    queried_any.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_available_or_explicitly_absent_without_panicking() {
        // These may be unavailable in a sandbox; the contract is "never panic".
        let _ = total_physical_memory_bytes();
        let _ = physical_core_count();
        let _ = process_cpu_100ns();
        let tree = process_tree_memory();
        if let Some(tree) = tree {
            assert!(!tree.processes.is_empty());
            assert!(tree.processes[0].working_set > 0);
            assert!(tree.processes[0].private_bytes > 0);
        }
        assert!(logical_processor_count() > 0);
    }
}
