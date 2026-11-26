//! Master volume, mute and playback-device switching of the *user session*.
//!
//! * Windows: WASAPI (`IMMDeviceEnumerator`, `IAudioEndpointVolume`) and the undocumented
//!   `IPolicyConfig` COM interface (the one EarTrumpet / SoundVolumeView use) to switch the default
//!   output for every role. Device ids are the WASAPI endpoint ids.
//! * Linux: PipeWire / WirePlumber `wpctl`, falling back to PulseAudio-compatible `pactl`
//!   (works on PipeWire and PulseAudio, X11 and Wayland). Device ids are sink names.
//!
//! Nothing is fabricated: when the audio stack cannot be reached `poll` is an error and the caller
//! reports "no audio state" (never 50 % unmuted).

use super::SessionError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSnapshot {
    /// 0-100.
    pub volume_percent: f64,
    pub muted: bool,
    /// Id of the default output, when known.
    pub default_id: Option<String>,
    pub outputs: Vec<AudioDevice>,
}

pub fn poll() -> Result<AudioSnapshot, SessionError> {
    imp::poll()
}

pub fn set_volume(percent: f64) -> Result<(), SessionError> {
    if !percent.is_finite() {
        return Err(SessionError::Invalid("volume is not a number".into()));
    }
    imp::set_volume(percent.clamp(0.0, 100.0))
}

pub fn set_mute(muted: bool) -> Result<(), SessionError> {
    imp::set_mute(muted)
}

/// Switches the default playback output to the device with this id (as listed by [`poll`]).
pub fn set_default_output(id: &str) -> Result<(), SessionError> {
    if id.trim().is_empty() {
        return Err(SessionError::Invalid("empty output id".into()));
    }
    imp::set_default_output(id)
}

// ---------------------------------------------------------------------------------------------
// Pure parsers for the Linux tools (tested on every OS)
// ---------------------------------------------------------------------------------------------

/// `wpctl get-volume`: `Volume: 0.55` or `Volume: 0.55 [MUTED]` -> `(percent, muted)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_wpctl_volume(text: &str) -> Option<(f64, bool)> {
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("Volume:"))?;
    let v: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round(), line.contains("MUTED")))
}

/// `pactl get-sink-volume`: `Volume: front-left: 36044 /  55% / -15.00 dB, ...` -> first percentage.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_pactl_volume(text: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("Volume:"))?;
    line.split('/').map(str::trim).find_map(|p| {
        p.strip_suffix('%')
            .and_then(|n| n.trim().parse::<f64>().ok())
    })
}

/// `pactl get-sink-mute`: `Mute: yes`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_pactl_mute(text: &str) -> Option<bool> {
    let line = text.lines().find(|l| l.trim_start().starts_with("Mute:"))?;
    match line.split(':').nth(1)?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// `pactl list sinks` (C locale): pairs `Name:` / `Description:` -> `(name, description)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_pactl_sinks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(n) = l.strip_prefix("Name:") {
            name = Some(n.trim().to_string());
        } else if let Some(d) = l.strip_prefix("Description:")
            && let Some(n) = name.take()
        {
            out.push((n, d.trim().to_string()));
        }
    }
    out
}

