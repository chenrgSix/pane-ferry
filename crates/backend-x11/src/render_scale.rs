//! Optional server-side reduction before an immutable MIT-SHM snapshot.
//! Only the approved drawable is sampled; all destinations are private pixmaps.
use crate::*;
use anyhow::ensure;
use x11rb::protocol::render::{self, ConnectionExt as _};

struct Canvas {
    pixmap: u32,
    picture: u32,
    size: (u32, u32),
    linear: Option<crate::linear_capture::Buffer>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an explicitly selected local GPU X11 display with DRI3/GBM; only private pixmaps are drawn"]
    fn linear_snapshots_match_server_pixels_before_any_reference_readback() -> Result<()> {
        let (conn, screen) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen];
        let mut scaler = RenderScale::new(&conn, screen.root);
        anyhow::ensure!(
            scaler.linear_enabled,
            "test display lacks linear DRI3/GBM capture"
        );
        let source = conn.generate_id()?;
        conn.create_pixmap(screen.root_depth, source, screen.root, 258, 194)?
            .check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, source, &CreateGCAux::new())?.check()?;
        let mut retained = Vec::new();
        for pass in 0..100u32 {
            let color = ((pass * 7919) ^ 0x123456) & 0xffffff;
            conn.change_gc(gc, &ChangeGCAux::new().foreground(color))?;
            conn.poly_fill_rectangle(
                source,
                gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 258,
                    height: 194,
                }],
            )?;
            let target = if pass < 50 { (129, 97) } else { (127, 95) };
            let (pixmap, _) = scaler
                .capture_drawable(&conn, source, screen.root_visual, (258, 194), target)?
                .context("linear test target missing")?;
            // Read the mapped path FIRST: GetImage would otherwise hide a
            // missing GPU submission/fence by synchronizing on our behalf.
            let (_, _, snapshot) = scaler
                .capture_linear(&conn, pixmap)
                .context("linear readback fell back")?;
            assert_eq!(
                &snapshot[..3],
                &[
                    (color & 255) as u8,
                    ((color >> 8) & 255) as u8,
                    ((color >> 16) & 255) as u8
                ]
            );
            let reference = conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    pixmap,
                    0,
                    0,
                    target.0 as u16,
                    target.1 as u16,
                    u32::MAX,
                )?
                .reply()?;
            assert_eq!(snapshot.len(), target.0 as usize * target.1 as usize * 4);
            assert!(snapshot
                .chunks_exact(4)
                .zip(reference.data.chunks_exact(4))
                .all(|(a, b)| a[..3] == b[..3]));
            if pass == 0 {
                retained = snapshot;
            }
        }
        assert_eq!(
            &retained[..3],
            &[0x56, 0x34, 0x12],
            "later GPU writes cannot change an owned snapshot"
        );
        let mut reference_scale = RenderScale::new(&conn, screen.root);
        reference_scale.linear_enabled = false;
        reference_scale.linear_probed = true;
        let mut detail = Vec::new();
        for y in 0..194u32 {
            for x in 0..258u32 {
                detail.extend_from_slice(&[
                    ((x % 17) * 15) as u8,
                    ((y % 19) * 13) as u8,
                    (((x ^ y) % 23) * 10) as u8,
                    0,
                ]);
            }
        }
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            source,
            gc,
            258,
            194,
            0,
            0,
            0,
            screen.root_depth,
            &detail,
        )?
        .check()?;
        for target in [(129, 97), (258, 194)] {
            let (pixmap, _) = scaler
                .capture_drawable(&conn, source, screen.root_visual, (258, 194), target)?
                .unwrap();
            let (_, _, snapshot) = scaler
                .capture_linear(&conn, pixmap)
                .context("linear detail fallback")?;
            let reference = reference_scale
                .capture_drawable(&conn, source, screen.root_visual, (258, 194), target)?
                .map_or(source, |r| r.0);
            let old = conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    reference,
                    0,
                    0,
                    target.0 as u16,
                    target.1 as u16,
                    u32::MAX,
                )?
                .reply()?;
            assert!(
                snapshot
                    .chunks_exact(4)
                    .zip(old.data.chunks_exact(4))
                    .all(|(a, b)| a[..3] == b[..3]),
                "fine detail/filter must match the former render target"
            );
        }
        let large = conn.generate_id()?;
        conn.create_pixmap(screen.root_depth, large, screen.root, 2940, 1618)?
            .check()?;
        for pass in 0..10u32 {
            let color = ((pass * 7919) ^ 0xabcdef) & 0xffffff;
            conn.change_gc(gc, &ChangeGCAux::new().foreground(color))?;
            conn.poly_fill_rectangle(
                large,
                gc,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 2940,
                    height: 1618,
                }],
            )?;
            let (pixmap, _) = scaler
                .capture_drawable(&conn, large, screen.root_visual, (2940, 1618), (1470, 809))?
                .unwrap();
            let (_, _, snapshot) = scaler
                .capture_linear(&conn, pixmap)
                .context("linear large-target fallback")?;
            let old = conn
                .get_image(ImageFormat::Z_PIXMAP, pixmap, 0, 0, 1470, 809, u32::MAX)?
                .reply()?;
            assert_eq!(snapshot.len(), 1470 * 809 * 4);
            assert!(snapshot.chunks_exact(4).all(|p| p[..3]
                == [
                    (color & 255) as u8,
                    ((color >> 8) & 255) as u8,
                    ((color >> 16) & 255) as u8
                ]));
            assert!(snapshot
                .chunks_exact(4)
                .zip(old.data.chunks_exact(4))
                .all(|(a, b)| a[..3] == b[..3]));
        }
        scaler.clear(&conn);
        reference_scale.clear(&conn);
        conn.free_gc(gc)?;
        conn.free_pixmap(source)?;
        conn.free_pixmap(large)?;
        conn.flush()?;
        Ok(())
    }
    #[test]
    #[ignore = "requires a private Xvfb DISPLAY started with -extension RENDER"]
    fn unavailable_render_preserves_the_original_readback() -> Result<()> {
        let (conn, screen) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen];
        let mut scaler = RenderScale::new(&conn, screen.root);
        assert!(
            scaler.formats.is_none(),
            "start Xvfb with -extension RENDER"
        );
        let source = conn.generate_id()?;
        conn.create_pixmap(screen.root_depth, source, screen.root, 64, 48)?
            .check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, source, &CreateGCAux::new().foreground(0x123456))?
            .check()?;
        conn.poly_fill_rectangle(
            source,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 64,
                height: 48,
            }],
        )?
        .check()?;
        assert!(scaler
            .capture_drawable(&conn, source, screen.root_visual, (64, 48), (32, 24))?
            .is_none());
        let original = conn
            .get_image(ImageFormat::Z_PIXMAP, source, 0, 0, 64, 48, u32::MAX)?
            .reply()?;
        assert_eq!(&original.data[..3], &[0x56, 0x34, 0x12]);
        assert!(scaler.canvas.is_none());
        conn.free_gc(gc)?;
        conn.free_pixmap(source)?;
        conn.flush()?;
        Ok(())
    }
    #[test]
    #[ignore = "requires a private Xvfb DISPLAY with RENDER and MIT-SHM"]
    fn scaling_preserves_selected_pixels_and_owned_snapshots_across_reuse() -> Result<()> {
        let (conn, screen) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen];
        let mut scaler = RenderScale::new(&conn, screen.root);
        let mut shared = crate::shared_capture::SharedCapture::new(&conn);
        let source = conn.generate_id()?;
        conn.create_pixmap(screen.root_depth, source, screen.root, 256, 194)?
            .check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, source, &CreateGCAux::new())?.check()?;
        let colors = [0xff0000u32, 0x00ff00, 0x0000ff];
        for y in 0..25 {
            for x in 0..32 {
                conn.change_gc(gc, &ChangeGCAux::new().foreground(colors[(x + y) % 3]))?;
                conn.poly_fill_rectangle(
                    source,
                    gc,
                    &[Rectangle {
                        x: (x * 8) as i16,
                        y: (y * 8) as i16,
                        width: 8,
                        height: 8,
                    }],
                )?;
            }
        }
        conn.flush()?;
        let (drawable, visual) = scaler
            .capture_drawable(&conn, source, screen.root_visual, (256, 194), (128, 97))?
            .context("test server lacks RENDER scaling")?;
        assert_eq!(visual, screen.root_visual);
        let (_, _, first) = shared
            .capture(&conn, drawable, 128, 97)
            .context("test server lacks SHM")?;
        for y in 0..97 {
            for x in 0..128 {
                let color = colors[(x / 4 + y / 4) % 3];
                let offset = (y * 128 + x) * 4;
                assert_eq!(
                    &first[offset..offset + 3],
                    &[
                        (color & 255) as u8,
                        ((color >> 8) & 255) as u8,
                        ((color >> 16) & 255) as u8
                    ],
                    "{x},{y}"
                );
            }
        }
        let saved = first.clone();
        let mut fine = Vec::new();
        for y in 0..194u32 {
            for x in 0..256u32 {
                fine.extend_from_slice(&[
                    ((x % 17) * 15) as u8,
                    ((y % 19) * 13) as u8,
                    (((x ^ y) % 23) * 10) as u8,
                    0,
                ]);
            }
        }
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            source,
            gc,
            256,
            194,
            0,
            0,
            0,
            screen.root_depth,
            &fine,
        )?
        .check()?;
        let (fine_drawable, _) = scaler
            .capture_drawable(&conn, source, screen.root_visual, (256, 194), (128, 97))?
            .unwrap();
        let (_, _, fine_scaled) = shared.capture(&conn, fine_drawable, 128, 97).unwrap();
        for y in 0..97usize {
            for x in 0..128usize {
                for channel in 0..3usize {
                    let sum: u32 = (0..2)
                        .flat_map(|dy| (0..2).map(move |dx| (dy, dx)))
                        .map(|(dy, dx)| {
                            u32::from(fine[((2 * y + dy) * 256 + 2 * x + dx) * 4 + channel])
                        })
                        .sum();
                    assert!(
                        i32::from(fine_scaled[(y * 128 + x) * 4 + channel])
                            .abs_diff((sum / 4) as i32)
                            <= 1,
                        "fine detail changed beyond bilinear rounding"
                    );
                }
            }
        }
        conn.change_gc(gc, &ChangeGCAux::new().foreground(0x123456))?;
        conn.poly_fill_rectangle(
            source,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 256,
                height: 194,
            }],
        )?
        .check()?;
        let (same, _) = scaler
            .capture_drawable(&conn, source, screen.root_visual, (256, 194), (128, 97))?
            .unwrap();
        assert_eq!(same, drawable, "reuse a bounded destination allocation");
        let (_, _, second) = shared.capture(&conn, same, 128, 97).unwrap();
        assert_eq!(&second[..3], &[0x56, 0x34, 0x12]);
        assert_eq!(
            first, saved,
            "later compositor writes must not mutate an in-flight snapshot"
        );
        let (resized, _) = scaler
            .capture_drawable(&conn, source, screen.root_visual, (256, 194), (100, 70))?
            .unwrap();
        assert_ne!(resized, drawable);
        assert!(
            conn.get_geometry(drawable)?.reply().is_err(),
            "retired destinations must be freed"
        );
        assert!(
            scaler
                .capture_drawable(&conn, source, screen.root_visual, (254, 194), (100, 70))
                .is_err(),
            "stale dimensions must not stretch a newer pixmap"
        );
        assert!(
            scaler.formats.is_some(),
            "resize races must not disable future scaling"
        );
        assert!(scaler
            .capture_drawable(&conn, source, 0, (256, 194), (100, 70))?
            .is_none());
        assert!(
            scaler.formats.is_none() && scaler.canvas.is_none(),
            "unsupported formats retain the ordinary capture fallback"
        );
        conn.free_gc(gc)?;
        conn.free_pixmap(source)?;
        conn.flush()?;
        Ok(())
    }
}
pub(super) struct RenderScale {
    formats: Option<render::QueryPictFormatsReply>,
    root: u32,
    visual: u32,
    depth: u8,
    canvas: Option<Canvas>,
    linear: Option<crate::linear_capture::Device>,
    linear_enabled: bool,
    linear_probed: bool,
}
impl RenderScale {
    pub fn new(conn: &RustConnection, root: u32) -> Self {
        let screen = conn.setup().roots.iter().find(|s| s.root == root).unwrap();
        let formats = conn
            .render_query_pict_formats()
            .ok()
            .and_then(|c| c.reply().ok());
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == screen.root_visual);
        let linear_enabled = formats.is_some()
            && screen.root_depth == 24
            && conn.setup().image_byte_order == ImageOrder::LSB_FIRST
            && conn
                .setup()
                .pixmap_formats
                .iter()
                .any(|f| f.depth == 24 && f.bits_per_pixel == 32 && f.scanline_pad == 32)
            && visual.is_some_and(|v| {
                (v.red_mask, v.green_mask, v.blue_mask) == (0xff0000, 0xff00, 0xff)
            });
        Self {
            formats,
            root,
            visual: screen.root_visual,
            depth: screen.root_depth,
            canvas: None,
            linear: None,
            linear_enabled,
            linear_probed: false,
        }
    }
    fn format(&self, visual: u32) -> Result<u32> {
        self.formats
            .as_ref()
            .context("RENDER unavailable")?
            .screens
            .iter()
            .flat_map(|s| &s.depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual == visual)
            .map(|v| v.format)
            .context("RENDER visual unavailable")
    }
    fn clear(&mut self, conn: &RustConnection) {
        if let Some(c) = self.canvas.take() {
            if let Some(linear) = &c.linear {
                linear.retire(conn);
            }
            let _ = conn.render_free_picture(c.picture);
            let _ = conn.free_pixmap(c.pixmap);
        }
    }
    fn scale(
        &mut self,
        conn: &RustConnection,
        drawable: u32,
        visual: u32,
        source: (u32, u32),
        target: (u32, u32),
    ) -> Result<u32> {
        let source_format = self.format(visual)?;
        let target_format = self.format(self.visual)?;
        if self
            .canvas
            .as_ref()
            .is_none_or(|c| c.size != target || (c.linear.is_some() && !self.linear_enabled))
        {
            self.clear(conn);
            let pixmap = conn.generate_id()?;
            let picture = conn.generate_id()?;
            let linear = if self.linear_enabled {
                self.linear
                    .as_ref()
                    .and_then(|d| d.buffer(conn, self.root, pixmap, target).ok())
            } else {
                None
            };
            if linear.is_none() {
                self.linear_enabled = false;
                conn.create_pixmap(
                    self.depth,
                    pixmap,
                    self.root,
                    target.0 as u16,
                    target.1 as u16,
                )?
                .check()?;
            }
            if let Err(error) = (|| -> Result<()> {
                conn.render_create_picture(
                    picture,
                    pixmap,
                    target_format,
                    &render::CreatePictureAux::new(),
                )?
                .check()?;
                Ok(())
            })() {
                let _ = conn.free_pixmap(pixmap);
                return Err(error.into());
            }
            self.canvas = Some(Canvas {
                pixmap,
                picture,
                size: target,
                linear,
            });
        }
        let source_picture = conn.generate_id()?;
        let result = (|| -> Result<()> {
            let created = conn.render_create_picture(
                source_picture,
                drawable,
                source_format,
                &render::CreatePictureAux::new().repeat(render::Repeat::PAD),
            )?;
            let filtered = conn.render_set_picture_filter(source_picture, b"bilinear", &[])?;
            let fixed = |a: u32, b: u32| ((u64::from(a) << 16) / u64::from(b)) as i32;
            let transformed = conn.render_set_picture_transform(
                source_picture,
                render::Transform {
                    matrix11: fixed(source.0, target.0),
                    matrix12: 0,
                    matrix13: 0,
                    matrix21: 0,
                    matrix22: fixed(source.1, target.1),
                    matrix23: 0,
                    matrix31: 0,
                    matrix32: 0,
                    matrix33: 1 << 16,
                },
            )?;
            let composed = conn.render_composite(
                render::PictOp::SRC,
                source_picture,
                0u32,
                self.canvas.as_ref().unwrap().picture,
                0,
                0,
                0,
                0,
                0,
                0,
                target.0 as u16,
                target.1 as u16,
            )?;
            // Queue the whole ordered operation before checking its final
            // sequence. Earlier checks then use the same server round trip.
            for check in [
                composed.check(),
                transformed.check(),
                filtered.check(),
                created.check(),
            ] {
                check?;
            }
            Ok(())
        })();
        let _ = conn.render_free_picture(source_picture);
        result?;
        Ok(self.canvas.as_ref().unwrap().pixmap)
    }
    pub fn capture_drawable(
        &mut self,
        conn: &RustConnection,
        drawable: u32,
        visual: u32,
        source: (u32, u32),
        target: (u32, u32),
    ) -> Result<Option<(u32, u32)>> {
        if self.formats.is_none() || target.0 > source.0 || target.1 > source.1 {
            return Ok(None);
        }
        // Keymap-only X11 connections and native Wayland capture must not open
        // a render device. Probe once, on this backend's first pixel request.
        if !self.linear_probed {
            self.linear_probed = true;
            if self.linear_enabled {
                self.linear = crate::linear_capture::Device::new(conn, self.root);
                self.linear_enabled = self.linear.is_some();
            }
        }
        if source == target && !self.linear_enabled {
            return Ok(None);
        }
        // A resized named pixmap must never be stretched using stale metadata.
        let geometry = conn.get_geometry(drawable)?.reply()?;
        ensure!(
            (u32::from(geometry.width), u32::from(geometry.height)) == source,
            "capture drawable changed size"
        );
        let source_format = self
            .format(visual)
            .ok()
            .and_then(|id| self.formats.as_ref()?.formats.iter().find(|f| f.id == id));
        if source_format.is_some_and(|f| f.depth != geometry.depth) {
            bail!("capture drawable changed depth");
        }
        match self.scale(conn, drawable, visual, source, target) {
            Ok(pixmap) => Ok(Some((pixmap, self.visual))),
            Err(_) => {
                self.clear(conn);
                self.formats = None;
                Ok(None)
            }
        }
    }
    pub fn capture_linear(
        &mut self,
        conn: &RustConnection,
        drawable: u32,
    ) -> Option<(u8, u32, Vec<u8>)> {
        let canvas = self.canvas.as_mut().filter(|c| c.pixmap == drawable)?;
        if !self.linear_enabled {
            return None;
        }
        match canvas.linear.as_mut()?.snapshot(conn, drawable) {
            Ok(bytes) => Some((self.depth, self.visual, bytes)),
            Err(_) => {
                // Keep this valid pixmap alive for the ordinary SHM fallback.
                // The next frame replaces it with a standard render target.
                self.linear_enabled = false;
                None
            }
        }
    }
}
