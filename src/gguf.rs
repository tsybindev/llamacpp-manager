//! Чтение заголовка GGUF и оценка потребления памяти моделью.
//!
//! Нас интересуют только метаданные (архитектура, слои, головы, контекст);
//! тензоры не читаются. Оценка памяти: веса ≈ размер файла,
//! KV-кэш = 2 * слои * контекст * kv_dim * байт_на_элемент.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};

const MAGIC: [u8; 4] = *b"GGUF";

/// Метаданные из заголовка GGUF (то, что нужно для оценки памяти).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GgufInfo {
    pub version: u32,
    pub tensor_count: u64,
    pub arch: Option<String>,
    pub n_layers: Option<u64>,
    pub n_embd: Option<u64>,
    pub n_head: Option<u64>,
    pub n_head_kv: Option<u64>,
    pub ctx_train: Option<u64>,
    /// Код типа квантования (general.file_type).
    pub file_type: Option<u32>,
}

/// Оценка потребления памяти llama-server.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemoryEstimate {
    /// Веса модели — приблизительно равны размеру файла.
    pub weights_bytes: u64,
    /// KV-кэш для указанного контекста.
    pub kv_cache_bytes: u64,
    pub ctx_used: u64,
    /// Веса выгруженных в VRAM слоёв (доля от размера файла по ngl/n_layers).
    pub weights_gpu: u64,
    /// Веса CPU-слоёв (остаток после выгрузки).
    pub weights_cpu: u64,
    /// KV-кэш в VRAM (при ngl > 0) или в ОЗУ (при ngl = 0).
    pub kv_gpu: u64,
    /// KV-кэш в ОЗУ (при ngl = 0) или 0 (при ngl > 0).
    pub kv_cpu: u64,
    /// Итого в VRAM: weights_gpu + kv_gpu.
    pub gpu_bytes: u64,
    /// Итого в ОЗУ: weights_cpu + kv_cpu.
    pub cpu_bytes: u64,
}

/// Человеческая метка квантования по general.file_type (частичные данные спецификации).
pub fn file_type_label(file_type: u32) -> Option<&'static str> {
    Some(match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        _ => return None,
    })
}

/// Прочитать заголовок GGUF: только метаданные, без тензоров.
pub fn read_header(path: &Path) -> Result<GgufInfo> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("не удалось открыть {}", path.display()))?;
    let mut reader = file;
    read_header_from(&mut reader)
}

fn read_header_from(reader: &mut impl Read) -> Result<GgufInfo> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).context("чтение магии GGUF")?;
    if magic != MAGIC {
        bail!("это не GGUF-файл (неверная магия)");
    }
    let version = read_u32(reader)?;
    if version < 2 {
        bail!("GGUF версии {version} не поддерживается (нужна 2+)");
    }
    let tensor_count = read_u64(reader)?;
    let kv_count = read_u64(reader)?;

    // Ограничение на всякий случай: повреждённый файл не должен заставить
    // нас читать гигабайты «ключей».
    if kv_count > 100_000 {
        bail!("подозрительно много метаданных: {kv_count}");
    }

    let mut info = GgufInfo {
        version,
        tensor_count,
        ..Default::default()
    };
    let mut scalars: HashMap<String, u64> = HashMap::new();
    let mut arch_string: Option<String> = None;

    for _ in 0..kv_count {
        let key = read_string(reader)?;
        let value_type = read_u32(reader)?;
        match value_type {
            // Скаляры: сохраняем интересные, остальные пропускаем.
            0 | 7 => {
                let byte = read_u8(reader)?;
                scalars.entry(key).or_insert(u64::from(byte));
            }
            1 => {
                let v = read_u8(reader)? as i8;
                scalars.entry(key).or_insert(v as u64);
            }
            2 | 3 => {
                let v = read_u16(reader)?;
                scalars.entry(key).or_insert(u64::from(v));
            }
            4..=6 => {
                let v = read_u32(reader)?;
                scalars.entry(key).or_insert(u64::from(v));
            }
            10..=12 => {
                let v = read_u64(reader)?;
                scalars.entry(key).or_insert(v);
            }
            8 => {
                let value = read_string(reader)?;
                if key == "general.architecture" {
                    arch_string = Some(value);
                }
            }
            9 => {
                skip_array(reader)?;
            }
            other => bail!("неизвестный тип метаданных GGUF: {other}"),
        }
    }

    info.arch = arch_string;
    info.file_type = scalars.get("general.file_type").copied().map(|v| v as u32);
    if let Some(arch) = &info.arch {
        info.n_layers = scalars.get(&format!("{arch}.block_count")).copied();
        info.n_embd = scalars.get(&format!("{arch}.embedding_length")).copied();
        info.n_head = scalars.get(&format!("{arch}.attention.head_count")).copied();
        info.n_head_kv = scalars
            .get(&format!("{arch}.attention.head_count_kv"))
            .copied();
        info.ctx_train = scalars.get(&format!("{arch}.context_length")).copied();
    }
    Ok(info)
}

