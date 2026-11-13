//! vision_bench — Stage 0 of the Vision Input spike (2026-11-01).
//!
//! Loads a small vision-language model (GGUF) plus its multimodal projector
//! (mmproj) through llama.cpp's `mtmd`, runs one transcription prompt per
//! image, and reports — per image, one JSON line — everything the spike's
//! decision rule needs: load time of model and projector, preprocess time,
//! image-token count, encode time, prefill time, time to first token, decode
//! time, tokens/s, the raw output, the per-token log-probabilities (the future
//! confidence signal) and VmRSS/VmHWM. It is a measuring instrument, not the
//! product path: it does not go through `EngineBackend`, has no sha gate, and
//! keeps a fresh context per image (no KV reuse).
//!
//! Usage (the device script wraps this):
//!   vision_bench --model <base.gguf> --mmproj <mmproj.gguf> \
//!       --family docling|smolvlm|qwen \
//!       --items items.json | --image <path> [--image <path> ...] \
//!       [--prompt TEXT] [--max-side 512] [--threads 4] [--n-ctx 2048] \
//!       [--n-batch 1024] [--max-tokens 256] [--repeat 1] [--json out.jsonl] \
//!       [--raw-stb] [--one-pass] [--via-engine] [--multi-formula]
//!
//! `--via-engine` skips the bench's own loop and drives the product path
//! (`LlamaVision` + `assess`) over the same items, to prove the module reproduces
//! the spike's raw outputs.
//!
//! `--items` is the spike's items.json: {"items":[{"id","file",...}]}, paths
//! relative to the file. `--raw-stb` loads the image through mtmd's own stb
//! decoder (no EXIF handling) to show what an orientation-blind path does.

use std::ffi::CString;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::mtmd::{
    mtmd_default_marker, MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputChunkType,
    MtmdInputText,
};
use llama_cpp_2::sampling::LlamaSampler;

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("vision_bench: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(args) {
        eprintln!("vision_bench: FATAL {e}");
        std::process::exit(1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Docling,
    SmolVlm,
    Qwen,
}

/// The prompt the product sends for a formula. Mirrors `prompt` in
/// `src-tauri/resources/catalog.json`; the page prompt in `default_prompt` stays
/// the non-engine bench's default (Stage 0 measured that regime).
const PRODUCT_FORMULA_PROMPT: &str = "Convert formula to latex.";
/// The product's `maxTokens` (catalog.json, every tier).
const PRODUCT_MAX_TOKENS: usize = 96;

impl Family {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "docling" => Ok(Family::Docling),
            "smolvlm" => Ok(Family::SmolVlm),
            "qwen" => Ok(Family::Qwen),
            o => Err(format!("bad --family {o} (docling|smolvlm|qwen)")),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Family::Docling => "docling",
            Family::SmolVlm => "smolvlm",
            Family::Qwen => "qwen",
        }
    }
    /// The instruction the transcriber gets. granite-docling is trained on a
    /// closed set of instructions (its DocTags vocabulary); the generalists get
    /// an explicit "transcribe, do not solve" prompt.
    fn default_prompt(self) -> &'static str {
        match self {
            Family::Docling => "Convert this page to docling.",
            Family::SmolVlm | Family::Qwen => {
                "Transcribe every piece of mathematics in this image as LaTeX. \
                 Output only the LaTeX, one expression per line. Do not solve anything."
            }
        }
    }
}

struct Item {
    id: String,
    /// The `file` value as items.json spells it (relative to the json), for the record.
    file: String,
    path: PathBuf,
}

