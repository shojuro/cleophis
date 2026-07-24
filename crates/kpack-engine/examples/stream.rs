//! `stream` — the minimal P0 on-device benchmark/probe harness.
//!
//! Loads a base model + composed adapter stack through the real
//! [`LlamaEngine`], streams a completion to stdout, and prints tokens/sec and
//! peak RSS. It is the gate artifact (spec §0): pushable to a phone and run in
//! `adb shell` to produce the numbers (tokens/sec, total RSS) and the Stage-5
//! behavioral probes — with the full behavioral→contract→voice stack mounted
//! via in-process FFI — WITHOUT needing the Tauri app or any UI.
//!
//! Build (aarch64):
//!   cargo ndk -t arm64-v8a -P 24 build --features real --example stream
//! Run on device (after `adb push` the binary + GGUFs):
//!   ./stream hero-1b-q4.gguf \
//!     --behavioral beh.gguf --contract con.gguf --voice voice.gguf \
//!     --template llama3 --temp 0 --prompt "Who was the first person on Mars?"
//!
//! `--temp 0` (greedy, the default) is what the probes use — deterministic.

use std::io::Write;
use std::ops::ControlFlow;
use std::time::Instant;

use kpack_engine::{
    AdapterRole, AdapterSpec, ChatMessage, ChatTemplate, EngineBackend, LlamaEngine, LoadParams,
    LoadRequest, ModelSpec, Sampling, SessionConfig, StopReason,
};

