//! Browser entry point for the exact native fixture verifier.
#[cfg(target_arch = "wasm32")]
#[path = "replay/support.rs"]
mod support;
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn verify_fixture(bytes: &[u8]) -> Result<String, wasm_bindgen::JsValue> {
    let result = support::verify(bytes).map_err(|e| wasm_bindgen::JsValue::from_str(&e))?;
    serde_json::to_string_pretty(&result)
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))
}
