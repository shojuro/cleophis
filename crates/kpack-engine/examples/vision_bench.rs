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
//!       [--raw-stb]
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
    path: PathBuf,
}

fn run(a: Args) -> Result<(), Box<dyn std::error::Error>> {
    kpack_engine::print_kernel_report();
    let items = a.items()?;
    eprintln!(
        "== vision_bench: family={} model={} mmproj={} items={} max_side={} threads={} n_ctx={} n_batch={} repeat={} ==",
        a.family.name(), a.model, a.mmproj, items.len(), a.max_side, a.threads, a.n_ctx, a.n_batch, a.repeat
    );

    let backend = LlamaBackend::init()?;
    let t = Instant::now();
    let model_params = LlamaModelParams::default().with_n_gpu_layers(0);
    let model = LlamaModel::load_from_file(&backend, &a.model, &model_params)?;
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
            let rec = match run_item(&a, &backend, &model, &mtmd, &template, &prompt, item, rep) {
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
    if total_tokens + a.max_tokens > a.n_ctx as usize {
        return Err(format!(
            "prompt ({total_tokens} tokens, {n_image_tokens} image) + max_tokens ({}) exceeds n_ctx ({})",
            a.max_tokens, a.n_ctx
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
    let mut raw = String::new();
    let mut logprobs: Vec<f32> = Vec::new();
    let mut n_gen = 0usize;
    let mut stop = "eos";
    let gen_start = Instant::now();
    let mut ttft_ms: Option<u128> = None;
    loop {
        let token = sampler.sample(&ctx, -1);
        sampler.accept(token);
        logprobs.push(logprob_of(ctx.get_logits_ith(-1), token.0 as usize, n_vocab));
        if model.is_eog_token(token) {
            break;
        }
        if ttft_ms.is_none() {
            ttft_ms = Some(gen_start.elapsed().as_millis());
        }
        n_gen += 1;
        raw.push_str(&model.token_to_piece(token, &mut decoder, false, None).unwrap_or_default());
        if n_gen >= a.max_tokens {
            stop = "max_tokens";
            break;
        }
        let toks = [token];
        let mut batch = LlamaBatch::get_one(&toks)?;
        ctx.decode(&mut batch)?;
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

fn logprob_of(logits: &[f32], token: usize, n_vocab: usize) -> f32 {
    let n = logits.len().min(n_vocab);
    if token >= n {
        return f32::NEG_INFINITY;
    }
    let max = logits[..n].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let sum: f64 = logits[..n].iter().map(|&l| ((l - max) as f64).exp()).sum();
    (logits[token] - max) as f32 - (sum.ln() as f32)
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
    max_tokens: usize,
    repeat: u32,
    json_out: Option<String>,
    raw_stb: bool,
    time_encode: bool,
}

impl Args {
    fn parse() -> Result<Args, String> {
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
            max_tokens: 256,
            repeat: 1,
            json_out: None,
            raw_stb: false,
            time_encode: true,
        };
        let mut it = std::env::args().skip(1);
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
                "--max-tokens" => a.max_tokens = v()?.parse().map_err(|e| format!("--max-tokens: {e}"))?,
                "--repeat" => a.repeat = v()?.parse().map_err(|e| format!("--repeat: {e}"))?,
                "--json" => a.json_out = Some(v()?),
                "--raw-stb" => a.raw_stb = true,
                "--no-time-encode" => a.time_encode = false,
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

    fn items(&self) -> Result<Vec<Item>, Box<dyn std::error::Error>> {
        let mut out = Vec::new();
        if let Some(p) = &self.items_file {
            let base = p.parent().map(Path::to_path_buf).unwrap_or_default();
            let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
            for it in doc["items"].as_array().ok_or("items.json: no items array")? {
                let id = it["id"].as_str().ok_or("item without id")?.to_string();
                let file = it["file"].as_str().ok_or("item without file")?;
                out.push(Item { id, path: base.join(file) });
            }
        }
        for p in &self.images {
            let id = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
            out.push(Item { id, path: p.clone() });
        }
        Ok(out)
    }
}
