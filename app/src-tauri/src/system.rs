//! What this PC has, for the first-run setup: CPU, memory, graphics cards and free disk space.
//! Everything here is read locally; nothing is sent anywhere.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub cpu: String,
    pub cores: usize,
    pub ram_mb: u64,
    pub gpus: Vec<Gpu>,
    /// Free space on the drive models are saved to.
    pub free_disk_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub vram_mb: u64,
    /// NVIDIA cards run Whisper with CUDA, which this build needs.
    pub nvidia: bool,
}

impl SystemInfo {
    /// The NVIDIA card with the most memory, which is the one Whisper will use.
    pub fn best_nvidia(&self) -> Option<&Gpu> {
        self.gpus.iter().filter(|g| g.nvidia).max_by_key(|g| g.vram_mb)
    }
}

const NVIDIA: u32 = 0x10DE;
const MB: u64 = 1024 * 1024;

pub fn detect(models_dir: &Path) -> SystemInfo {
    SystemInfo {
        cpu: cpu_name().unwrap_or_else(|| "Unknown processor".into()),
        cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        ram_mb: ram_mb().unwrap_or(0),
        gpus: gpus(),
        free_disk_mb: free_disk_mb(models_dir),
    }
}

fn gpus() -> Vec<Gpu> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};

    let mut out: Vec<Gpu> = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
            continue; // "Microsoft Basic Render Driver"
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let gpu = Gpu {
            name: String::from_utf16_lossy(&desc.Description[..len]).trim().to_string(),
            vram_mb: desc.DedicatedVideoMemory as u64 / MB,
            nvidia: desc.VendorId == NVIDIA,
        };
        // A card can show up once per output it drives.
        if !out.contains(&gpu) {
            out.push(gpu);
        }
    }
    out
}

fn ram_mb() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    Some(m.ullTotalPhys / MB)
}

fn cpu_name() -> Option<String> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let mut buf = [0u16; 256];
    let mut bytes = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]).split_whitespace().collect::<Vec<_>>().join(" "))
}

fn free_disk_mb(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    // The folder may not exist yet; ask about the closest one that does.
    let existing = dir.ancestors().find(|p| p.exists())?;
    let wide: Vec<u16> = existing.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free / MB)
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_this_machine() {
        let info = super::detect(&std::env::temp_dir());
        eprintln!("{info:#?}");
        assert!(info.cores >= 1);
        assert!(info.ram_mb > 512, "{info:?}");
        assert!(!info.cpu.is_empty());
        assert!(info.free_disk_mb.is_some());
    }
}
