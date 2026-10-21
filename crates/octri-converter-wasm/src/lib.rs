//! No host imports for files, networking, time or telemetry.
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
pub fn execute(request: &str) -> String {
    octri_converter::wire::dispatch(request)
}
