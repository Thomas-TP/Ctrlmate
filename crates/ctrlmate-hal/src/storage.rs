//! Storage telemetry: one entry per **physical drive**, with usage aggregated over its volumes and
//! the drive's own temperature.
//!
//! Volumes are matched to physical drives by device identity, never by list position:
//! * Windows: `IOCTL_STORAGE_GET_DEVICE_NUMBER` on the volume gives its `PhysicalDriveN`;
//!   `IOCTL_STORAGE_QUERY_PROPERTY` gives model, serial and the temperature of that drive.
//! * Linux: the mounted partition (`/dev/nvme0n1p2`, `/dev/sda1`) is resolved to its parent disk, whose
//!   model / serial / hwmon temperature (`nvme` controller hwmon, `drivetemp`) are read from sysfs.
//!
//! A volume that cannot be matched (Storage Spaces, network, exotic mappings) is reported on its own
//! without a temperature. Disk lists are refreshed in place; the physical layout is re-enumerated
//! only when the set of mounted volumes changes or every few minutes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use sysinfo::Disks;

const RELAYOUT_EVERY: Duration = Duration::from_secs(300);
const MIN_VOLUME_BYTES: u64 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct DriveDescriptor {
    pub id: String,
    pub name: String,
    pub manufacturer: Option<String>,
    pub has_temperature: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriveSample {
    pub id: String,
    pub used_percent: f64,
    pub temperature_c: Option<f64>,
}

/// Where a drive's temperature comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TempSource {
    None,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    PhysicalDrive(u32),
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    SysfsFile(std::path::PathBuf),
}

#[derive(Debug, Clone)]
struct PhysDrive {
    id: String,
    name: String,
    /// Mount points (`C:\`, `/home`) of the volumes on this drive.
    mounts: Vec<String>,
    temp: TempSource,
}

pub struct StorageProvider {
    disks: Disks,
    layout: Vec<PhysDrive>,
    layout_mounts: Vec<String>,
    layout_at: Instant,
}

impl Default for StorageProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn mounts_of(disks: &Disks) -> Vec<String> {
    let mut m: Vec<String> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() >= MIN_VOLUME_BYTES)
        .map(|d| d.mount_point().to_string_lossy().into_owned())
        .collect();
    m.sort();
    m.dedup();
    m
}

impl StorageProvider {
    pub fn new() -> Self {
        let disks = Disks::new_with_refreshed_list();
        let mut p = Self {
            disks,
            layout: Vec::new(),
            layout_mounts: Vec::new(),
            layout_at: Instant::now(),
        };
        p.relayout();
        p
    }

    fn relayout(&mut self) {
        self.layout_mounts = mounts_of(&self.disks);
        let vols: Vec<(String, String)> = self
            .disks
            .list()
            .iter()
            .filter(|d| d.total_space() >= MIN_VOLUME_BYTES)
            .map(|d| {
                (
                    d.mount_point().to_string_lossy().into_owned(),
                    d.name().to_string_lossy().into_owned(),
                )
            })
            .collect();
        self.layout = imp::build_layout(&vols);
        self.layout_at = Instant::now();
    }

    fn refresh(&mut self) {
        self.disks.refresh(true);
        // Mount / unmount: new volumes need a (cheap) re-enumeration of the physical layout.
        if mounts_of(&self.disks) != self.layout_mounts || self.layout_at.elapsed() > RELAYOUT_EVERY
        {
            self.disks = Disks::new_with_refreshed_list();
            self.relayout();
        }
    }

    pub fn descriptors(&mut self) -> Vec<DriveDescriptor> {
        self.refresh();
        self.layout
            .iter()
            .map(|d| DriveDescriptor {
                id: d.id.clone(),
                name: d.name.clone(),
                manufacturer: infer_storage_vendor(&d.name),
                has_temperature: read_temperature(&d.temp).is_some(),
            })
            .collect()
    }

    pub fn poll(&mut self) -> Vec<DriveSample> {
        self.refresh();
        let usage: BTreeMap<String, (u64, u64)> = self
            .disks
            .list()
            .iter()
            .map(|d| {
                (
                    d.mount_point().to_string_lossy().into_owned(),
                    (d.total_space(), d.available_space()),
                )
            })
            .collect();
        self.layout
            .iter()
            .map(|drv| {
                let (mut total, mut avail) = (0u64, 0u64);
                for m in &drv.mounts {
                    if let Some((t, a)) = usage.get(m) {
                        total += t;
                        avail += a.min(t);
                    }
                }
                DriveSample {
                    id: drv.id.clone(),
                    used_percent: used_percent(total, avail),
                    temperature_c: read_temperature(&drv.temp),
                }
            })
            .collect()
    }
}

