#[cfg(target_arch = "wasm32")]
#[path = "schema/support.rs"]
mod support;
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn schema_fixture() -> Result<String, wasm_bindgen::JsValue> {
    support::fixture().map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))
}
