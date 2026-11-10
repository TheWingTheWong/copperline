//! Wall clock that also works in the browser (std::time::Instant panics on
//! wasm32-unknown-unknown, so the page provides `copperline_now_ms`).

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn copperline_now_ms() -> f64;
}

#[derive(Clone, Copy)]
pub struct Clock(f64);

fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { copperline_now_ms() }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static T0: OnceLock<std::time::Instant> = OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
    }
}

impl Clock {
    pub fn now() -> Clock {
        Clock(now_ms())
    }
    pub fn ms(&self) -> f64 {
        now_ms() - self.0
    }
}
