//! The `calc` Tauri command — a thin wrapper over the pure `kpack-calc`
//! evaluator. All math + all tests live in the crate; this only adapts the
//! result shape for the FE tool loop and turns a CalcError into an Err
//! STRING, which the FE hands back to the model AS THE TOOL RESULT so the
//! model can self-correct (fix the expression / explain the limit).

use serde::Serialize;

// Debug is needed for `.unwrap_err()` in the error_is_a_plain_message test
// (Result::unwrap_err requires the Ok side to implement Debug) — the brief's
// snippet omitted it; added here rather than changing the test.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CalcResult {
    pub expression: String,
    pub result: f64,
    pub display: String,
}

#[tauri::command]
pub fn calc(expression: String) -> Result<CalcResult, String> {
    let display = kpack_calc::evaluate_display(&expression).map_err(|e| e.to_string())?;
    let result = kpack_calc::evaluate(&expression).map_err(|e| e.to_string())?;
    Ok(CalcResult { expression, result, display })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ok_shape() {
        let r = calc("(3/4)*88".into()).unwrap();
        assert_eq!(r.display, "66");
        assert_eq!(r.result, 66.0);
    }
    #[test]
    fn error_is_a_plain_message() {
        let e = calc("sqrt(-1)".into()).unwrap_err();
        assert!(e.contains("negative"), "got: {e}");
    }
}
