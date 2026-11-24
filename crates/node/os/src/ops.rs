use std::net::IpAddr;

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;

pub(crate) const OPS: &[OpDecl] = &[
    op_os_system_info(),
    op_os_live_info(),
    op_os_cpus(),
    op_os_network_interfaces(),
    op_os_homedir(),
    op_os_tmpdir(),
];

#[cfg(target_os = "linux")]
fn proc_text(path: &str, fallback: &str) -> String {
    std::fs::read_to_string(path)
        .map_or_else(|_| fallback.to_owned(), |text| text.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn memory() -> (u64, u64) {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut available = 0u64;
    for line in meminfo.lines() {
        let mut fields = line.split_whitespace();
        let key = fields.next();
        let value = fields.next().and_then(|value| value.parse::<u64>().ok());
        match (key, value) {
            (Some("MemTotal:"), Some(kib)) => total = kib * 1024,
            (Some("MemAvailable:"), Some(kib)) => available = kib * 1024,
            _ => {}
        }
    }
    (total, available)
}

#[cfg(target_os = "linux")]
fn system_info() -> String {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu_count = cpuinfo
        .lines()
        .filter(|line| line.trim_start().starts_with("processor"))
        .count()
        .max(1);
    let (total, _) = memory();

    format!(
        "{cpu_count}\0{total}\0{}\0{}\0{}",
        proc_text("/proc/sys/kernel/osrelease", "unknown"),
        proc_text("/proc/version", "unknown"),
        proc_text("/proc/sys/kernel/hostname", "localhost"),
    )
}

#[cfg(target_os = "linux")]
fn live_info() -> String {
    let (_, available) = memory();
    let uptime = proc_text("/proc/uptime", "0")
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let loadavg = proc_text("/proc/loadavg", "0 0 0");
    let mut loads = ["0", "0", "0"];
    for (slot, value) in loads.iter_mut().zip(loadavg.split_whitespace()) {
        *slot = value;
    }

    format!(
        "{available}\0{uptime}\0{}\0{}\0{}",
        loads[0], loads[1], loads[2]
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn system_info() -> String {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let cpu_count = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    // `os.release()` is the kernel version on these platforms; sysinfo has no
    // `uname -v` equivalent for `os.version()`, so the product version stands in.
    format!(
        "{cpu_count}\0{}\0{}\0{}\0{}",
        system.total_memory(),
        System::kernel_version().unwrap_or_default(),
        System::long_os_version().unwrap_or_default(),
        System::host_name().unwrap_or_default(),
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn live_info() -> String {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    let loads = System::load_average();
    format!(
        "{}\0{}\0{}\0{}\0{}",
        system.available_memory(),
        System::uptime(),
        loads.one,
        loads.five,
        loads.fifteen,
    )
}

#[cfg(windows)]
fn system_info() -> String {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let cpu_count = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    format!(
        "{cpu_count}\0{}\0{}\0{}\0{}",
        system.total_memory(),
        System::os_version().unwrap_or_default(),
        System::long_os_version().unwrap_or_default(),
        System::host_name().unwrap_or_default(),
    )
}

#[cfg(windows)]
fn live_info() -> String {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
    );
    // Windows has no load average; Node reports zeros there as well.
    format!(
        "{}\0{}\0{}\0{}\0{}",
        system.available_memory(),
        System::uptime(),
        "0",
        "0",
        "0",
    )
}

#[op2]
#[string]
pub(crate) fn op_os_system_info() -> String {
    system_info()
}

#[op2]
#[string]
pub(crate) fn op_os_live_info() -> String {
    live_info()
}

#[cfg(target_os = "linux")]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "`cpu MHz` is a small non-negative frequency, so rounding makes the cast exact"
)]
const fn rounded_mhz(mhz: f64) -> u64 {
    mhz.round() as u64
}

#[cfg(target_os = "linux")]
fn cpus() -> String {
    const USER_HZ: u64 = 100;
    const MILLISECONDS: u64 = 1000;

    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let mut models: Vec<(String, u64)> = Vec::new();
    let mut model = String::new();
    let mut speed = 0u64;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "processor" => {
                if !model.is_empty() {
                    models.push((model.clone(), speed));
                }
                model.clear();
                speed = 0;
            }
            "model name" => value.trim().clone_into(&mut model),
            "cpu MHz" => speed = value.trim().parse::<f64>().map_or(0, rounded_mhz),
            _ => {}
        }
    }
    if !model.is_empty() {
        models.push((model, speed));
    }

    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let mut fields: Vec<String> = Vec::new();
    let mut index = 0;
    for line in stat.lines() {
        if !line.starts_with("cpu") || line.starts_with("cpu ") {
            continue;
        }
        let mut values = line
            .split_whitespace()
            .skip(1)
            .filter_map(|value| value.parse::<u64>().ok());
        let user = values.next().unwrap_or(0);
        let nice = values.next().unwrap_or(0);
        let system = values.next().unwrap_or(0);
        let idle = values.next().unwrap_or(0);
        let iowait = values.next().unwrap_or(0);
        let irq = values.next().unwrap_or(0);
        let softirq = values.next().unwrap_or(0);
        let (model, speed) = models
            .get(index)
            .cloned()
            .unwrap_or_else(|| (String::from("unknown"), 0));
        index += 1;
        let to_ms = |ticks: u64| (ticks * MILLISECONDS / USER_HZ).to_string();
        fields.push(model);
        fields.push(speed.to_string());
        fields.push(to_ms(user));
        fields.push(to_ms(nice));
        fields.push(to_ms(system));
        fields.push(to_ms(idle + iowait));
        fields.push(to_ms(irq + softirq));
    }
    fields.join("\0")
}

// Shared by every non-Linux target: per-cpu brand and frequency are real, but
// per-cpu times are not collected (the Windows build reports zeros the same
// way).
#[cfg(not(target_os = "linux"))]
fn cpus() -> String {
    use sysinfo::{CpuRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_frequency()),
    );
    let cpus = system.cpus();
    let model = cpus.first().map_or("unknown", sysinfo::Cpu::brand);
    let speed = cpus.first().map_or(0, sysinfo::Cpu::frequency);
    let count = cpus.len().max(1);
    let mut fields = Vec::with_capacity(count * 7);
    for _ in 0..count {
        fields.push(model.to_owned());
        fields.push(speed.to_string());
        for _ in 0..5 {
            fields.push("0".to_owned());
        }
    }
    fields.join("\0")
}

