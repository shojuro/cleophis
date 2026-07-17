//! Engine smoke test — spawns the actual bundled llama-server with the actual
//! bundled model and asserts one completion. Requires Task 6/7 artifacts.
//! Run (Windows): cargo test --test engine_smoke -- --ignored --nocapture

use std::time::{Duration, Instant};

#[test]
#[ignore]
fn spawn_and_complete() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
    let raw = std::fs::read_to_string(root.join("catalog.json")).unwrap();
    let catalog: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let hero = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["real"].as_bool() == Some(true))
        .expect("no hero in catalog");
    let model = root.join(hero["modelFile"].as_str().unwrap());
    assert!(model.exists(), "model missing: {}", model.display());

    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = std::process::Command::new(root.join("llama").join("llama-server.exe"))
        .args([
            "-m",
            model.to_str().unwrap(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-ngl",
            "99",
            "-c",
            "4096",
            "--no-webui",
        ])
        .spawn()
        .expect("failed to spawn llama-server");

    let deadline = Instant::now() + Duration::from_secs(180);
    let mut up = false;
    while Instant::now() < deadline {
        if let Ok(r) = ureq::get(&format!("http://127.0.0.1:{port}/health")).call() {
            if r.status() == 200 {
                up = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !up {
        let _ = child.kill();
        panic!("llama-server never became healthy");
    }

    let resp: serde_json::Value = ureq::post(&format!(
        "http://127.0.0.1:{port}/v1/chat/completions"
    ))
    .send_json(serde_json::json!({
        "messages": [{"role": "user", "content": "Say hello in five words."}],
        "max_tokens": 32
    }))
    .unwrap()
    .into_json()
    .unwrap();
    let content = resp["choices"][0]["message"]["content"].as_str().unwrap_or("");
    let _ = child.kill();
    assert!(!content.is_empty(), "empty completion: {resp}");
}