fn used_percent(total: u64, available: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    total.saturating_sub(available) as f64 / total as f64 * 100.0
}

fn infer_storage_vendor(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    [
        ("samsung", "Samsung"),
        ("western digital", "Western Digital"),
        ("wd ", "Western Digital"),
        ("wdc", "Western Digital"),
        ("crucial", "Crucial"),
        ("micron", "Crucial"),
        ("kingston", "Kingston"),
        ("sabrent", "Sabrent"),
        ("corsair", "Corsair"),
        ("seagate", "Seagate"),
        ("sandisk", "SanDisk"),
        ("toshiba", "Toshiba"),
    ]
    .iter()
    .find(|(k, _)| lower.contains(k))
    .map(|(_, v)| (*v).to_string())
}

/// Plausible drive temperatures only (an IOCTL may return garbage on odd controllers).
fn plausible(c: f64) -> Option<f64> {
    (-20.0..=120.0).contains(&c).then_some(c)
}

fn read_temperature(src: &TempSource) -> Option<f64> {
    match src {
        TempSource::None => None,
        TempSource::PhysicalDrive(n) => imp::read_physical_temperature(*n).and_then(plausible),
        TempSource::SysfsFile(p) => std::fs::read_to_string(p)
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|m| m / 1000.0)
            .and_then(plausible),
    }
}

fn sanitize_id(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `nvme0n1p2` -> `nvme0n1`, `mmcblk0p1` -> `mmcblk0`, `sda1` -> `sda`, `sda` -> `sda`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parent_disk_name(part: &str) -> String {
    let name = part.trim_start_matches("/dev/");
    let is_p_style =
        name.starts_with("nvme") || name.starts_with("mmcblk") || name.starts_with("loop");
    if is_p_style {
        if let Some(i) = name.rfind('p')
            && i > 0
            && name[i + 1..].chars().all(|c| c.is_ascii_digit())
            && !name[i + 1..].is_empty()
            && name[..i].chars().last().is_some_and(|c| c.is_ascii_digit())
        {
            return name[..i].to_string();
        }
        return name.to_string();
    }
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string()
}

/// `nvme0n1` -> `nvme0` (controller).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn nvme_controller(disk: &str) -> Option<String> {
    let rest = disk.strip_prefix("nvme")?;
    let n_pos = rest.find('n')?;
    Some(format!("nvme{}", &rest[..n_pos]))
}

/// Linux sysfs facts of one disk, read below `sys_root` (`/sys`). Pure file access: testable.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Default)]
struct SysDisk {
    model: Option<String>,
    serial: Option<String>,
    temp_file: Option<std::path::PathBuf>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn read_sys_disk(sys_root: &std::path::Path, disk: &str) -> SysDisk {
    let trim = |p: std::path::PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let first_hwmon_temp = |dir: std::path::PathBuf| {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        v.into_iter()
            .map(|d| d.join("temp1_input"))
            .find(|p| p.is_file())
    };
    if let Some(ctrl) = nvme_controller(disk) {
        let base = sys_root.join("class/nvme").join(&ctrl);
        return SysDisk {
            model: trim(base.join("model")),
            serial: trim(base.join("serial")),
            temp_file: first_hwmon_temp(base),
        };
    }
    let dev = sys_root.join("class/block").join(disk).join("device");
    SysDisk {
        model: trim(dev.join("model")),
        serial: trim(dev.join("serial")).or_else(|| trim(dev.join("wwid"))),
        temp_file: first_hwmon_temp(dev.join("hwmon")),
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{PhysDrive, TempSource, sanitize_id};
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
        STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
        STORAGE_TEMPERATURE_DATA_DESCRIPTOR, StorageDeviceProperty,
        StorageDeviceTemperatureProperty,
    };
    use windows::core::PCWSTR;

    /// Query-only handle (access 0); works for volumes and, elevated, for physical drives.
    fn open(path: &str) -> Option<HANDLE> {
        let wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: valid NUL-terminated wide string; the handle is closed by the callers.
        unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .ok()
    }

    fn close(h: HANDLE) {
        // SAFETY: `h` came from CreateFileW.
        unsafe {
            let _ = CloseHandle(h);
        }
    }

    fn query_property(
        h: HANDLE,
        id: windows::Win32::System::Ioctl::STORAGE_PROPERTY_ID,
        out: &mut [u8],
    ) -> Option<usize> {
        let q = STORAGE_PROPERTY_QUERY {
            PropertyId: id,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut ret = 0u32;
        // SAFETY: input and output buffers are valid for the sizes passed.
        unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some((&raw const q).cast()),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                Some(out.as_mut_ptr().cast()),
                out.len() as u32,
                Some(&mut ret),
                None,
            )
        }
        .ok()
        .map(|()| ret as usize)
    }