fn run(a: Args) -> Result<(), Box<dyn std::error::Error>> {
    kpack_engine::print_kernel_report();
    extern "C" {
        fn cleophis_mtmd_idefics3_overview_only(enable: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    if a.one_pass {
        // SAFETY: a plain C function over a std::atomic<int>; no pointers cross.
        unsafe { cleophis_mtmd_idefics3_overview_only(1) };
    }
    if a.via_engine {
        return run_via_engine(&a);
    }
    let items = a.items()?;
    eprintln!(
        "== vision_bench: family={} model={} mmproj={} items={} max_side={} threads={} n_ctx={} n_batch={} repeat={} ==",
        a.family.name(), a.model, a.mmproj, items.len(), a.max_side, a.threads, a.n_ctx, a.n_batch, a.repeat
    );

    // One backend per process: reuse the engine's (print_kernel_report above already initialised it).
    let backend = kpack_engine::shared_backend()?;
    let t = Instant::now();
    let model_params = LlamaModelParams::default().with_n_gpu_layers(0);
    let model = LlamaModel::load_from_file(backend, &a.model, &model_params)?;
    let load_model_ms = t.elapsed().as_millis();
    eprintln!("== model loaded in {load_model_ms} ms; {} ==", rss());

    let t = Instant::now();
    let mtmd_params = MtmdContextParams {
        use_gpu: false,
        print_timings: false,
        n_threads: a.threads,
        media_marker: CString::new(mtmd_default_marker())?,
        ..Default::default()
    };
    let mtmd = MtmdContext::init_from_file(&a.mmproj, &model, &mtmd_params)?;
    let load_mmproj_ms = t.elapsed().as_millis();
    eprintln!(
        "== mmproj loaded in {load_mmproj_ms} ms (includes mtmd's warm-up encode); vision={} mrope={} non_causal={}; {} ==",
        mtmd.support_vision(), mtmd.decode_use_mrope(), mtmd.decode_use_non_causal(), rss()
    );
    if !mtmd.support_vision() {
        return Err("the projector does not support vision input".into());
    }

    let template = model.chat_template(None)?;
    let prompt = a.prompt.clone().unwrap_or_else(|| a.family.default_prompt().to_string());

    let mut out = match &a.json_out {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };

    for rep in 0..a.repeat {
        for item in &items {
            let rec = match run_item(&a, backend, &model, &mtmd, &template, &prompt, item, rep) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[{}] rep {} ERROR {e}", item.id, rep);
                    serde_json::json!({
                        "id": item.id, "rep": rep, "family": a.family.name(), "error": e.to_string(),
                        "load_model_ms": load_model_ms, "load_mmproj_ms": load_mmproj_ms,
                    })
                }
            };
            let mut rec = rec;
            rec["load_model_ms"] = serde_json::json!(load_model_ms);
            rec["load_mmproj_ms"] = serde_json::json!(load_mmproj_ms);
            rec["model"] = serde_json::json!(Path::new(&a.model).file_name().map(|s| s.to_string_lossy().to_string()));
            rec["mmproj"] = serde_json::json!(Path::new(&a.mmproj).file_name().map(|s| s.to_string_lossy().to_string()));
            rec["max_side"] = serde_json::json!(a.max_side);
            rec["threads"] = serde_json::json!(a.threads);
            rec["prompt"] = serde_json::json!(prompt);
            if let Some(f) = out.as_mut() {
                writeln!(f, "{}", rec)?;
                f.flush()?;
            }
        }
    }
    eprintln!("== done; {} ==", rss());
    Ok(())
}