#[cfg(target_os = "windows")]
#[allow(non_snake_case, clippy::missing_safety_doc)]
mod imp {
    use super::{AudioDevice, AudioSnapshot, SessionError};
    use std::ffi::c_void;
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCommunications,
        eConsole, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, STGM_READ,
    };
    use windows::core::{GUID, HRESULT, PCWSTR, PWSTR, interface};

    const CLSID_POLICY_CONFIG_CLIENT: GUID =
        GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// Undocumented `IPolicyConfig` (Vista - 11); only `SetDefaultEndpoint` is used, the other
    /// slots exist to keep the vtable layout right.
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
        unsafe fn SetProcessingPeriod(&self, device_id: PCWSTR, pmft_period: *const i64)
        -> HRESULT;
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
            // SAFETY: balanced by CoUninitialize in Drop when it succeeded.
            Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }
    impl Drop for ComScope {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: matches the successful CoInitializeEx.
                unsafe { CoUninitialize() };
            }
        }
    }

    fn fail(ctx: &str, e: impl std::fmt::Display) -> SessionError {
        SessionError::Failed(format!("{ctx}: {e}"))
    }

    fn id_of(dev: &IMMDevice) -> windows::core::Result<String> {
        // SAFETY: GetId returns a CoTaskMem string that is freed right after the copy.
        unsafe {
            let p: PWSTR = dev.GetId()?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            Ok(s)
        }
    }

    fn name_of(dev: &IMMDevice) -> windows::core::Result<String> {
        // SAFETY: property store access with an owned PROPVARIANT; the string is freed after use.
        unsafe {
            let store = dev.OpenPropertyStore(STGM_READ)?;
            let prop = store.GetValue(&PKEY_Device_FriendlyName)?;
            let p = PropVariantToStringAlloc(&prop)?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            Ok(s)
        }
    }

    fn default_endpoint() -> windows::core::Result<(IMMDeviceEnumerator, IMMDevice)> {
        // SAFETY: COM initialised by the caller's ComScope.
        unsafe {
            let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let d = e.GetDefaultAudioEndpoint(eRender, eConsole)?;
            Ok((e, d))
        }
    }

    pub fn poll() -> Result<AudioSnapshot, SessionError> {
        let _com = ComScope::enter();
        let (enumerator, default_dev) =
            default_endpoint().map_err(|e| fail("no default output", e))?;
        // SAFETY: COM calls on live interface pointers.
        unsafe {
            let default_id = id_of(&default_dev).map_err(|e| fail("default id", e))?;
            let vol: IAudioEndpointVolume = default_dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| fail("endpoint volume", e))?;
            let scalar = vol
                .GetMasterVolumeLevelScalar()
                .map_err(|e| fail("volume", e))?;
            let muted = vol.GetMute().map_err(|e| fail("mute", e))?.as_bool();
            let coll = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(|e| fail("enumerate", e))?;
            let mut outputs = Vec::new();
            for i in 0..coll.GetCount().unwrap_or(0) {
                if let Ok(dev) = coll.Item(i)
                    && let (Ok(id), Ok(name)) = (id_of(&dev), name_of(&dev))
                {
                    outputs.push(AudioDevice {
                        is_default: id == default_id,
                        id,
                        name,
                    });
                }
            }
            Ok(AudioSnapshot {
                volume_percent: (f64::from(scalar) * 100.0).round(),
                muted,
                default_id: Some(default_id),
                outputs,
            })
        }
    }

    pub fn set_volume(percent: f64) -> Result<(), SessionError> {
        let _com = ComScope::enter();
        let (_, dev) = default_endpoint().map_err(|e| fail("no default output", e))?;
        // SAFETY: COM calls on live interface pointers.
        unsafe {
            let vol: IAudioEndpointVolume = dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| fail("endpoint volume", e))?;
            vol.SetMasterVolumeLevelScalar((percent / 100.0) as f32, std::ptr::null())
                .map_err(|e| fail("set volume", e))
        }
    }

    pub fn set_mute(muted: bool) -> Result<(), SessionError> {
        let _com = ComScope::enter();
        let (_, dev) = default_endpoint().map_err(|e| fail("no default output", e))?;
        // SAFETY: COM calls on live interface pointers.
        unsafe {
            let vol: IAudioEndpointVolume = dev
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| fail("endpoint volume", e))?;
            vol.SetMute(muted, std::ptr::null())
                .map_err(|e| fail("set mute", e))
        }
    }

    pub fn set_default_output(id: &str) -> Result<(), SessionError> {
        let _com = ComScope::enter();
        let snap = poll()?;
        if !snap.outputs.iter().any(|d| d.id == id) {
            return Err(SessionError::Invalid("unknown output id".into()));
        }
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: valid NUL-terminated id; vtable matches the documented-by-reverse-engineering layout.
        unsafe {
            let policy: IPolicyConfig =
                CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                    .map_err(|e| fail("IPolicyConfig", e))?;
            for role in [
                eConsole.0 as u32,
                eMultimedia.0 as u32,
                eCommunications.0 as u32,
            ] {
                let hr = policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role);
                if hr.is_err() {
                    return Err(fail("SetDefaultEndpoint", format!("0x{:08X}", hr.0)));
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{
        AudioDevice, AudioSnapshot, SessionError, parse_pactl_mute, parse_pactl_sinks,
        parse_pactl_volume, parse_wpctl_volume,
    };
    use crate::session::exec::run_ok;
    use std::io::ErrorKind;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(4);

    fn io_err(tool: &str, e: &std::io::Error) -> SessionError {
        if e.kind() == ErrorKind::NotFound {
            SessionError::NotSupported(format!("{tool} is not installed"))
        } else {
            SessionError::Failed(e.to_string())
        }
    }

    pub fn poll() -> Result<AudioSnapshot, SessionError> {
        let (volume_percent, muted) =
            match run_ok("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"], T) {
                Ok(t) => parse_wpctl_volume(&t)
                    .ok_or_else(|| SessionError::Failed("unparsable wpctl output".into()))?,
                Err(_) => {
                    let v = run_ok("pactl", &["get-sink-volume", "@DEFAULT_SINK@"], T)
                        .map_err(|e| io_err("pactl", &e))?;
                    let m = run_ok("pactl", &["get-sink-mute", "@DEFAULT_SINK@"], T)
                        .map_err(|e| io_err("pactl", &e))?;
                    (
                        parse_pactl_volume(&v).ok_or_else(|| {
                            SessionError::Failed("unparsable pactl volume".into())
                        })?,
                        parse_pactl_mute(&m)
                            .ok_or_else(|| SessionError::Failed("unparsable pactl mute".into()))?,
                    )
                }
            };
        let default_name = run_ok("pactl", &["get-default-sink"], T)
            .ok()
            .map(|s| s.trim().to_string());
        let outputs = run_ok("pactl", &["list", "sinks"], T)
            .map(|t| {
                parse_pactl_sinks(&t)
                    .into_iter()
                    .map(|(id, name)| AudioDevice {
                        is_default: default_name.as_deref() == Some(id.as_str()),
                        id,
                        name,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(AudioSnapshot {
            volume_percent,
            muted,
            default_id: default_name.filter(|s| !s.is_empty()),
            outputs,
        })
    }

    pub fn set_volume(percent: f64) -> Result<(), SessionError> {
        let frac = format!("{:.2}", percent / 100.0);
        run_ok("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &frac], T)
            .or_else(|_| {
                run_ok(
                    "pactl",
                    &[
                        "set-sink-volume",
                        "@DEFAULT_SINK@",
                        &format!("{percent:.0}%"),
                    ],
                    T,
                )
            })
            .map(|_| ())
            .map_err(|e| io_err("wpctl/pactl", &e))
    }

    pub fn set_mute(muted: bool) -> Result<(), SessionError> {
        let flag = if muted { "1" } else { "0" };
        run_ok("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", flag], T)
            .or_else(|_| run_ok("pactl", &["set-sink-mute", "@DEFAULT_SINK@", flag], T))
            .map(|_| ())
            .map_err(|e| io_err("wpctl/pactl", &e))
    }

    pub fn set_default_output(id: &str) -> Result<(), SessionError> {
        let snap = poll()?;
        if !snap.outputs.iter().any(|d| d.id == id) {
            return Err(SessionError::Invalid("unknown output id".into()));
        }
        run_ok("pactl", &["set-default-sink", id], T)
            .map(|_| ())
            .map_err(|e| io_err("pactl", &e))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use super::{AudioSnapshot, SessionError};

    fn ns<T>() -> Result<T, SessionError> {
        Err(SessionError::NotSupported(
            "audio control is not supported on this OS".into(),
        ))
    }
    pub fn poll() -> Result<AudioSnapshot, SessionError> {
        ns()
    }
    pub fn set_volume(_: f64) -> Result<(), SessionError> {
        ns()
    }
    pub fn set_mute(_: bool) -> Result<(), SessionError> {
        ns()
    }
    pub fn set_default_output(_: &str) -> Result<(), SessionError> {
        ns()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpctl_output() {
        assert_eq!(parse_wpctl_volume("Volume: 0.55\n"), Some((55.0, false)));
        assert_eq!(
            parse_wpctl_volume("Volume: 0.30 [MUTED]\n"),
            Some((30.0, true))
        );
        assert_eq!(parse_wpctl_volume("Volume: 1.00"), Some((100.0, false)));
        assert_eq!(parse_wpctl_volume("garbage"), None);
        assert_eq!(parse_wpctl_volume(""), None);
    }

    #[test]
    fn pactl_output() {
        let v = "Volume: front-left: 36044 /  55% / -15.00 dB,   front-right: 36044 /  55% / -15.00 dB\n        balance 0.00\n";
        assert_eq!(parse_pactl_volume(v), Some(55.0));
        assert_eq!(parse_pactl_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_pactl_mute("Mute: no\n"), Some(false));
        assert_eq!(parse_pactl_mute("Mute: ???"), None);
        let sinks = "Sink #52\n\tState: RUNNING\n\tName: alsa_output.pci-0000_0a_00.4.analog-stereo\n\tDescription: Built-in Audio Analog Stereo\n\tDriver: PipeWire\nSink #60\n\tName: bluez_output.AA\n\tDescription: Headset\n";
        assert_eq!(
            parse_pactl_sinks(sinks),
            vec![
                (
                    "alsa_output.pci-0000_0a_00.4.analog-stereo".to_string(),
                    "Built-in Audio Analog Stereo".to_string()
                ),
                ("bluez_output.AA".to_string(), "Headset".to_string()),
            ]
        );
    }

    #[test]
    fn invalid_arguments_are_rejected_before_touching_the_audio_stack() {
        assert!(matches!(
            set_volume(f64::NAN),
            Err(SessionError::Invalid(_))
        ));
        assert!(matches!(
            set_default_output("  "),
            Err(SessionError::Invalid(_))
        ));
    }
}
