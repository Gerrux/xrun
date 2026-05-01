#![deny(unsafe_code)]

// The xrun_hook wheel is embedded at build time when build.rs locates one
// under python/xrun_hook/dist/. The build script flips
// `cfg(xrun_hook_wheel_embedded)` and points XRUN_HOOK_WHEEL_PATH at the
// resolved wheel. When the wheel is absent (clean clone, no Python), we fall
// through to an empty stub and the Kaggle adapter logs a runtime warning
// instead of failing the build.
//
// To force a hard build error when the wheel is missing (release CI):
//   XRUN_KAGGLE_EMBED_WHEEL=strict cargo build
// To opt into building the wheel from source as part of `cargo build`:
//   XRUN_KAGGLE_AUTO_BUILD_WHEEL=1 cargo build

#[cfg(xrun_hook_wheel_embedded)]
pub const XRUN_HOOK_WHEEL: &[u8] = include_bytes!(env!("XRUN_HOOK_WHEEL_PATH"));

#[cfg(not(xrun_hook_wheel_embedded))]
pub const XRUN_HOOK_WHEEL: &[u8] = &[];

pub fn wheel_available() -> bool {
    !XRUN_HOOK_WHEEL.is_empty()
}
