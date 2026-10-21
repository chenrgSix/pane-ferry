use super::authenticated_connection;
use anyhow::Result;
use std::{io::Cursor, path::Path};
use x11rb::{
    connection::Connection,
    protocol::{
        composite::ConnectionExt as _,
        res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
        xproto::{AtomEnum, ConnectionExt as _, MapState, WindowClass},
    },
};

#[derive(Clone, Debug)]
pub struct DesktopWindow {
    pub window_id: u32,
    pub process_id: u32,
    pub title: String,
    pub class: String,
    pub minimized: bool,
    pub icon_png: Option<Vec<u8>>,
}

/// Metadata only: never invokes scan(), capture(), resize or focus on a desktop.
pub fn desktop_windows(display: u16, authority: &Path) -> Result<Vec<DesktopWindow>> {
    let conn = authenticated_connection(display, authority)?;
    conn.composite_query_version(0, 4)?.reply()?;
    conn.res_query_version(1, 2)?.reply()?;
    let root = conn.setup().roots[0].root;
    let atom =
        |name: &str| -> Result<u32> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let list = atom("_NET_CLIENT_LIST")?;
    let title_atom = atom("_NET_WM_NAME")?;
    let utf8 = atom("UTF8_STRING")?;
    let icon_atom = atom("_NET_WM_ICON")?;
    let type_atom = atom("_NET_WM_WINDOW_TYPE")?;
    let excluded: Vec<_> = [
        "DOCK",
        "DESKTOP",
        "SPLASH",
        "NOTIFICATION",
        "TOOLTIP",
        "MENU",
    ]
    .iter()
    .map(|kind| atom(&format!("_NET_WM_WINDOW_TYPE_{kind}")))
    .collect::<Result<_>>()?;
    let clients = conn
        .get_property(false, root, list, AtomEnum::WINDOW, 0, 512)?
        .reply()?;
    let mut windows = Vec::new();
    for id in clients.value32().into_iter().flatten().take(512) {
        let candidate = (|| -> Result<Option<DesktopWindow>> {
            let attrs = conn.get_window_attributes(id)?.reply()?;
            if attrs.override_redirect || attrs.class != WindowClass::INPUT_OUTPUT {
                return Ok(None);
            }
            let kinds = conn
                .get_property(false, id, type_atom, AtomEnum::ATOM, 0, 16)?
                .reply()?;
            if kinds
                .value32()
                .into_iter()
                .flatten()
                .any(|kind| excluded.contains(&kind))
            {
                return Ok(None);
            }
            // A title or _NET_WM_PID property alone is not an attachment identity.
            let actual = conn
                .res_query_client_ids(&[ClientIdSpec {
                    client: id,
                    mask: ClientIdMask::LOCAL_CLIENT_PID,
                }])?
                .reply()?;
            let Some(pid) = actual
                .ids
                .iter()
                .find_map(|value| value.value.first().copied())
                .filter(|pid| *pid > 1)
            else {
                return Ok(None);
            };
            let title = conn
                .get_property(false, id, title_atom, utf8, 0, 256)?
                .reply()?
                .value;
            let title = if title.is_empty() {
                conn.get_property(false, id, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)?
                    .reply()?
                    .value
            } else {
                title
            };
            let class = conn
                .get_property(false, id, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
                .reply()?
                .value;
            let class = class
                .split(|byte| *byte == 0)
                .rfind(|value| !value.is_empty())
                .unwrap_or_default();
            let icon = conn
                .get_property(false, id, icon_atom, AtomEnum::CARDINAL, 0, 262_144)?
                .reply()?;
            let icon: Vec<_> = icon.value32().into_iter().flatten().collect();
            Ok(Some(DesktopWindow {
                window_id: id,
                process_id: pid,
                title: String::from_utf8_lossy(&title)
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(256)
                    .collect(),
                class: String::from_utf8_lossy(class)
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(128)
                    .collect(),
                minimized: attrs.map_state != MapState::VIEWABLE,
                icon_png: icon_png(&icon),
            }))
        })();
        if let Ok(Some(window)) = candidate {
            windows.push(window);
        }
        if windows.len() == 128 {
            break;
        }
    }
    Ok(windows)
}

fn icon_png(values: &[u32]) -> Option<Vec<u8>> {
    let mut offset = 0;
    let mut chosen: Option<(u32, u32, &[u32])> = None;
    while offset + 2 <= values.len() {
        let (width, height) = (values[offset], values[offset + 1]);
        let length = width.checked_mul(height)? as usize;
        if width == 0 || height == 0 || length > 512 * 512 || offset + 2 + length > values.len() {
            break;
        }
        let pixels = &values[offset + 2..offset + 2 + length];
        if chosen.as_ref().is_none_or(|(w, h, _)| {
            width.abs_diff(64) + height.abs_diff(64) < w.abs_diff(64) + h.abs_diff(64)
        }) {
            chosen = Some((width, height, pixels));
        }
        offset += 2 + length;
    }
    let (width, height, pixels) = chosen?;
    let fit = (64.0 / f64::from(width.max(height))).min(1.0);
    let (w, h) = (
        (f64::from(width) * fit).max(1.0) as u32,
        (f64::from(height) * fit).max(1.0) as u32,
    );
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let value = pixels[(y * height / h * width + x * width / w) as usize];
            rgba.extend_from_slice(&[
                (value >> 16) as u8,
                (value >> 8) as u8,
                value as u8,
                (value >> 24) as u8,
            ]);
        }
    }
    let mut out = Cursor::new(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
    }
    let bytes = out.into_inner();
    (bytes.len() <= 16 * 1024).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_icon_preserves_argb_and_rejects_bad_dimensions() {
        assert!(icon_png(&[u32::MAX, u32::MAX]).is_none());
        assert!(icon_png(&[128, 128, 1]).is_none());
        let bytes = icon_png(&[1, 1, 0x80402010]).unwrap();
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut output = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut output).unwrap();
        assert_eq!(output, [0x40, 0x20, 0x10, 0x80]);
    }
}
