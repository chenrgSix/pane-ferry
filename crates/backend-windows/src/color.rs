//! Shared wire layout; Windows keeps its platform timing wrapper.
use anyhow::Result;
pub use paneferry_protocol::video_color::ColorMode;
pub fn atlas_into(bgra: &[u8], width: u32, height: u32, output: &mut Vec<u8>) -> Result<()> {
    atlas_into_mode(ColorMode::Full, bgra, width, height, output)
}
pub fn atlas_into_mode(
    mode: ColorMode,
    bgra: &[u8],
    width: u32,
    height: u32,
    output: &mut Vec<u8>,
) -> Result<()> {
    let _timing = crate::metrics::span("video.convert_444");
    paneferry_protocol::video_color::atlas_into_mode(mode, bgra, width, height, output)
}
