//! spec-H2 16 KB page-alignment harness. Referencing `LlamaEngine::load` drags
//! the llama.cpp FFI symbols into the linked shared object, so the resulting
//! aarch64 `.so` is a realistic subject for the `readelf -l` LOAD-segment
//! alignment assertion in `check-mobile-build.sh`. Never actually called.

use kpack_engine::{EngineBackend, LlamaEngine, LoadRequest, ModelSpec};

#[no_mangle]
pub extern "C" fn kpack_align_probe() -> i32 {
    let engine = LlamaEngine::new();
    match engine.load(LoadRequest::new(ModelSpec::new("/nonexistent.gguf"), vec![])) {
        Ok(_) => 0,
        Err(_) => 1,
    }
}
