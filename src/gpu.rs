//! Welches Rechen-Backend Whisper nutzt: eingebaut (beim Build festgelegt), gewählt und tatsächlich gefunden.
//!
//! Die Informationen stammen aus den Logs von whisper.cpp/ggml, die [`LogTap`] mitliest:
//! - ggml zählt Vulkan-Geräte in der Reihenfolge des Treibers auf, auf Laptops oft die integrierte GPU zuerst.
//!   whisper.cpp nähme dann sie statt der viel schnelleren dedizierten GPU (gemessen: RTX 3080 Laptop untätig,
//!   Intel-iGPU rechnet). [`select_gpu`] wählt deshalb die erste dedizierte GPU.
//! - Ohne passende GPU fällt whisper.cpp still auf die CPU zurück; das zeigt nur seine Log-Zeile.
//!
//! Die Geräteliste direkt abzufragen (`whisper_rs::vulkan::list_devices`) kann ohne Vulkan-Treiber eine
//! C++-Ausnahme werfen und den Prozess beenden; die Initialisierung über whisper.cpp fängt das ab.

use std::sync::{Mutex, PoisonError};

/// GPU-Backends, mit denen whisper.cpp gebaut wurde (siehe `Cargo.toml`).
pub fn compiled_backend() -> &'static str {
    match (cfg!(feature = "cuda"), cfg!(target_os = "macos")) {
        (true, true) => "CUDA + Metal",
        (true, false) => "CUDA + Vulkan",
        (false, true) => "Metal",
        (false, false) => "Vulkan",
    }
}

/// Was whisper.cpp beim Start gewählt hat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Device {
    /// ggml-Gerätename (z. B. `Vulkan1`, `CUDA0`, `MTL0`) und, bei Vulkan, der Name der GPU.
    Gpu {
        backend: String,
        name: Option<String>,
    },
    Cpu,
}

/// Vulkan-Gerät, wie ggml es beim Start meldet. Die Position in der Liste ist ggmls Geräteindex.
#[derive(Clone, Debug, PartialEq, Eq)]
struct VulkanDevice {
    name: String,
    /// Integrierte GPU (teilt sich den Arbeitsspeicher mit der CPU).
    integrated: bool,
}

#[derive(Default)]
struct State {
    vulkan: Vec<VulkanDevice>,
    device: Option<Device>,
}

