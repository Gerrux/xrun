#![deny(unsafe_code)]

// The xrun_hook wheel is embedded at build time when available.
// When XRUN_KAGGLE_EMBED_WHEEL=1 is set and the wheel is absent, build.rs fails.
// Otherwise (CI / development without a built wheel), we use an empty slice stub.
//
// The wheel path is resolved relative to the workspace root:
// python/xrun_hook/dist/xrun_hook-*.whl
//
// To build the wheel:
//   cd python/xrun_hook && python -m build --wheel

#[cfg(feature = "embed-wheel")]
pub const XRUN_HOOK_WHEEL: &[u8] = include_bytes!(env!("XRUN_HOOK_WHEEL_PATH"));

#[cfg(not(feature = "embed-wheel"))]
pub const XRUN_HOOK_WHEEL: &[u8] = &[];

pub fn wheel_available() -> bool {
    !XRUN_HOOK_WHEEL.is_empty()
}
