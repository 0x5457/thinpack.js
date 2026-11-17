use std::collections::HashMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, Ipv6Addr};

use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;

pub(crate) const OPS: &[OpDecl] = &[
    op_os_system_info(),
    op_os_cpus(),
    op_os_network_interfaces(),
];

fn proc_text(path: &str, fallback: &str) -> String {
    std::fs::read_to_string(path)
        .map_or_else(|_| fallback.to_owned(), |text| text.trim().to_owned())
}

#[op2]
#[string]
pub(crate) fn op_os_system_info() -> String {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu_count = cpuinfo
        .lines()
        .filter(|line| line.trim_start().starts_with("processor"))
        .count()
        .max(1);

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
        "{cpu_count}\0{total}\0{available}\0{}\0{}\0{}\0{uptime}\0{}\0{}\0{}",
        proc_text("/proc/sys/kernel/osrelease", "unknown"),
        proc_text("/proc/version", "unknown"),
        proc_text("/proc/sys/kernel/hostname", "localhost"),
        loads[0],
        loads[1],
        loads[2],
    )
}

unsafe fn text_of(address: *const libc::sockaddr, family: i32) -> Option<String> {
    if address.is_null() {
        return None;
    }
    match family {
        libc::AF_INET => {
            // SAFETY: `family` is `AF_INET`, so `address` points to a `sockaddr_in`.
            let address = unsafe { &*address.cast::<libc::sockaddr_in>() };
            Some(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()).to_string())
        }
        libc::AF_INET6 => {
            // SAFETY: `family` is `AF_INET6`, so `address` points to a `sockaddr_in6`.
            let address = unsafe { &*address.cast::<libc::sockaddr_in6>() };
            Some(Ipv6Addr::from(address.sin6_addr.s6_addr).to_string())
        }
        _ => None,
    }
}

unsafe fn prefix_of(netmask: *const libc::sockaddr, family: i32) -> u32 {
    if netmask.is_null() {
        return 0;
    }
    match family {
        libc::AF_INET => {
            // SAFETY: `family` is `AF_INET`, so `netmask` points to a `sockaddr_in`.
            let mask = unsafe { &*netmask.cast::<libc::sockaddr_in>() };
            u32::from_be(mask.sin_addr.s_addr).count_ones()
        }
        libc::AF_INET6 => {
            // SAFETY: `family` is `AF_INET6`, so `netmask` points to a `sockaddr_in6`.
            let mask = unsafe { &*netmask.cast::<libc::sockaddr_in6>() };
            mask.sin6_addr
                .s6_addr
                .iter()
                .map(|byte| byte.count_ones())
                .sum()
        }
        _ => 0,
    }
}

#[cfg(target_os = "linux")]
unsafe fn mac_of(address: *const libc::sockaddr) -> Option<String> {
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller passes the address of an `AF_PACKET` entry, i.e. a `sockaddr_ll`.
    let link = unsafe { &*address.cast::<libc::sockaddr_ll>() };
    if link.sll_halen != 6 {
        return None;
    }
    let bytes: Vec<String> = link.sll_addr[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(bytes.join(":"))
}

#[cfg(target_vendor = "apple")]
unsafe fn mac_of(address: *const libc::sockaddr) -> Option<String> {
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller passes the address of an `AF_LINK` entry, i.e. a `sockaddr_dl`.
    let link = unsafe { &*address.cast::<libc::sockaddr_dl>() };
    if link.sdl_alen != 6 {
        return None;
    }
    let start = usize::from(link.sdl_nlen);
    let bytes = link.sdl_data.get(start..start + 6)?;
    Some(
        bytes
            .iter()
            .map(|byte| format!("{:02x}", byte.cast_unsigned()))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

#[cfg(target_os = "linux")]
const LINK_FAMILY: libc::c_int = libc::AF_PACKET;
#[cfg(target_vendor = "apple")]
const LINK_FAMILY: libc::c_int = libc::AF_LINK;
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
const LINK_FAMILY: libc::c_int = -1;

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
unsafe fn mac_of(_address: *const libc::sockaddr) -> Option<String> {
    None
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "`cpu MHz` is a small non-negative frequency, so rounding makes the cast exact"
)]
const fn rounded_mhz(mhz: f64) -> u64 {
    mhz.round() as u64
}

#[op2]
#[string]
pub(crate) fn op_os_cpus() -> String {
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

#[op2]
#[string]
pub(crate) fn op_os_network_interfaces() -> Result<String, JsErrorBox> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` only writes the head of the list it allocates through the pointer.
    if unsafe { libc::getifaddrs(&raw mut head) } != 0 {
        let errno = std::io::Error::last_os_error()
            .raw_os_error()
            .map_or(5, |errno| -errno);
        return Err(JsErrorBox::new("Error", errno.to_string()));
    }

    let mut macs: HashMap<String, String> = HashMap::new();
    let mut entries: Vec<String> = Vec::new();

    let mut current = head;
    while !current.is_null() {
        // SAFETY: `current` is a non-null node of the list owned by `head`.
        let entry = unsafe { &*current };
        // SAFETY: `ifa_name` points to a NUL-terminated string inside the same node.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        if !entry.ifa_addr.is_null() {
            // SAFETY: `ifa_addr` is non-null and starts with the common `sockaddr` header.
            let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
            let mac = if family == LINK_FAMILY {
                // SAFETY: a `LINK_FAMILY` entry carries a link-layer address in `ifa_addr`.
                unsafe { mac_of(entry.ifa_addr) }
            } else {
                None
            };
            if let Some(mac) = mac {
                macs.insert(name, mac);
            }
        }
        current = entry.ifa_next;
    }

    current = head;
    while !current.is_null() {
        // SAFETY: `current` is a non-null node of the list owned by `head`.
        let entry = unsafe { &*current };
        // SAFETY: `ifa_name` points to a NUL-terminated string inside the same node.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        if !entry.ifa_addr.is_null() {
            // SAFETY: `ifa_addr` is non-null and starts with the common `sockaddr` header.
            let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
            if family == libc::AF_INET || family == libc::AF_INET6 {
                // SAFETY: `family` reports the layout of `ifa_addr`.
                let address = unsafe { text_of(entry.ifa_addr, family) }.unwrap_or_default();
                // SAFETY: `ifa_netmask` is either null or a `sockaddr` of the same family.
                let netmask = unsafe { text_of(entry.ifa_netmask, family) }.unwrap_or_else(|| {
                    if family == libc::AF_INET {
                        "0.0.0.0".to_owned()
                    } else {
                        "::".to_owned()
                    }
                });
                // SAFETY: `ifa_netmask` is either null or a `sockaddr` of the same family.
                let prefix = unsafe { prefix_of(entry.ifa_netmask, family) };
                let internal = name == "lo"
                    || address.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
                    || address.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback());
                let scope = if family == libc::AF_INET6 {
                    // SAFETY: an `AF_INET6` entry carries a `sockaddr_in6` in `ifa_addr`.
                    unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in6>()).sin6_scope_id }
                } else {
                    0
                };
                let mac = macs
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| "00:00:00:00:00:00".to_owned());
                entries.push(format!(
                    "{name}\0{address}\0{netmask}\0{}\0{mac}\0{}\0{address}/{prefix}\0{scope}",
                    if family == libc::AF_INET {
                        "IPv4"
                    } else {
                        "IPv6"
                    },
                    u8::from(internal),
                ));
            }
        }
        current = entry.ifa_next;
    }
    // SAFETY: `head` is the list allocated by the successful `getifaddrs` call above.
    unsafe { libc::freeifaddrs(head) };
    Ok(entries.join("\n"))
}
