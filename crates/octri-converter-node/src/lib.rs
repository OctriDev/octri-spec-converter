//! Work runs on libuv's native worker pool, never Node's event loop.
use napi::{Task, bindgen_prelude::*};
use napi_derive::napi;

pub struct ConverterTask {
    request: String,
}
impl Task for ConverterTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> napi::Result<String> {
        Ok(octri_converter::wire::dispatch(&self.request))
    }
    fn resolve(&mut self, _: Env, output: String) -> napi::Result<String> {
        Ok(output)
    }
}
#[napi]
pub fn execute(request: String) -> AsyncTask<ConverterTask> {
    AsyncTask::new(ConverterTask { request })
}
