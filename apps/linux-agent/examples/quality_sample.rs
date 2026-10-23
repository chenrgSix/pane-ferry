//! Opt-in, one-shot X11/Xwayland pixel probe. Never scan/resize/focus the window,
//! acquire a controller lease, redirect a window, or change the running Agent.
//! DISPLAY and XAUTHORITY must identify the explicitly selected X server.
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use paneferry_backend_x11::CapturedImage;
use serde_json::json;
use std::{fs, os::unix::fs::DirBuilderExt, path::Path, sync::Arc};
use x11rb::{
    connection::Connection,
    protocol::{
        composite::ConnectionExt as _,
        res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
        xproto::{ConnectionExt as _, ImageFormat, ImageOrder, MapState},
    },
};

// Use the actual encoder implementation and parameters, not an FFmpeg substitute.
#[allow(dead_code)]
#[path = "../src/video.rs"]
mod video;

fn identity(pid: u32) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}");
    ensure!(fs::metadata(&path)?.uid() == unsafe { libc::geteuid() });
    let stat = fs::read_to_string(format!("{path}/stat"))?;
    Ok(stat
        .rsplit_once(')')
        .context("process stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("process start time")?
        .to_owned())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "quality_sample XID PID NEW_OUTPUT_DIRECTORY"
    );
    let xid: u32 = args[1].parse()?;
    let pid: u32 = args[2].parse()?;
    let output = Path::new(&args[3]);
    ensure!(!output.exists(), "output directory must be new");
    let display = std::env::var("DISPLAY").context("explicit DISPLAY required")?;
    std::env::var("XAUTHORITY").context("explicit XAUTHORITY required")?;
    let started = identity(pid)?;
    let (conn, _) = x11rb::connect(Some(&display))?;
    let owner = conn
        .res_query_client_ids(&[ClientIdSpec {
            client: xid,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        }])?
        .reply()?;
    ensure!(
        owner.ids.iter().any(|id| id.value.first() == Some(&pid)),
        "XRes process identity mismatch"
    );
    let attributes = conn.get_window_attributes(xid)?.reply()?;
    ensure!(attributes.map_state == MapState::VIEWABLE, "window hidden");
    let before = conn.get_geometry(xid)?.reply()?;
    let (width, height) = (u32::from(before.width), u32::from(before.height));
    paneferry_protocol::dimensions(width, height)?;
    let xwayland = conn.query_extension(b"XWAYLAND")?.reply()?.present;

    // Same pixel source as Backend::capture_image; no compositor redirection.
    // Xvfb without a compositor uses the drawable itself, as the Agent does.
    conn.composite_query_version(0, 4)?.reply()?;
    let pixmap = conn.generate_id()?;
    let named = conn
        .composite_name_window_pixmap(xid, pixmap)?
        .check()
        .is_ok();
    let drawable = if named { pixmap } else { xid };
    let geometry = conn.get_geometry(drawable)?.reply()?;
    ensure!(
        (geometry.width, geometry.height) == (before.width, before.height),
        "pixmap and window geometry differ; sample rejected"
    );
    let raw = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            drawable,
            0,
            0,
            before.width,
            before.height,
            u32::MAX,
        )?
        .reply()?;
    if named {
        conn.free_pixmap(pixmap)?.check()?;
    }
    let after = conn.get_geometry(xid)?.reply()?;
    ensure!(
        (before.width, before.height, before.depth) == (after.width, after.height, after.depth)
            && identity(pid)? == started,
        "window/process changed during capture; sample rejected"
    );
    let setup = conn.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == raw.depth)
        .context("pixel format")?;
    let visual_id = if raw.visual == 0 {
        attributes.visual
    } else {
        raw.visual
    };
    let visual = setup
        .roots
        .iter()
        .flat_map(|s| &s.allowed_depths)
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == visual_id)
        .context("visual")?;
    let image = Arc::new(CapturedImage {
        width,
        height,
        data: raw.data,
        bpp: format.bits_per_pixel,
        pad: format.scanline_pad,
        little: setup.image_byte_order == ImageOrder::LSB_FIRST,
        masks: (visual.red_mask, visual.green_mask, visual.blue_mask),
    });
    let png = image.png()?;
    let mut samples = Vec::new();
    // The live negotiated cap is not exposed by window.list. Compare both
    // supported desktop caps, explicitly labelled; do not claim either was live.
    for fps in [60, 90] {
        let mut encoder = video::Encoder::with_cap(width, height, 1, fps)?;
        let mut packets = Vec::new();
        for _ in 0..3 {
            let packet = encoder.encode(&image)?;
            packets.push(json!({"key_frame":packet.keyframe,"data":STANDARD.encode(packet.bytes)}));
        }
        samples.push(json!({"max_fps":fps,"packets":packets}));
    }
    ensure!(image.png()? == png, "encoder modified source pixels");
    fs::DirBuilder::new().mode(0o700).create(output)?;
    fs::write(output.join("source.png"), png)?;
    fs::write(
        output.join("sample.json"),
        serde_json::to_vec_pretty(&json!({
            "schema":1,"capture":"one read-only X11 snapshot; isolated encoding of identical pixels",
            "window_id":xid,"pid":pid,"process_start_ticks":started,
            "backend":if xwayland {"xwayland"} else {"x11"},
            "capture_source":if named {"XComposite pixmap"} else {"window drawable"},
            "width":width,"height":height,"encoded_width":width+width%2,"encoded_height":height+height%2,
            "geometry_revision":null,"source_scale":null,"samples":samples,
            "limitations":["Not a tap of the running stream; live geometry_revision and negotiated cap are unknown",
                "X11 pixel dimensions do not measure toolkit DPI; no application redraw is requested",
                "Three repeated source frames do not measure live bandwidth, CPU, FPS or latency"]
        }))?,
    )?;
    println!("saved one {width}x{height} sample; original process/window untouched");
    Ok(())
}