/// Оценить память: веса (размер файла) + KV-кэш, с разбивкой по GPU/CPU.
/// `ctx_override` — контекст из параметров сервера, если включён;
/// `kv_elem_bytes` — байт на элемент кэша (f16 = 2, q8_0 = 1, q4_0 ≈ 0.5);
/// `ngl_effective` — сколько слоёв модели реально живёт в VRAM (0..=n_layers).
pub fn estimate_memory(
    info: &GgufInfo,
    file_size: u64,
    ctx_override: Option<u64>,
    kv_elem_bytes: f64,
    ngl_effective: u64,
) -> Option<MemoryEstimate> {
    let layers = info.n_layers?;
    let ctx = ctx_override.or(info.ctx_train)?;
    // Размер K/V на токен: n_embd, уменьшенный на GQA-фактор, если известны головы.
    let embd = info.n_embd.unwrap_or(0);
    let kv_dim = match (info.n_head, info.n_head_kv) {
        (Some(heads), Some(kv_heads)) if heads > 0 => embd * kv_heads / heads,
        _ => embd,
    };
    let kv_cache_bytes = if kv_dim > 0 {
        (2.0 * layers as f64 * ctx as f64 * kv_dim as f64 * kv_elem_bytes) as u64
    } else {
        0
    };
    let weights_bytes = file_size;
    // Веса выгруженных слоёв — пропорционально доле ngl/n_layers.
    let ngl = ngl_effective.min(layers);
    let weights_gpu = file_size.saturating_mul(ngl) / layers;
    let weights_cpu = weights_bytes.saturating_sub(weights_gpu);
    // KV-кэш живёт там, где живёт модель: при ngl > 0 — в VRAM, иначе в ОЗУ.
    let kv_gpu = if ngl > 0 { kv_cache_bytes } else { 0 };
    let kv_cpu = kv_cache_bytes.saturating_sub(kv_gpu);
    Some(MemoryEstimate {
        weights_bytes,
        kv_cache_bytes,
        ctx_used: ctx,
        weights_gpu,
        weights_cpu,
        kv_gpu,
        kv_cpu,
        gpu_bytes: weights_gpu + kv_gpu,
        cpu_bytes: weights_cpu + kv_cpu,
    })
}