    fn cstr_at(buf: &[u8], off: u32) -> Option<String> {
        let off = off as usize;
        if off == 0 || off >= buf.len() {
            return None;
        }
        let s = &buf[off..];
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        let t = String::from_utf8_lossy(&s[..end]).trim().to_string();
        (!t.is_empty()).then_some(t)
    }

    fn volume_device_number(letter: char) -> Option<u32> {
        let h = open(&format!(r"\\.\{letter}:"))?;
        let mut out = STORAGE_DEVICE_NUMBER::default();
        let mut ret = 0u32;
        // SAFETY: `out` is a plain struct of the exact size passed.
        let r = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                None,
                0,
                Some((&raw mut out).cast()),
                size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                Some(&mut ret),
                None,
            )
        };
        close(h);
        r.ok().map(|()| out.DeviceNumber)
    }

    /// `(model, serial)` of `\\.\PhysicalDriveN`.
    fn drive_identity(n: u32) -> Option<(String, Option<String>)> {
        let h = open(&format!(r"\\.\PhysicalDrive{n}"))?;
        let mut buf = [0u8; 1024];
        let got = query_property(h, StorageDeviceProperty, &mut buf);
        close(h);
        let got = got?;
        if got < size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
            return None;
        }
        // SAFETY: the buffer holds at least a descriptor; it is read unaligned (a byte array has no
        // alignment guarantee).
        let d: STORAGE_DEVICE_DESCRIPTOR = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        let buf = &buf[..got];
        let vendor = cstr_at(buf, d.VendorIdOffset);
        let product = cstr_at(buf, d.ProductIdOffset);
        let serial = cstr_at(buf, d.SerialNumberOffset);
        let model = match (vendor, product) {
            (Some(v), Some(p)) if !v.eq_ignore_ascii_case("nvme") && !p.starts_with(&v) => {
                format!("{v} {p}")
            }
            (_, Some(p)) => p,
            (Some(v), None) => v,
            (None, None) => format!("Drive {n}"),
        };
        Some((model, serial))
    }

    pub fn read_physical_temperature(n: u32) -> Option<f64> {
        let h = open(&format!(r"\\.\PhysicalDrive{n}"))?;
        let mut buf = [0u8; 512];
        let got = query_property(h, StorageDeviceTemperatureProperty, &mut buf);
        close(h);
        let got = got?;
        if got < size_of::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>() {
            return None;
        }
        // SAFETY: see `drive_identity`.
        let d: STORAGE_TEMPERATURE_DATA_DESCRIPTOR =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        if d.InfoCount == 0 {
            return None;
        }
        let t = d.TemperatureInfo[0].Temperature;
        // 0x8000 = "not available" in the Windows API.
        (t != i16::MIN).then_some(f64::from(t))
    }

    pub fn build_layout(volumes: &[(String, String)]) -> Vec<PhysDrive> {
        use std::collections::BTreeMap;
        let mut by_drive: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        let mut loose: Vec<String> = Vec::new();
        for (mount, _) in volumes {
            let letter = mount.chars().next().filter(char::is_ascii_alphabetic);
            match letter.and_then(volume_device_number) {
                Some(n) => by_drive.entry(n).or_default().push(mount.clone()),
                None => loose.push(mount.clone()),
            }
        }
        let mut out = Vec::new();
        for (n, mounts) in by_drive {
            let model = drive_identity(n).map_or_else(|| format!("Drive {n}"), |(m, _serial)| m);
            let letters: Vec<_> = mounts
                .iter()
                .map(|m| m.trim_end_matches('\\').to_string())
                .collect();
            out.push(PhysDrive {
                id: format!("storage_{n}"),
                name: format!("{model} ({})", letters.join(", ")),
                mounts,
                temp: TempSource::PhysicalDrive(n),
            });
        }
        for m in loose {
            let letter = m.trim_end_matches(['\\', ':']).to_ascii_lowercase();
            out.push(PhysDrive {
                id: format!("storage_vol_{}", sanitize_id(&letter)),
                name: format!("Volume {}", m.trim_end_matches('\\')),
                mounts: vec![m],
                temp: TempSource::None,
            });
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{PhysDrive, TempSource, parent_disk_name, read_sys_disk, sanitize_id};
    use std::collections::BTreeMap;
    use std::path::Path;

    pub fn read_physical_temperature(_n: u32) -> Option<f64> {
        None
    }

    pub fn build_layout(volumes: &[(String, String)]) -> Vec<PhysDrive> {
        let mut by_disk: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut loose = Vec::new();
        for (mount, dev) in volumes {
            if dev.starts_with("/dev/") {
                by_disk
                    .entry(parent_disk_name(dev))
                    .or_default()
                    .push(mount.clone());
            } else {
                loose.push(mount.clone());
            }
        }
        let mut out = Vec::new();
        for (disk, mounts) in by_disk {
            let sys = read_sys_disk(Path::new("/sys"), &disk);
            let model = sys.model.clone().unwrap_or_else(|| disk.clone());
            out.push(PhysDrive {
                id: format!("storage_{}", sanitize_id(&disk)),
                name: format!("{model} ({})", mounts.join(", ")),
                mounts,
                temp: sys
                    .temp_file
                    .map_or(TempSource::None, TempSource::SysfsFile),
            });
        }
        for m in loose {
            out.push(PhysDrive {
                id: format!("storage_vol_{}", sanitize_id(&m)),
                name: format!("Volume {m}"),
                mounts: vec![m],
                temp: TempSource::None,
            });
        }
        out
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::PhysDrive;
    pub fn read_physical_temperature(_n: u32) -> Option<f64> {
        None
    }
    pub fn build_layout(_v: &[(String, String)]) -> Vec<PhysDrive> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn partitions_resolve_to_their_disk() {
        assert_eq!(parent_disk_name("/dev/nvme0n1p2"), "nvme0n1");
        assert_eq!(parent_disk_name("nvme1n1p10"), "nvme1n1");
        assert_eq!(parent_disk_name("/dev/nvme0n1"), "nvme0n1");
        assert_eq!(parent_disk_name("/dev/mmcblk0p1"), "mmcblk0");
        assert_eq!(parent_disk_name("/dev/sda1"), "sda");
        assert_eq!(parent_disk_name("/dev/sdb"), "sdb");
        assert_eq!(nvme_controller("nvme0n1").as_deref(), Some("nvme0"));
        assert_eq!(nvme_controller("sda"), None);
    }

    #[test]
    fn usage_and_temperature_rules() {
        assert_eq!(used_percent(1000, 250), 75.0);
        assert_eq!(used_percent(0, 0), 0.0);
        assert_eq!(plausible(45.0), Some(45.0));
        assert_eq!(plausible(300.0), None);
        assert_eq!(
            infer_storage_vendor("Samsung SSD 990 PRO 2TB").as_deref(),
            Some("Samsung")
        );
        assert_eq!(
            infer_storage_vendor("WD Blue").as_deref(),
            Some("Western Digital")
        );
    }

    #[test]
    fn sysfs_identity_for_nvme_and_sata() {
        let root = std::env::temp_dir().join(format!("ctrlmate-hal-stor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let nvme = root.join("class/nvme/nvme0");
        fs::create_dir_all(nvme.join("hwmon4")).unwrap();
        fs::write(nvme.join("model"), "Samsung SSD 990 PRO 2TB\n").unwrap();
        fs::write(nvme.join("serial"), "S7DNNJ0X123456\n").unwrap();
        fs::write(nvme.join("hwmon4/temp1_input"), "41850\n").unwrap();
        let sda = root.join("class/block/sda/device");
        fs::create_dir_all(sda.join("hwmon/hwmon7")).unwrap();
        fs::write(sda.join("model"), "WDC WD40EZAZ\n").unwrap();
        fs::write(sda.join("hwmon/hwmon7/temp1_input"), "33000\n").unwrap();

        let n = read_sys_disk(&root, "nvme0n1");
        assert_eq!(n.model.as_deref(), Some("Samsung SSD 990 PRO 2TB"));
        assert_eq!(n.serial.as_deref(), Some("S7DNNJ0X123456"));
        let t = read_temperature(&TempSource::SysfsFile(n.temp_file.unwrap()));
        assert_eq!(t, Some(41.85));
        let s = read_sys_disk(&root, "sda");
        assert_eq!(s.model.as_deref(), Some("WDC WD40EZAZ"));
        assert_eq!(
            read_temperature(&TempSource::SysfsFile(s.temp_file.unwrap())),
            Some(33.0)
        );
        // Unknown disk: nothing is invented.
        assert_eq!(read_sys_disk(&root, "sdz"), SysDisk::default());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn provider_lists_real_volumes_without_panicking() {
        let mut p = StorageProvider::new();
        let d = p.descriptors();
        let s = p.poll();
        // Every sample belongs to a described drive and percentages are sane.
        for x in &s {
            assert!(d.iter().any(|d| d.id == x.id));
            assert!((0.0..=100.0).contains(&x.used_percent));
        }
    }
}
