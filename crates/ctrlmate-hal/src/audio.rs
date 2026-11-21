//! Cross-platform Audio Control (Master Volume, Mute, Playback Device Enumeration & Switching).
//!
//! - Windows: Native WASAPI (`IMMDeviceEnumerator`, `IAudioEndpointVolume`) + `IPolicyConfig` COM
//!   to switch the default playback output (Headphones <-> Speakers <-> Monitor) across all roles.
//! - Linux: Native PipeWire / WirePlumber (`wpctl` / `pactl`).

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct AudioSnapshot {
    pub volume_percent: f64,
    pub muted: bool,
    pub current_output: String,
    pub outputs: Vec<AudioDevice>,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            volume_percent: 50.0,
            muted: false,
            current_output: "Default Output".into(),
            outputs: vec![AudioDevice {
                id: "default".into(),
                name: "Default Output".into(),
                is_default: true,
            }],
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(non_snake_case, dead_code)]
mod imp {
    use super::{AudioDevice, AudioSnapshot};
    use anyhow::{bail, Context, Result};
    use std::ffi::c_void;
    use windows::core::{interface, GUID, HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCommunications, eConsole, eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator,
        MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CoTaskMemFree, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
    };

    const CLSID_POLICY_CONFIG_CLIENT: GUID =
        GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Undocumented Windows Vista–11 `IPolicyConfig` COM interface used by EarTrumpet & SoundVolumeView
    /// to switch the default audio output device.
    #[interface("F8679F50-850A-41CF-9C72-430F290290C8")]
    unsafe trait IPolicyConfig: windows::core::IUnknown {
        unsafe fn GetMixFormat(&self, device_id: PCWSTR, pp_format: *mut *mut c_void) -> HRESULT;
        unsafe fn GetDeviceFormat(
            &self,
            device_id: PCWSTR,
            default_fmt: i32,
            pp_format: *mut *mut c_void,
        ) -> HRESULT;
        unsafe fn ResetDeviceFormat(&self, device_id: PCWSTR) -> HRESULT;
        unsafe fn SetDeviceFormat(
            &self,
            device_id: PCWSTR,
            p_endpoint_fmt: *const c_void,
            p_mix_fmt: *const c_void,
        ) -> HRESULT;
        unsafe fn GetProcessingPeriod(
            &self,
            device_id: PCWSTR,
            default_period: i32,
            pmft_default: *mut i64,
            pmft_min: *mut i64,
        ) -> HRESULT;
        unsafe fn SetProcessingPeriod(&self, device_id: PCWSTR, pmft_period: *const i64) -> HRESULT;
        unsafe fn GetShareMode(&self, device_id: PCWSTR, p_mode: *mut c_void) -> HRESULT;
        unsafe fn SetShareMode(&self, device_id: PCWSTR, p_mode: *const c_void) -> HRESULT;
        unsafe fn GetPropertyValue(
            &self,
            device_id: PCWSTR,
            key: *const c_void,
            pv: *mut c_void,
        ) -> HRESULT;
        unsafe fn SetPropertyValue(
            &self,
            device_id: PCWSTR,
            key: *const c_void,
            pv: *const c_void,
        ) -> HRESULT;
        unsafe fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: u32) -> HRESULT;
        unsafe fn SetEndpointVisibility(&self, device_id: PCWSTR, visible: i32) -> HRESULT;
    }

    struct ComScope(bool);
    impl ComScope {
        fn enter() -> Self {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Self(hr.is_ok())
        }
    }
    impl Drop for ComScope {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn device_id_string(dev: &IMMDevice) -> Result<String> {
        unsafe {
            let pwstr: PWSTR = dev.GetId()?;
            let s = pwstr.to_string().unwrap_or_default();
            CoTaskMemFree(Some(pwstr.0 as *const c_void));
            Ok(s)
        }
    }

    fn device_friendly_name(dev: &IMMDevice) -> Result<String> {
        unsafe {
            let store = dev.OpenPropertyStore(STGM_READ)?;
            let prop = store.GetValue(&PKEY_Device_FriendlyName)?;
            let pwstr = PropVariantToStringAlloc(&prop)?;
            let s = pwstr.to_string().unwrap_or_else(|_| "Audio Device".into());
            CoTaskMemFree(Some(pwstr.0 as *const c_void));
            Ok(s)
        }
    }

    pub fn poll_audio() -> Result<AudioSnapshot> {
        let _com = ComScope::enter();
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let default_dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let default_id = device_id_string(&default_dev)?;
            let default_name = device_friendly_name(&default_dev)
                .unwrap_or_else(|_| "Default Output".into());

            let endpoint_vol: IAudioEndpointVolume = default_dev.Activate(CLSCTX_ALL, None)?;
            let scalar = endpoint_vol.GetMasterVolumeLevelScalar()?;
            let muted = endpoint_vol.GetMute()?.as_bool();

            let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let count = collection.GetCount()?;
            let mut outputs = Vec::with_capacity(count as usize);
            for i in 0..count {
                if let Ok(dev) = collection.Item(i) {
                    if let (Ok(id), Ok(name)) = (device_id_string(&dev), device_friendly_name(&dev))
                    {
                        outputs.push(AudioDevice {
                            is_default: id == default_id,
                            id,
                            name,
                        });
                    }
                }
            }

            Ok(AudioSnapshot {
                volume_percent: (f64::from(scalar) * 100.0).round(),
                muted,
                current_output: default_name,
                outputs,
            })
        }
    }

    pub fn set_volume(percent: f64) -> Result<f64> {
        let _com = ComScope::enter();
        let clamped = percent.clamp(0.0, 100.0);
        let scalar = (clamped / 100.0) as f32;
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let default_dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let endpoint_vol: IAudioEndpointVolume = default_dev.Activate(CLSCTX_ALL, None)?;
            endpoint_vol.SetMasterVolumeLevelScalar(scalar, std::ptr::null())?;
        }
        Ok(clamped.round())
    }

    pub fn set_mute(muted: bool) -> Result<bool> {
        let _com = ComScope::enter();
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let default_dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let endpoint_vol: IAudioEndpointVolume = default_dev.Activate(CLSCTX_ALL, None)?;
            endpoint_vol.SetMute(muted, std::ptr::null())?;
        }
        Ok(muted)
    }

    pub fn set_default_output_by_name(target_name: &str) -> Result<String> {
        let _com = ComScope::enter();
        let snap = poll_audio()?;
        let found = snap
            .outputs
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(target_name) || d.id == target_name)
            .with_context(|| format!("Audio output '{target_name}' not found"))?;

        let wide: Vec<u16> = found.id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let policy: IPolicyConfig =
                CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
            for role in [eConsole.0 as u32, eMultimedia.0 as u32, eCommunications.0 as u32] {
                let hr = policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role);
                if hr.is_err() {
                    bail!("IPolicyConfig::SetDefaultEndpoint failed (0x{:08X})", hr.0);
                }
            }
        }
        Ok(found.name.clone())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{AudioDevice, AudioSnapshot};
    use anyhow::{bail, Context, Result};
    use std::process::Command;

    pub fn poll_audio() -> Result<AudioSnapshot> {
        let vol_out = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
            .context("wpctl unavailable")?;
        let text = String::from_utf8_lossy(&vol_out.stdout);
        // Format: "Volume: 0.55" or "Volume: 0.55 [MUTED]"
        let muted = text.contains("MUTED");
        let volume_percent = text
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (v * 100.0).round())
            .unwrap_or(50.0);

        let mut outputs = Vec::new();
        let mut current_output = "Default Output".to_string();

        if let Ok(sinks) = Command::new("pactl").args(["list", "short", "sinks"]).output() {
            let def_name = Command::new("pactl")
                .args(["get-default-sink"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            for line in String::from_utf8_lossy(&sinks.stdout).lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 {
                    let name = parts[1].to_string();
                    let is_default = name == def_name;
                    if is_default {
                        current_output = name.clone();
                    }
                    outputs.push(AudioDevice {
                        id: parts[0].to_string(),
                        name,
                        is_default,
                    });
                }
            }
        }

        if outputs.is_empty() {
            outputs.push(AudioDevice {
                id: "default".into(),
                name: current_output.clone(),
                is_default: true,
            });
        }

        Ok(AudioSnapshot {
            volume_percent,
            muted,
            current_output,
            outputs,
        })
    }

    pub fn set_volume(percent: f64) -> Result<f64> {
        let clamped = percent.clamp(0.0, 100.0).round();
        let status = Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", clamped / 100.0)])
            .status()?;
        if !status.success() {
            bail!("wpctl set-volume failed");
        }
        Ok(clamped)
    }

    pub fn set_mute(muted: bool) -> Result<bool> {
        let flag = if muted { "1" } else { "0" };
        let status = Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", flag])
            .status()?;
        if !status.success() {
            bail!("wpctl set-mute failed");
        }
        Ok(muted)
    }

    pub fn set_default_output_by_name(target_name: &str) -> Result<String> {
        let status = Command::new("pactl")
            .args(["set-default-sink", target_name])
            .status()?;
        if !status.success() {
            bail!("pactl set-default-sink failed for '{target_name}'");
        }
        Ok(target_name.to_string())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::AudioSnapshot;
    use anyhow::{bail, Result};

    pub fn poll_audio() -> Result<AudioSnapshot> {
        Ok(AudioSnapshot::default())
    }
    pub fn set_volume(_: f64) -> Result<f64> {
        bail!("Unsupported OS")
    }
    pub fn set_mute(_: bool) -> Result<bool> {
        bail!("Unsupported OS")
    }
    pub fn set_default_output_by_name(_: &str) -> Result<String> {
        bail!("Unsupported OS")
    }
}

pub fn poll_audio() -> AudioSnapshot {
    imp::poll_audio().unwrap_or_default()
}

pub fn set_volume(percent: f64) -> Result<f64> {
    imp::set_volume(percent)
}

pub fn set_mute(muted: bool) -> Result<bool> {
    imp::set_mute(muted)
}

pub fn set_default_output_by_name(target_name: &str) -> Result<String> {
    if target_name.trim().is_empty() {
        bail!("Audio output name cannot be empty");
    }
    imp::set_default_output_by_name(target_name)
}
