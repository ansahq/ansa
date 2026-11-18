//! Non-Windows metric stub. The harness still reports explicit unavailability.

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
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

pub fn total_physical_memory_bytes() -> Option<u64> {
    None
}

pub fn logical_processor_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(0)
}

pub fn physical_core_count() -> Option<u32> {
    None
}

pub fn process_cpu_100ns() -> Option<u64> {
    None
}

pub fn process_tree_memory() -> Option<TreeMemory> {
    None
}