/// `--via-engine`: the product path (`LlamaVision` -> `prepare_image` -> `transcribe` -> `assess`)
/// over the same items, printing the raw text, the verdict and the warnings.
fn run_via_engine(a: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use kpack_engine::{
        assess, prepare_image as prepare_product_image, ModelSpec, ProjectorSpec, StopReason,
        VisionBackend, VisionConfig, VisionLoadRequest,
    };
    let items = a.items()?;
    let (prompt, max_tokens) = a.via_engine_regime();
    eprintln!("[bench] via-engine regime: prompt={prompt:?} max_tokens={max_tokens}");
    let req = VisionLoadRequest {
        base: ModelSpec::new(&a.model),
        mmproj: ProjectorSpec::new(&a.mmproj),
        config: VisionConfig {
            n_threads: a.threads,
            max_tokens,
            one_pass: a.one_pass,
            single_formula: !a.multi_formula,
            prompt,
            ..Default::default()
        },
        n_gpu_layers: 0,
    };
    let mut handle = kpack_engine::LlamaVision::new().load(req)?;
    let mut out = match &a.json_out {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };
    for rep in 0..a.repeat {
        for item in &items {
            let t = Instant::now();
            let mut rec = serde_json::json!({
                "id": item.id, "file": item.file, "rep": rep,
                "verdict": null, "warnings": [], "blocks": [], "raw": null,
                "n_image_tokens": null, "stop": null,
                "prep_ms": null, "prefill_ms": null, "decode_ms": null,
                "total_ms": null, "vmrss_kb": null, "vmhwm_kb": null, "error": null,
            });
            let res: Result<(), Box<dyn std::error::Error>> = (|| {
                let tp = Instant::now();
                let bytes = std::fs::read(&item.path)?;
                let img = prepare_product_image(&bytes, a.max_side)?;
                rec["prep_ms"] = serde_json::json!(tp.elapsed().as_millis() as u64);
                let tr = handle.transcribe(&img, &mut || false)?;
                let assessment = assess(&tr.tokens, tr.stop == StopReason::MaxTokens);
                eprintln!("[{}] img_tok {} stop {:?}", item.id, tr.n_image_tokens, tr.stop);
                eprintln!("    raw: {}", tr.raw.replace('\n', "\\n"));
                eprintln!("    verdict: {:?} warnings {:?}", assessment.verdict, assessment.warnings);
                rec["verdict"] = serde_json::json!(verdict_wire(assessment.verdict));
                rec["warnings"] = serde_json::json!(assessment.warnings.iter().copied().map(warning_wire).collect::<Vec<_>>());
                rec["blocks"] = blocks_wire(&assessment.blocks);
                rec["raw"] = serde_json::json!(tr.raw);
                rec["n_image_tokens"] = serde_json::json!(tr.n_image_tokens);
                rec["stop"] = serde_json::json!(format!("{:?}", tr.stop));
                rec["prefill_ms"] = serde_json::json!(tr.timings.prefill_ms);
                rec["decode_ms"] = serde_json::json!(tr.timings.decode_ms);
                Ok(())
            })();
            if let Err(e) = res {
                eprintln!("[{}] rep {} ERROR {e}", item.id, rep);
                rec["error"] = serde_json::json!(e.to_string());
            }
            let (r, h) = rss_kb();
            rec["total_ms"] = serde_json::json!(t.elapsed().as_millis() as u64);
            rec["vmrss_kb"] = serde_json::json!(r);
            rec["vmhwm_kb"] = serde_json::json!(h);
            if let Some(f) = out.as_mut() {
                writeln!(f, "{}", rec)?;
                f.flush()?;
            }
        }
    }
    Ok(())
}

/// Same shortest-decimal widening as `src-tauri/src/vision_cmds.rs::f32_wire`
/// (that one lives in the Tauri crate, so it is repeated here).
fn f32_wire(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(f64::from(x))
}

fn verdict_wire(v: kpack_engine::Verdict) -> &'static str {
    match v {
        kpack_engine::Verdict::Accept => "accept",
        kpack_engine::Verdict::Review => "review",
        kpack_engine::Verdict::Refuse => "refuse",
    }
}

fn warning_wire(w: kpack_engine::Warning) -> &'static str {
    use kpack_engine::Warning::*;
    match w {
        LowConfidence => "low_confidence",
        OperatorUncertain => "operator_uncertain",
        NoMathFound => "no_math_found",
        InvalidLatex => "invalid_latex",
        Truncated => "truncated",
    }
}