static STATE: Mutex<State> = Mutex::new(State {
    vulkan: Vec::new(),
    device: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Gerät des zuletzt gestarteten Whisper; `None`, solange keins gestartet wurde oder das Log nicht mitgelesen
/// wird.
pub fn detected() -> Option<Device> {
    state().device.clone()
}

/// Index für `WhisperContextParameters::gpu_device`. Unter Vulkan die erste dedizierte GPU, sonst 0.
pub fn select_gpu() -> i32 {
    // CUDA-Geräte stehen bei ggml vor den Vulkan-Geräten; macOS hat nur ein Metal-Gerät.
    if cfg!(feature = "cuda") || cfg!(target_os = "macos") {
        return 0;
    }
    // Initialisiert beim ersten Aufruf die ggml-Backends; dabei meldet ggml die Vulkan-Geräte ins Log.
    let _ = whisper_rs::print_system_info();
    preferred_gpu(&state().vulkan) as i32
}

/// Erste dedizierte GPU, sonst die erste überhaupt (whisper.cpp fällt ohne Gerät selbst auf die CPU zurück).
fn preferred_gpu(devices: &[VulkanDevice]) -> usize {
    devices.iter().position(|d| !d.integrated).unwrap_or(0)
}

/// `ggml_vulkan: 1 = NVIDIA GeForce RTX 3080 Laptop GPU (NVIDIA) | uma: 0 | fp16: 1 | …`
fn parse_vulkan_device(msg: &str) -> Option<(usize, VulkanDevice)> {
    let (index, rest) = msg.strip_prefix("ggml_vulkan: ")?.split_once(" = ")?;
    let index = index.parse().ok()?;
    let mut fields = rest.split(" | ");
    // Name (Treiber): Der Treiber steht in der letzten Klammer.
    let name_driver = fields.next()?;
    let name = name_driver.rsplit_once(" (").map_or(name_driver, |(name, _)| name);
    let integrated = fields.find_map(|f| f.strip_prefix("uma: "))? == "1";
    Some((
        index,
        VulkanDevice {
            name: name.to_string(),
            integrated,
        },
    ))
}

/// Erkennt die Zeilen von `whisper_backend_init_gpu` in whisper.cpp: `…: using Vulkan1 backend` bzw.
/// `…: no GPU found`. Den GPU-Namen ergänzt erst [`observe`].
fn parse_whisper_choice(msg: &str) -> Option<Device> {
    let rest = msg.strip_prefix("whisper_backend_init_gpu: ")?;
    if rest == "no GPU found" {
        return Some(Device::Cpu);
    }
    let backend = rest.strip_prefix("using ")?.strip_suffix(" backend")?;
    Some(Device::Gpu {
        backend: backend.to_string(),
        name: None,
    })
}

/// Ordnet eine Log-Zeile von whisper.cpp/ggml dem Zustand zu.
fn observe(state: &mut State, msg: &str) {
    if let Some((index, device)) = parse_vulkan_device(msg) {
        if index >= state.vulkan.len() {
            state.vulkan.resize(index + 1, device.clone());
        }
        state.vulkan[index] = device;
    } else if let Some(mut device) = parse_whisper_choice(msg) {
        if let Device::Gpu { backend, name } = &mut device {
            *name = backend
                .strip_prefix("Vulkan")
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|i| state.vulkan.get(i))
                .map(|d| d.name.clone());
        }
        state.device = Some(device);
    }
}

/// Zeile für Log und CLI, z. B. `Vulkan (GPU: NVIDIA GeForce RTX 3080 Laptop GPU)` oder
/// `CPU (keine GPU gefunden, gebaut mit Vulkan)`.
pub fn describe(compiled: &str, device: Option<&Device>) -> String {
    match device {
        Some(Device::Gpu { backend, name }) => {
            format!("{compiled} (GPU: {})", name.as_deref().unwrap_or(backend))
        }
        Some(Device::Cpu) => format!("CPU (keine GPU gefunden, gebaut mit {compiled})"),
        None => format!("{compiled}, Gerät unbekannt (ohne GPU rechnet Whisper auf der CPU)"),
    }
}

/// Logger, der die Gerätemeldungen von whisper.cpp/ggml mitliest und alles an `inner` weiterreicht.
pub struct LogTap<L> {
    inner: L,
}

impl<L: log::Log> LogTap<L> {
    pub fn new(inner: L) -> Self {
        Self { inner }
    }
}

/// whisper-rs leitet die Logs von whisper.cpp und ggml unter diesen Targets weiter (Feature `log_backend`).
fn is_whisper_target(target: &str) -> bool {
    target.starts_with("whisper_rs")
}

impl<L: log::Log> log::Log for LogTap<L> {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        is_whisper_target(metadata.target()) || self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if is_whisper_target(record.target()) && record.level() <= log::Level::Debug {
            observe(&mut state(), &record.args().to_string());
        }
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Installiert `env_logger` hinter [`LogTap`]. Die Gerätemeldungen (ggml: Debug) kommen so immer an, auch
/// wenn `RUST_LOG` sie für die Ausgabe ausblendet.
pub fn init_logging(inner: env_logger::Logger) {
    let level = inner.filter().max(log::LevelFilter::Debug);
    if log::set_boxed_logger(Box::new(LogTap::new(inner))).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTEL: &str = "ggml_vulkan: 0 = Intel(R) UHD Graphics (TGL GT1) (Intel open-source Mesa driver) | uma: 1 | \
                         fp16: 1 | bf16: 0 | warp size: 32 | shared memory: 65536 | int dot: 1 | matrix cores: none";
    const NVIDIA: &str = "ggml_vulkan: 1 = NVIDIA GeForce RTX 3080 Laptop GPU (NVIDIA) | uma: 0 | fp16: 1 | \
                          bf16: 0 | warp size: 32 | shared memory: 49152 | int dot: 1 | matrix cores: NV_coopmat2";

    fn device(name: &str, integrated: bool) -> VulkanDevice {
        VulkanDevice {
            name: name.into(),
            integrated,
        }
    }

    #[test]
    fn parses_vulkan_device_lines() {
        assert_eq!(
            parse_vulkan_device(INTEL),
            Some((0, device("Intel(R) UHD Graphics (TGL GT1)", true)))
        );
        assert_eq!(
            parse_vulkan_device(NVIDIA),
            Some((1, device("NVIDIA GeForce RTX 3080 Laptop GPU", false)))
        );
        assert_eq!(parse_vulkan_device("ggml_vulkan: Found 2 Vulkan devices:"), None);
        assert_eq!(parse_vulkan_device("ggml_vulkan: No devices found."), None);
    }

    #[test]
    fn prefers_dedicated_gpu() {
        let intel = device("Intel", true);
        let nvidia = device("NVIDIA", false);
        assert_eq!(preferred_gpu(&[intel.clone(), nvidia.clone()]), 1);
        assert_eq!(preferred_gpu(&[nvidia, intel.clone()]), 0);
        assert_eq!(preferred_gpu(&[intel]), 0);
        assert_eq!(preferred_gpu(&[]), 0);
    }

    #[test]
    fn observes_choice_with_gpu_name() {
        let mut state = State::default();
        for line in [
            NVIDIA,
            INTEL,
            "whisper_backend_init_gpu: found GPU device 1: Vulkan1 (type: 1, cnt: 1)",
            "whisper_backend_init_gpu: using Vulkan1 backend",
        ] {
            observe(&mut state, line);
        }
        assert_eq!(
            state.device,
            Some(Device::Gpu {
                backend: "Vulkan1".into(),
                name: Some("NVIDIA GeForce RTX 3080 Laptop GPU".into())
            })
        );
        observe(&mut state, "whisper_backend_init_gpu: using CUDA0 backend");
        assert_eq!(
            state.device,
            Some(Device::Gpu {
                backend: "CUDA0".into(),
                name: None
            })
        );
        observe(&mut state, "whisper_backend_init_gpu: no GPU found");
        assert_eq!(state.device, Some(Device::Cpu));
        observe(&mut state, "whisper_init_state: kv self size = 10 MB");
        assert_eq!(state.device, Some(Device::Cpu));
    }

    #[test]
    fn describes_backend() {
        let gpu = |name: Option<&str>| Device::Gpu {
            backend: "Vulkan0".into(),
            name: name.map(Into::into),
        };
        assert_eq!(describe("Vulkan", Some(&gpu(Some("RTX")))), "Vulkan (GPU: RTX)");
        assert_eq!(describe("Vulkan", Some(&gpu(None))), "Vulkan (GPU: Vulkan0)");
        assert_eq!(
            describe("Vulkan", Some(&Device::Cpu)),
            "CPU (keine GPU gefunden, gebaut mit Vulkan)"
        );
        assert!(describe("Metal", None).starts_with("Metal, Gerät unbekannt"));
    }

    #[test]
    fn compiled_backend_matches_platform() {
        let backend = compiled_backend();
        if cfg!(target_os = "macos") {
            assert!(backend.ends_with("Metal"));
        } else {
            assert!(backend.ends_with("Vulkan"));
        }
        assert_eq!(backend.starts_with("CUDA"), cfg!(feature = "cuda"));
    }

    /// Zählt weitergereichte Meldungen; lässt nur Warnungen und Schlimmeres durch.
    struct Counting(std::sync::atomic::AtomicUsize);

    impl log::Log for Counting {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }
        fn log(&self, _: &log::Record<'_>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn flush(&self) {}
    }

    #[test]
    fn tap_reads_whisper_logs_but_respects_inner_filter() {
        use log::Log as _;
        let tap = LogTap::new(Counting(0.into()));
        let record = |target, level, msg| {
            tap.log(
                &log::Record::builder()
                    .target(target)
                    .level(level)
                    .args(format_args!("{msg}"))
                    .build(),
            );
        };
        let forwarded = || tap.inner.0.load(std::sync::atomic::Ordering::Relaxed);
        record("tuto_mic", log::Level::Info, "egal");
        record(
            "whisper_rs::ggml_logging_hook",
            log::Level::Debug,
            "ggml_vulkan: 0 = GPU (Treiber) | uma: 0",
        );
        assert_eq!(forwarded(), 0);
        record("tuto_mic", log::Level::Warn, "Warnung");
        assert_eq!(forwarded(), 1);
        // Einziger Test, der den globalen Zustand setzt.
        record(
            "whisper_rs::whisper_logging_hook",
            log::Level::Info,
            "whisper_backend_init_gpu: using Vulkan0 backend",
        );
        assert_eq!(
            detected(),
            Some(Device::Gpu {
                backend: "Vulkan0".into(),
                name: Some("GPU".into())
            })
        );
    }
}