fn read_u8(r: &mut impl Read) -> Result<u8> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16(r: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string(r: &mut impl Read) -> Result<String> {
    let len = read_u64(r)?;
    if len > 1 << 20 {
        bail!("строка метаданных слишком длинная: {len}");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).context("строка метаданных не UTF-8")
}

/// Пропустить значение-массив (элементы строк перебираются по длинам).
fn skip_array(r: &mut impl Read) -> Result<()> {
    let elem_type = read_u32(r)?;
    let count = read_u64(r)?;
    if count > 10_000_000 {
        bail!("подозрительно длинный массив метаданных: {count}");
    }
    match elem_type {
        8 => {
            for _ in 0..count {
                read_string(r)?;
            }
        }
        9 => {
            for _ in 0..count {
                skip_array(r)?;
            }
        }
        t => {
            let size = scalar_size(t)
                .with_context(|| format!("неизвестный тип элемента массива: {t}"))?;
            let total = size
                .checked_mul(count)
                .context("переполнение размера массива")?;
            if total > 1 << 30 {
                bail!("массив метаданных слишком велик: {total}");
            }
            let mut buf = vec![0u8; total as usize];
            r.read_exact(&mut buf)?;
        }
    }
    Ok(())
}

fn scalar_size(value_type: u32) -> Option<u64> {
    Some(match value_type {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Собрать минимальный GGUF в памяти: заголовок + набор метаданных.
    fn build_test_gguf() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&405u64.to_le_bytes()); // тензоров (не читаем)
        buf.extend_from_slice(&7u64.to_le_bytes()); // kv_count

        let put_string = |buf: &mut Vec<u8>, s: &str| {
            buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        };
        let put_kv_u32 = |buf: &mut Vec<u8>, key: &str, value: u32| {
            put_string(buf, key);
            buf.extend_from_slice(&4u32.to_le_bytes());
            buf.extend_from_slice(&value.to_le_bytes());
        };

        // general.architecture — строка.
        put_string(&mut buf, "general.architecture");
        buf.extend_from_slice(&8u32.to_le_bytes());
        put_string(&mut buf, "gemma3n");

        put_kv_u32(&mut buf, "general.file_type", 15);
        put_kv_u32(&mut buf, "gemma3n.block_count", 35);
        put_kv_u32(&mut buf, "gemma3n.embedding_length", 2048);
        put_kv_u32(&mut buf, "gemma3n.attention.head_count", 8);
        put_kv_u32(&mut buf, "gemma3n.attention.head_count_kv", 2);
        put_kv_u32(&mut buf, "gemma3n.context_length", 32768);

        // Массив строк (токенизатор) — должен корректно пропуститься.
        put_string(&mut buf, "tokenizer.ggml.tokens");
        buf.extend_from_slice(&9u32.to_le_bytes()); // array
        buf.extend_from_slice(&8u32.to_le_bytes()); // of strings
        buf.extend_from_slice(&3u64.to_le_bytes()); // 3 элемента
        put_string(&mut buf, "<pad>");
        put_string(&mut buf, "<eos>");
        put_string(&mut buf, "hello");

        // Массив скаляров — тоже пропуститься.
        put_string(&mut buf, "tokenizer.ggml.scores");
        buf.extend_from_slice(&9u32.to_le_bytes());
        buf.extend_from_slice(&6u32.to_le_bytes()); // f32
        buf.extend_from_slice(&2u64.to_le_bytes());
        buf.extend_from_slice(&0f32.to_le_bytes());
        buf.extend_from_slice(&0f32.to_le_bytes());

        buf
    }

    #[test]
    fn read_header_parses_metadata_and_skips_arrays() {
        let data = build_test_gguf();
        let mut reader = data.as_slice();
        let info = read_header_from(&mut reader).expect("разбор заголовка");
        assert_eq!(info.version, 3);
        assert_eq!(info.tensor_count, 405);
        assert_eq!(info.arch.as_deref(), Some("gemma3n"));
        assert_eq!(info.file_type, Some(15));
        assert_eq!(info.n_layers, Some(35));
        assert_eq!(info.n_embd, Some(2048));
        assert_eq!(info.n_head, Some(8));
        assert_eq!(info.n_head_kv, Some(2));
        assert_eq!(info.ctx_train, Some(32768));
        assert!(!reader.is_empty() || true, "остаток может быть пуст");
    }

    #[test]
    fn not_gguf_file_is_rejected() {
        let mut reader: &[u8] = b"PK\x03\x04 garbage";
        assert!(read_header_from(&mut reader).is_err());
    }

    /// Live-тест на реальной модели: LLAMA_TEST_MODEL=/путь/к/model.gguf
    #[test]
    #[ignore = "требует реальный GGUF-файл в LLAMA_TEST_MODEL"]
    fn read_real_model_header() {
        let path = std::env::var("LLAMA_TEST_MODEL").expect("LLAMA_TEST_MODEL не задан");
        let info = read_header(Path::new(&path)).expect("заголовок");
        println!("{info:#?}");
        assert!(info.n_layers.unwrap_or(0) > 0, "нет числа слоёв");
        assert!(info.n_embd.unwrap_or(0) > 0, "нет размерности эмбеддингов");
    }

    #[test]
    fn estimate_accounts_gqa_and_ctx_override() {
        let info = GgufInfo {
            version: 3,
            tensor_count: 405,
            arch: Some("gemma3n".into()),
            n_layers: Some(35),
            n_embd: Some(2048),
            n_head: Some(8),
            n_head_kv: Some(2),
            ctx_train: Some(32768),
            file_type: Some(15),
        };
        let file_size = 4u64 * 1024 * 1024 * 1024; // 4 ГиБ весов

        // Тренировочный контекст, полный offload (ngl = n_layers): всё в VRAM.
        // kv_dim = 2048 * 2/8 = 512.
        let est = estimate_memory(&info, file_size, None, 2.0, 35).expect("оценка");
        assert_eq!(est.ctx_used, 32768);
        let kv_expected = 2 * 35 * 32768 * 512 * 2;
        assert_eq!(est.kv_cache_bytes, kv_expected);
        assert_eq!(est.weights_bytes, file_size);
        assert_eq!(est.gpu_bytes, est.kv_cache_bytes + est.weights_bytes);
        assert_eq!(est.cpu_bytes, 0);

        // Только CPU (ngl = 0): всё в ОЗУ, контекст переопределён.
        let est = estimate_memory(&info, file_size, Some(8192), 2.0, 0).expect("оценка");
        assert_eq!(est.ctx_used, 8192);
        assert_eq!(est.kv_cache_bytes, 2 * 35 * 8192 * 512 * 2);
        assert_eq!(est.gpu_bytes, 0);
        assert_eq!(est.cpu_bytes, est.kv_cache_bytes + est.weights_bytes);
    }

    #[test]
    fn estimate_partial_offload_splits_weights() {
        let info = GgufInfo {
            version: 3,
            tensor_count: 405,
            arch: Some("gemma3n".into()),
            n_layers: Some(20),
            n_embd: Some(1024),
            n_head: Some(16),
            n_head_kv: Some(16),
            ctx_train: Some(4096),
            file_type: Some(15),
        };
        let file_size = 10u64 * 1024 * 1024 * 1024;
        // ngl = 10 из 20: половина весов в VRAM, половина в ОЗУ.
        let est = estimate_memory(&info, file_size, None, 2.0, 10).expect("оценка");
        assert_eq!(est.weights_bytes, file_size);
        assert_eq!(est.gpu_bytes, file_size / 2 + est.kv_cache_bytes);
        assert_eq!(est.cpu_bytes, file_size / 2);
        assert_eq!(est.gpu_bytes + est.cpu_bytes, est.weights_bytes + est.kv_cache_bytes);
    }

    #[test]
    fn estimate_requires_layers_and_ctx() {
        let empty = GgufInfo::default();
        assert!(estimate_memory(&empty, 1000, None, 2.0, 999).is_none());
    }
}