fn blocks_wire(blocks: &[kpack_engine::Block]) -> serde_json::Value {
    blocks
        .iter()
        .map(|b| {
            serde_json::json!({
                "kind": match b.kind {
                    kpack_engine::BlockKind::Latex => "latex",
                    kpack_engine::BlockKind::Text => "text",
                },
                "content": b.content,
                "confidence": f32_wire(b.confidence),
                "min_operator_prob": b.min_operator_prob.map(f32_wire),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kpack_engine::{Block, BlockKind, Verdict, Warning};

    fn args(extra: &[&str]) -> Args {
        let mut v: Vec<String> = ["--model", "m", "--mmproj", "p", "--image", "i.png", "--via-engine"].map(String::from).to_vec();
        v.extend(extra.iter().map(|s| s.to_string()));
        Args::parse_from(v).unwrap()
    }

    #[test]
    fn via_engine_defaults_to_the_product_regime() {
        assert_eq!(args(&[]).via_engine_regime(), ("Convert formula to latex.".to_string(), 96));
        assert_eq!(args(&["--prompt", "X", "--max-tokens", "10"]).via_engine_regime(), ("X".to_string(), 10));
        assert_eq!(args(&[]).bench_max_tokens(), 256);
    }

    #[test]
    fn record_shape_matches_the_scorer() {
        let blocks = vec![
            Block { kind: BlockKind::Latex, content: "3x-7=2x+5".into(), confidence: 0.99, min_operator_prob: Some(0.995), n_tokens: 9 },
            Block { kind: BlockKind::Text, content: "hi".into(), confidence: 0.5, min_operator_prob: None, n_tokens: 1 },
        ];
        let b = blocks_wire(&blocks);
        assert_eq!(b[0]["kind"], "latex");
        assert_eq!(b[0]["confidence"], 0.99);
        assert_eq!(b[0]["min_operator_prob"], 0.995);
        assert!(b[1]["min_operator_prob"].is_null());
        assert!(b[1].as_object().unwrap().contains_key("min_operator_prob"));
        assert_eq!(b[1]["kind"], "text");
        assert_eq!(verdict_wire(Verdict::Review), "review");
        let w: Vec<_> = [Warning::LowConfidence, Warning::OperatorUncertain, Warning::NoMathFound, Warning::InvalidLatex, Warning::Truncated]
            .into_iter().map(warning_wire).collect();
        assert_eq!(w, ["low_confidence", "operator_uncertain", "no_math_found", "invalid_latex", "truncated"]);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_item(
    a: &Args,
    backend: &LlamaBackend,
    model: &LlamaModel,
    mtmd: &MtmdContext,
    template: &llama_cpp_2::model::LlamaChatTemplate,
    prompt: &str,
    item: &Item,
    rep: u32,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    // 1. Image: decode, orient, downscale → RGB8 (the product path's preprocess), or raw stb.
    let t = Instant::now();
    let (bitmap, prep) = if a.raw_stb {
        let bm = MtmdBitmap::from_file(mtmd, item.path.to_str().ok_or("non-utf8 path")?, false)?;
        let (nx, ny) = (bm.nx(), bm.ny());
        (bm, Prep { width: nx, height: ny, orientation: 0, source_sha12: String::new() })
    } else {
        let bytes = std::fs::read(&item.path)?;
        let prepared = prepare_image(&bytes, a.max_side)?;
        let bm = MtmdBitmap::from_image_data(prepared.width, prepared.height, &prepared.rgb)?;
        (bm, Prep { width: prepared.width, height: prepared.height, orientation: prepared.orientation, source_sha12: sha12(&bytes) })
    };
    let prep_ms = t.elapsed().as_millis();

    // 2. Prompt through the GGUF's own chat template, with the media marker in the user turn.
    let user = format!("{}\n{}", mtmd_default_marker(), prompt);
    let msgs = vec![LlamaChatMessage::new("user".into(), user)?];
    let text = model.apply_chat_template(template, &msgs, true)?;
    let chunks = mtmd.tokenize(
        MtmdInputText { text, add_special: true, parse_special: true },
        &[&bitmap],
    )?;
    let n_image_tokens: usize = (0..chunks.len())
        .filter_map(|i| chunks.get(i))
        .filter(|c| c.chunk_type() == MtmdInputChunkType::Image)
        .map(|c| c.n_tokens())
        .sum();
    let total_tokens = chunks.total_tokens();
    if total_tokens + a.bench_max_tokens() > a.n_ctx as usize {
        return Err(format!(
            "prompt ({total_tokens} tokens, {n_image_tokens} image) + max_tokens ({}) exceeds n_ctx ({})",
            a.bench_max_tokens(), a.n_ctx
        ).into());
    }

    // 3. A fresh context per image; batch sizes explicit so llama.cpp's
    //    n_tokens <= n_batch assertion can never fire.
    let n_batch = a.n_batch.max(total_tokens as u32 + 1);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(a.n_ctx))
        .with_n_batch(n_batch)
        .with_n_ubatch(n_batch)
        .with_n_threads(a.threads)
        .with_n_threads_batch(a.threads);
    let mut ctx = model.new_context(backend, ctx_params)?;

    // 4. Encode-only timing (one extra encoder pass, so prefill below double-counts
    //    it; the split is what the spike needs, the cost is only in this bench).
    let mut encode_ms: Option<u128> = None;
    if a.time_encode {
        let t = Instant::now();
        for i in 0..chunks.len() {
            if let Some(c) = chunks.get(i) {
                if c.chunk_type() == MtmdInputChunkType::Image {
                    mtmd.encode_chunk(&c)?;
                }
            }
        }
        encode_ms = Some(t.elapsed().as_millis());
    }

    // 5. Prefill: encode (again) + decode of every chunk.
    let t = Instant::now();
    let n_past = chunks.eval_chunks(mtmd, &ctx, 0, 0, n_batch as i32, true)?;
    let prefill_ms = t.elapsed().as_millis();

    // 6. Greedy generation with per-token log-probabilities.
    let mut sampler = LlamaSampler::greedy();
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let n_vocab = model.n_vocab() as usize;
    let mrope = mtmd.decode_use_mrope();
    let mut raw = String::new();
    // None for the first token (its logits were produced by mtmd's prefill, which the
    // binding's bookkeeping does not see) and for M-RoPE models (positions are
    // inferred by llama.cpp through get_one, which the binding does not track either).
    let mut logprobs: Vec<Option<f32>> = Vec::new();
    let mut n_gen = 0usize;
    let mut stop = "eos";
    let gen_start = Instant::now();
    let mut ttft_ms: Option<u128> = None;
    let mut pos = n_past;
    let mut logits_tracked = false;
    loop {
        let token = sampler.sample(&ctx, -1);
        sampler.accept(token);
        logprobs.push(if logits_tracked { Some(kpack_engine::vision::logprob_of(ctx.get_logits_ith(0), token.0 as usize, n_vocab)) } else { None });
        if model.is_eog_token(token) {
            break;
        }
        if ttft_ms.is_none() {
            ttft_ms = Some(gen_start.elapsed().as_millis());
        }
        n_gen += 1;
        raw.push_str(&model.token_to_piece(token, &mut decoder, false, None).unwrap_or_default());
        if n_gen >= a.bench_max_tokens() {
            stop = "max_tokens";
            break;
        }
        if mrope {
            let toks = [token];
            let mut batch = LlamaBatch::get_one(&toks)?;
            ctx.decode(&mut batch)?;
        } else {
            let mut batch = LlamaBatch::new(1, 1);
            batch.add(token, pos, &[0], true)?;
            ctx.decode(&mut batch)?;
            logits_tracked = true;
        }
        pos += 1;
    }
    let decode_ms = gen_start.elapsed().as_millis();
    let tok_s = if decode_ms > 0 { n_gen as f64 * 1000.0 / decode_ms as f64 } else { 0.0 };
    let total_ms = prep_ms + encode_ms.unwrap_or(0) + prefill_ms + decode_ms;

    let (vmrss_kb, vmhwm_kb) = rss_kb();
    eprintln!(
        "[{}] rep {} img {}x{} orient={} tokens: img={} prompt={} | prep {} ms, encode {:?} ms, prefill {} ms, ttft {:?} ms, decode {} ms ({} tok, {:.1} tok/s) | VmHWM {} kB | {}",
        item.id, rep, prep.width, prep.height, prep.orientation, n_image_tokens, total_tokens, prep_ms, encode_ms, prefill_ms, ttft_ms, decode_ms, n_gen, tok_s, vmhwm_kb, stop
    );
    eprintln!("    raw: {}", raw.replace('\n', "\\n").chars().take(240).collect::<String>());

    Ok(serde_json::json!({
        "id": item.id, "rep": rep, "family": a.family.name(), "file": item.path.file_name().map(|s| s.to_string_lossy().to_string()),
        "source_sha12": prep.source_sha12, "raw_stb": a.raw_stb,
        "width": prep.width, "height": prep.height, "orientation": prep.orientation,
        "n_image_tokens": n_image_tokens, "n_prompt_tokens": total_tokens, "n_past": n_past,
        "prep_ms": prep_ms, "encode_ms": encode_ms, "prefill_ms": prefill_ms, "ttft_ms": ttft_ms,
        "decode_ms": decode_ms, "n_gen": n_gen, "tok_s": tok_s, "total_ms": total_ms, "stop": stop,
        "raw": raw, "token_logprobs": logprobs,
        "vmrss_kb": vmrss_kb, "vmhwm_kb": vmhwm_kb,
    }))
}

struct Prep {
    width: u32,
    height: u32,
    orientation: u32,
    source_sha12: String,
}

struct Prepared {
    width: u32,
    height: u32,
    orientation: u32,
    rgb: Vec<u8>,
}

/// Decode, apply the EXIF orientation (1–8), downscale so the longest edge is
/// `max_side` (0 = keep), return RGB8. This is the product path's preprocess in
/// miniature; the document models' own preprocessor tiles anything larger than
/// 512 px into several encoder passes, so the downscale is the lever that keeps
/// the encode count at two.
fn prepare_image(bytes: &[u8], max_side: u32) -> Result<Prepared, Box<dyn std::error::Error>> {
    let mut img = image::load_from_memory(bytes)?;
    let orientation = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(bytes))
        .ok()
        .and_then(|e| e.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)))
        .unwrap_or(1);
    img = match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    };
    if max_side > 0 {
        let (w, h) = (img.width(), img.height());
        let longest = w.max(h);
        if longest > max_side {
            let scale = max_side as f64 / longest as f64;
            let nw = ((w as f64 * scale).round() as u32).max(1);
            let nh = ((h as f64 * scale).round() as u32).max(1);
            img = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
        }
    }
    let rgb = img.to_rgb8();
    Ok(Prepared { width: rgb.width(), height: rgb.height(), orientation, rgb: rgb.into_raw() })
}

fn sha12(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes)).chars().take(12).collect()
}

