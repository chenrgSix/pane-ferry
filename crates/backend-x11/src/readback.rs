//! Pixel work on an already approved drawable. The caller owns this connection
//! and all its render/SHM resources; input never shares them with a readback job.
use crate::*;
use anyhow::ensure;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Surface {
    pub window: u32,
    pub source: (u32, u32),
    pub target: (u32, u32),
    pub visual: Option<u32>,
    pub composite: bool,
}

pub(super) fn capture(
    conn: &RustConnection,
    shared: &mut shared_capture::SharedCapture,
    render: &mut render_scale::RenderScale,
    surface: &Surface,
) -> Result<CapturedImage> {
    let pixmap = if surface.composite {
        let id = conn.generate_id()?;
        conn.composite_name_window_pixmap(surface.window, id)?
            .check()
            .ok()
            .map(|_| id)
    } else {
        None
    };
    let mut size = surface.source;
    let mut visual_override = None;
    let image = (|| -> Result<_> {
        let mut drawable = pixmap.unwrap_or(surface.window);
        // Validate even an unscaled source: a resize between admission and the
        // worker must not silently return a top-left crop of a newer window.
        let geometry = conn.get_geometry(surface.window)?.reply()?;
        ensure!(
            (u32::from(geometry.width), u32::from(geometry.height)) == surface.source,
            "capture drawable changed size"
        );
        if let Some(visual) = surface.visual {
            if let Some((scaled, target_visual)) =
                render.capture_drawable(conn, drawable, visual, size, surface.target)?
            {
                drawable = scaled;
                size = surface.target;
                visual_override = Some(target_visual);
            }
        }
        if let Some(mapped) = render.capture_linear(conn, drawable) {
            return Ok(mapped);
        }
        if let Some(image) = shared.capture(conn, drawable, size.0, size.1) {
            return Ok(image);
        }
        let image = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                drawable,
                0,
                0,
                size.0 as u16,
                size.1 as u16,
                u32::MAX,
            )?
            .reply()?;
        Ok((image.depth, image.visual, image.data))
    })();
    if let Some(pixmap) = pixmap {
        conn.free_pixmap(pixmap)?;
    }
    let (depth, visual_id, data) = image?;
    let visual_id = if let Some(visual) = visual_override {
        visual
    } else if visual_id == 0 {
        conn.get_window_attributes(surface.window)?.reply()?.visual
    } else {
        visual_id
    };
    let setup = conn.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .context("missing pixel format")?;
    let visual = setup
        .roots
        .iter()
        .flat_map(|s| &s.allowed_depths)
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == visual_id)
        .context("missing visual")?;
    Ok(CapturedImage {
        width: size.0,
        height: size.1,
        data,
        bpp: format.bits_per_pixel,
        pad: format.scanline_pad,
        little: setup.image_byte_order == ImageOrder::LSB_FIRST,
        masks: (visual.red_mask, visual.green_mask, visual.blue_mask),
    })
}
