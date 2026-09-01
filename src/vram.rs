//! Определение объёма VRAM системы без внешних зависимостей.
//!
//! Источники (по приоритету):
//! - AMD (Linux): `/sys/class/drm/card*/device/mem_info_vram_total` (килобайты);
//! - NVIDIA: `nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits` (МиБ).
//!
//! Windows/кросс-вендор (Vulkan `ash` / `vulkaninfo` / `lspci`) — осознанно не
//! реализовано до приоритета Windows (см. LOGS.md, этап 10).

/// Суммарный объём VRAM всех GPU системы в байтах (None — не определён).
pub fn vram_total() -> Option<u64> {
    #[cfg(target_os = "linux")]
    if let Some(total) = amdgpu_vram_total() {
        return Some(total);
    }
    nvidia_smi_vram_total()
}

/// AMD (amdgpu): суммирует `mem_info_vram_total` по всем картам (KB → байты).
///
/// Дискретная карта может быть не `card0` (например, iGPU + dGPU), поэтому
/// перебираем все `card*`. Если amdgpu-карт нет (NVIDIA-машина), — None.
#[cfg(target_os = "linux")]
fn amdgpu_vram_total() -> Option<u64> {
    let cards = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut total_kb: u64 = 0;
    let mut found = false;
    for entry in cards.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with("card") {
            continue;
        }
        let path = format!("/sys/class/drm/{name}/device/mem_info_vram_total");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(kb) = text.trim().parse::<u64>() else {
            continue;
        };
        total_kb = total_kb.saturating_add(kb);
        found = true;
    }
    if !found {
        return None;
    }
    Some(total_kb.saturating_mul(1024))
}

/// NVIDIA: суммирует `memory.total` по всем GPU (nvidia-smi, МиБ → байты).
fn nvidia_smi_vram_total() -> Option<u64> {
    let output = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_nvidia_smi(&stdout)
}

/// Разбор вывода `nvidia-smi --query-gpu=memory.total` (число-МиБ на строку)
/// → суммарные байты. Чистая функция, чтобы тестировать без GPU.
pub fn parse_nvidia_smi(text: &str) -> Option<u64> {
    let mut total_mib: u64 = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(mib) = line.parse::<u64>() else {
            continue;
        };
        total_mib = total_mib.saturating_add(mib);
    }
    if total_mib == 0 {
        return None;
    }
    Some(total_mib.saturating_mul(1024).saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn nvidia_smi_single_gpu() {
        // 16384 МиБ = 16 ГиБ.
        assert_eq!(parse_nvidia_smi("16384\n").unwrap(), 16384 * MIB);
    }

    #[test]
    fn nvidia_smi_multi_gpu_sums() {
        // Две карты: 16384 + 8192 МиБ.
        assert_eq!(parse_nvidia_smi("16384\n8192\n").unwrap(), (16384 + 8192) * MIB);
    }

    #[test]
    fn nvidia_smi_empty_is_none() {
        assert!(parse_nvidia_smi("").is_none());
        assert!(parse_nvidia_smi("\n   \n").is_none());
    }

    #[test]
    fn nvidia_smi_skips_garbage_lines() {
        // Заголовки/ошибки пропускаются, число берётся.
        assert_eq!(parse_nvidia_smi("garbage\n16384\n").unwrap(), 16384 * MIB);
    }

    /// Live-проверка на реальном железе: если VRAM есть — определяется.
    #[test]
    #[ignore = "зависит от наличия GPU; VRAM на CI не гарантирован"]
    fn vram_total_detects_hardware() {
        if let Some(total) = vram_total() {
            assert!(total >= 1024 * 1024 * 1024, "VRAM подозрительно мал: {total}");
            println!("VRAM = {total} байт");
        }
    }
}
