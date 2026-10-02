//! The acoustic model: token ids, a style vector and a speed in, 24 kHz
//! mono float audio out. `OrtModel` runs `kokoro-v1.0.onnx` on Microsoft's
//! official CPU ONNX Runtime (`onnxruntime.dll`, MIT), loaded at run time
//! from a full path (`ort` feature `load-dynamic`): `sonarad.exe` starts
//! without it, and Windows' own `System32\onnxruntime.dll` is never picked
//! up by mistake.
use crate::{Error, Result};
use std::path::Path;
use std::sync::OnceLock;

/// Kokoro's output rate.
pub const SAMPLE_RATE: u32 = 24_000;

/// Something that turns tokens into audio. Tests use a fake.
pub trait Acoustic: Send {
    /// `tokens` without the pad tokens (at most `MAX_TOKENS`), `style` of
    /// `STYLE_DIM` values, `speed` 0.5..=2.0.
    fn infer(&mut self, tokens: &[i64], style: &[f32], speed: f32) -> Result<Vec<f32>>;
}

/// Load ONNX Runtime from `dylib` once per process. Later calls return the
/// first result (ONNX Runtime cannot be unloaded and loaded again).
pub fn init_runtime(dylib: &Path) -> std::result::Result<(), String> {
    static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        if !dylib.is_file() {
            return Err(format!(
                "ONNX Runtime is not installed ({} is missing)",
                dylib.display()
            ));
        }
        let path = dylib.to_path_buf();
        std::panic::catch_unwind(move || {
            ort::init_from(&path)
                .map(|b| {
                    b.with_name("sonara").commit();
                })
                .map_err(|e| format!("cannot load ONNX Runtime: {e}"))
        })
        .unwrap_or_else(|_| Err("cannot load ONNX Runtime (it panicked)".into()))
    })
    .clone()
}

/// `kokoro-v1.0.onnx` in an ONNX Runtime session.
pub struct OrtModel {
    session: ort::session::Session,
}

impl OrtModel {
    /// Load the model (`init_runtime` first). Measured on the M0 corpus
    /// (M4 notes in `docs/plans/2026-10-02-m0-engine-spike.md`): the CPU
    /// arena allocator halves synthesis time (RTF 0.10 instead of 0.21)
    /// for about 30 MB more; memory patterns only add memory (about 50 MB),
    /// so they are off. Peak working set about 700 MB with sentence batches.
    pub fn load(model: &Path) -> Result<OrtModel> {
        let fail = |e: String| Error::Engine(format!("cannot load the Kokoro model: {e}"));
        let model = model.to_path_buf();
        std::panic::catch_unwind(move || -> Result<OrtModel> {
            let session = ort::session::Session::builder()
                .map_err(|e| fail(e.to_string()))?
                .with_execution_providers([ort::ep::CPU::default()
                    .with_arena_allocator(true)
                    .build()])
                .map_err(|e| fail(e.to_string()))?
                .with_memory_pattern(false)
                .map_err(|e| fail(e.to_string()))?
                .commit_from_file(&model)
                .map_err(|e| fail(e.to_string()))?;
            Ok(OrtModel { session })
        })
        .unwrap_or_else(|_| Err(fail("ONNX Runtime panicked".into())))
    }
}

impl Acoustic for OrtModel {
    fn infer(&mut self, tokens: &[i64], style: &[f32], speed: f32) -> Result<Vec<f32>> {
        use ort::value::Tensor;
        let fail = |e: String| Error::Engine(format!("Kokoro inference failed: {e}"));
        let n = tokens.len();
        let mut padded = Vec::with_capacity(n + 2);
        padded.push(0);
        padded.extend_from_slice(tokens);
        padded.push(0);
        let tokens =
            Tensor::from_array(([1usize, n + 2], padded)).map_err(|e| fail(e.to_string()))?;
        let style = Tensor::from_array(([1usize, style.len()], style.to_vec()))
            .map_err(|e| fail(e.to_string()))?;
        let speed = Tensor::from_array(([1usize], vec![speed])).map_err(|e| fail(e.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![
                "tokens" => tokens,
                "style" => style,
                "speed" => speed,
            ])
            .map_err(|e| fail(e.to_string()))?;
        let (_, audio) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(|e| fail(e.to_string()))?;
        Ok(audio.to_vec())
    }
}