#[op2]
#[string]
pub(crate) fn op_os_cpus() -> String {
    cpus()
}

#[op2]
#[string]
pub(crate) fn op_os_network_interfaces() -> Result<String, JsErrorBox> {
    let interfaces = netif::up().map_err(|error| {
        let errno = error.raw_os_error().unwrap_or(5);
        JsErrorBox::new("Error", (-errno).to_string())
    })?;
    let mut entries = Vec::new();
    for interface in interfaces {
        let address = interface.address();
        let family = match address {
            IpAddr::V4(_) => "IPv4",
            IpAddr::V6(_) => "IPv6",
        };
        let (cidr, prefix) = interface.cidr();
        let mac = interface
            .mac()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<String>>()
            .join(":");
        entries.push(format!(
            "{}\0{address}\0{}\0{family}\0{mac}\0{}\0{cidr}/{prefix}\0{}",
            interface.name(),
            interface.netmask(),
            u8::from(address.is_loopback()),
            interface.scope_id().unwrap_or(0),
        ));
    }
    Ok(entries.join("\n"))
}

#[op2]
#[string]
pub(crate) fn op_os_homedir() -> String {
    dirs::home_dir().map_or_else(String::new, |path| path.to_string_lossy().into_owned())
}

#[op2]
#[string]
pub(crate) fn op_os_tmpdir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::live_info;

    #[test]
    fn live_info_reports_five_numeric_fields() {
        let info = live_info();
        let fields: Vec<&str> = info.split('\0').collect();
        assert_eq!(fields.len(), 5);
        assert!(fields.iter().all(|field| field.parse::<f64>().is_ok()));
    }
}