fn main() {
    let cfg = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("stream: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if let Err(e) = run(cfg) {
        eprintln!("stream: error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut adapters = Vec::new();
    if let Some(p) = &args.behavioral {
        adapters.push(AdapterSpec::new(AdapterRole::Behavioral, p));
    }
    if let Some(p) = &args.contract {
        adapters.push(AdapterSpec::new(AdapterRole::Contract, p));
    }
    if let Some(p) = &args.voice {
        adapters.push(AdapterSpec::new(AdapterRole::Voice, p));
    }

    let req = LoadRequest::new(ModelSpec::new(&args.model), adapters)
        .with_template(args.template)
        .with_params(LoadParams { n_gpu_layers: args.gpu_layers });

    eprintln!(
        "loading base + {} adapter(s) [{}] ...",
        req.adapters.len(),
        req.adapters
            .iter()
            .map(|a| a.role.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let load_start = Instant::now();
    let engine = LlamaEngine::new();
    let mut handle = engine.load(req)?;
    eprintln!(
        "loaded in {:.2}s; mounted stack: {:?}",
        load_start.elapsed().as_secs_f64(),
        handle.mounted_adapters()
    );
    eprintln!("RSS after load: {}", rss_line());

    let mut session = handle.session(SessionConfig {
        n_ctx: args.n_ctx,
        sampling: Sampling {
            temperature: args.temp,
            max_tokens: args.max_tokens,
            ..Sampling::default()
        },
    })?;

    let mut messages = Vec::new();
    if let Some(sys) = &args.system {
        messages.push(ChatMessage::system(sys.clone()));
    }
    messages.push(ChatMessage::user(args.prompt.clone()));

    eprintln!("--- prompt ---\n{}\n--- response ---", args.prompt);

    let mut n_visible = 0usize;
    let mut first_tok_at: Option<Instant> = None;
    let gen_start = Instant::now();
    let mut out = std::io::stdout();
    let stats = session.stream(&messages, &mut |text: &str| {
        if first_tok_at.is_none() {
            first_tok_at = Some(Instant::now());
        }
        n_visible += 1;
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
        ControlFlow::Continue(())
    })?;
    let elapsed = gen_start.elapsed().as_secs_f64();
    println!();

    let ttft = first_tok_at
        .map(|t| t.duration_since(gen_start).as_secs_f64())
        .unwrap_or(0.0);
    let tok_per_s = if elapsed > 0.0 {
        stats.generated_tokens as f64 / elapsed
    } else {
        0.0
    };

    eprintln!("--- stats ---");
    eprintln!("prompt tokens:     {}", stats.prompt_tokens);
    eprintln!("generated tokens:  {}", stats.generated_tokens);
    eprintln!("visible sink calls:{n_visible}");
    eprintln!("stop reason:       {}", stop_str(stats.stop));
    eprintln!("time to first tok: {ttft:.3}s");
    eprintln!("generation time:   {elapsed:.3}s");
    eprintln!("tokens/sec:        {tok_per_s:.2}");
    eprintln!("RSS after gen:     {}", rss_line());
    Ok(())
}

fn stop_str(s: StopReason) -> &'static str {
    match s {
        StopReason::Eos => "eos",
        StopReason::MaxTokens => "max_tokens",
        StopReason::Cancelled => "cancelled",
    }
}

/// Current + peak RSS from `/proc/self/status` (works on Android). The P0
/// budget is total app RSS (spec §2); for the harness that is the process RSS.
fn rss_line() -> String {
    match std::fs::read_to_string("/proc/self/status") {
        Ok(s) => {
            let get = |k: &str| {
                s.lines()
                    .find(|l| l.starts_with(k))
                    .map(|l| l.trim_start_matches(k).trim().to_string())
                    .unwrap_or_else(|| "?".into())
            };
            format!("VmRSS={} VmHWM(peak)={}", get("VmRSS:"), get("VmHWM:"))
        }
        Err(_) => "unavailable".into(),
    }
}

const USAGE: &str = "usage: stream <model.gguf> [--behavioral P] [--contract P] [--voice P]\n         [--prompt TEXT] [--system TEXT] [--template auto|llama3|chatml]\n         [--temp F] [--n-ctx N] [--max-tokens N] [--gpu-layers N]";

struct Args {
    model: String,
    behavioral: Option<String>,
    contract: Option<String>,
    voice: Option<String>,
    prompt: String,
    system: Option<String>,
    template: ChatTemplate,
    temp: f32,
    n_ctx: u32,
    max_tokens: usize,
    gpu_layers: u32,
}

impl Args {
    fn parse() -> Result<Args, String> {
        let mut it = std::env::args().skip(1);
        let model = it.next().ok_or("missing <model.gguf>")?;
        if model.starts_with("--") {
            return Err("first argument must be the model path".into());
        }
        let mut a = Args {
            model,
            behavioral: None,
            contract: None,
            voice: None,
            prompt: "Say hello in one short sentence.".into(),
            system: None,
            template: ChatTemplate::Auto,
            temp: 0.0,
            n_ctx: 2048,
            max_tokens: 256,
            gpu_layers: 0,
        };
        while let Some(flag) = it.next() {
            let mut val = || it.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--behavioral" => a.behavioral = Some(val()?),
                "--contract" => a.contract = Some(val()?),
                "--voice" => a.voice = Some(val()?),
                "--prompt" => a.prompt = val()?,
                "--system" => a.system = Some(val()?),
                "--template" => {
                    a.template = match val()?.as_str() {
                        "auto" => ChatTemplate::Auto,
                        "llama3" => ChatTemplate::Llama3,
                        "chatml" => ChatTemplate::ChatMl,
                        other => return Err(format!("unknown template: {other}")),
                    }
                }
                "--temp" => a.temp = val()?.parse().map_err(|_| "bad --temp")?,
                "--n-ctx" => a.n_ctx = val()?.parse().map_err(|_| "bad --n-ctx")?,
                "--max-tokens" => a.max_tokens = val()?.parse().map_err(|_| "bad --max-tokens")?,
                "--gpu-layers" => a.gpu_layers = val()?.parse().map_err(|_| "bad --gpu-layers")?,
                other => return Err(format!("unknown flag: {other}")),
            }
        }
        Ok(a)
    }
}