fn rss_kb() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let g = |k: &str| -> u64 {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.trim_start_matches(k).trim().split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (g("VmRSS:"), g("VmHWM:"))
}

fn rss() -> String {
    let (r, h) = rss_kb();
    format!("VmRSS={r} kB VmHWM={h} kB")
}

struct Args {
    model: String,
    mmproj: String,
    family: Family,
    items_file: Option<PathBuf>,
    images: Vec<PathBuf>,
    prompt: Option<String>,
    max_side: u32,
    threads: i32,
    n_ctx: u32,
    n_batch: u32,
    /// `None` = not given: the non-engine path uses 256, `--via-engine` the product's 96.
    max_tokens: Option<usize>,
    repeat: u32,
    json_out: Option<String>,
    raw_stb: bool,
    one_pass: bool,
    time_encode: bool,
    via_engine: bool,
    /// `--via-engine` only: `single_formula: false`, so the page regime stays measurable.
    multi_formula: bool,
}

impl Args {
    fn parse() -> Result<Args, String> {
        Args::parse_from(std::env::args().skip(1))
    }

    fn parse_from(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut a = Args {
            model: String::new(),
            mmproj: String::new(),
            family: Family::Docling,
            items_file: None,
            images: Vec::new(),
            prompt: None,
            max_side: 512,
            threads: 4,
            n_ctx: 2048,
            n_batch: 1024,
            max_tokens: None,
            repeat: 1,
            json_out: None,
            raw_stb: false,
            one_pass: false,
            time_encode: false,
            via_engine: false,
            multi_formula: false,
        };
        let mut it = argv.into_iter();
        while let Some(f) = it.next() {
            let mut v = || it.next().ok_or_else(|| format!("{f} needs a value"));
            match f.as_str() {
                "--model" => a.model = v()?,
                "--mmproj" => a.mmproj = v()?,
                "--family" => a.family = Family::parse(&v()?)?,
                "--items" => a.items_file = Some(PathBuf::from(v()?)),
                "--image" => a.images.push(PathBuf::from(v()?)),
                "--prompt" => a.prompt = Some(v()?),
                "--max-side" => a.max_side = v()?.parse().map_err(|e| format!("--max-side: {e}"))?,
                "--threads" => a.threads = v()?.parse().map_err(|e| format!("--threads: {e}"))?,
                "--n-ctx" => a.n_ctx = v()?.parse().map_err(|e| format!("--n-ctx: {e}"))?,
                "--n-batch" => a.n_batch = v()?.parse().map_err(|e| format!("--n-batch: {e}"))?,
                "--max-tokens" => a.max_tokens = Some(v()?.parse().map_err(|e| format!("--max-tokens: {e}"))?),
                "--repeat" => a.repeat = v()?.parse().map_err(|e| format!("--repeat: {e}"))?,
                "--json" => a.json_out = Some(v()?),
                "--raw-stb" => a.raw_stb = true,
                "--one-pass" => a.one_pass = true,
                "--via-engine" => a.via_engine = true,
                "--multi-formula" => a.multi_formula = true,
                "--no-time-encode" => a.time_encode = false,
                "--time-encode" => a.time_encode = true,
                o => return Err(format!("unknown flag {o}")),
            }
        }
        if a.model.is_empty() || a.mmproj.is_empty() {
            return Err("--model and --mmproj are required".into());
        }
        if a.items_file.is_none() && a.images.is_empty() {
            return Err("give --items <items.json> or at least one --image".into());
        }
        Ok(a)
    }

    /// The non-engine bench's cap.
    fn bench_max_tokens(&self) -> usize {
        self.max_tokens.unwrap_or(256)
    }

    /// `--via-engine` runs the PRODUCT regime unless a flag says otherwise.
    fn via_engine_regime(&self) -> (String, usize) {
        (
            self.prompt.clone().unwrap_or_else(|| PRODUCT_FORMULA_PROMPT.to_string()),
            self.max_tokens.unwrap_or(PRODUCT_MAX_TOKENS),
        )
    }

    fn items(&self) -> Result<Vec<Item>, Box<dyn std::error::Error>> {
        let mut out = Vec::new();
        if let Some(p) = &self.items_file {
            let base = p.parent().map(Path::to_path_buf).unwrap_or_default();
            let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
            for it in doc["items"].as_array().ok_or("items.json: no items array")? {
                let id = it["id"].as_str().ok_or("item without id")?.to_string();
                let file = it["file"].as_str().ok_or("item without file")?;
                out.push(Item { id, file: file.to_string(), path: base.join(file) });
            }
        }
        for p in &self.images {
            let id = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
            out.push(Item { id, file: p.to_string_lossy().to_string(), path: p.clone() });
        }
        Ok(out)
    }
}
