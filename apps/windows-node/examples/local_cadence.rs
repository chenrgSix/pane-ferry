//! Read only the synthetic fixture through local IPC, without a network or decoder.
use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use paneferry_network::framing;
use paneferry_platform::ipc::LocalStream;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

async fn read<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<(Value, Instant, u64)> {
    let mut bytes = Vec::new();
    ensure!(
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            reader.read_until(b'\n', &mut bytes)
        )
        .await??
            > 0,
        "EOF"
    );
    let received = Instant::now();
    let value: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        value["type"] != "error",
        "publisher error: {}",
        value["code"]
    );
    Ok((value, received, received.elapsed().as_micros() as u64))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Optional diagnostic export is restricted to the named synthetic fixture.
    // Keep encoded data in memory so file writes do not perturb frame timing.
    let export = std::env::var_os("PANEFERRY_CADENCE_EXPORT").map(PathBuf::from);
    let frame_limit: usize = std::env::var("PANEFERRY_CADENCE_FRAMES")
        .ok()
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(1800);
    ensure!(
        (2..=1800).contains(&frame_limit),
        "frame limit must be 2..1800"
    );
    let mut encoded = Vec::new();
    let color = std::env::var("PANEFERRY_CADENCE_COLOR").unwrap_or_else(|_| "444-atlas-v1".into());
    ensure!(
        matches!(color.as_str(), "420" | "444-atlas-v1" | "444-atlas-v2"),
        "unsupported probe color"
    );
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("PaneFerry")
        });
    let socket = LocalStream::connect(root.join("node/management.sock")).await?;
    let (input, mut output) = socket.into_split();
    let mut input = BufReader::new(input);
    framing::json(&mut output, &json!({"version":1,"type":"hello"})).await?;
    let _ = read(&mut input).await?;
    framing::json(&mut output, &json!({"version":1,"type":"window.list"})).await?;
    let windows = loop {
        let value = read(&mut input).await?.0;
        if value["type"] == "window.list" {
            break value;
        }
    };
    let target = windows["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["title"] == "PaneFerry dynamic video verification" && w["controlled"] != true)
        .ok_or_else(|| anyhow::anyhow!("synthetic fixture unavailable"))?;
    framing::json(&mut output, &json!({"version":1,"type":"video.enable","codec":"h264","color":color,"max_fps":60,"feedback":true})).await?;
    framing::json(&mut output, &json!({"version":1,"type":"app.launch","app_id":target["id"],"request_id":uuid::Uuid::new_v4().to_string(),"backend":"native","attach":true})).await?;
    let origin = Instant::now();
    let mut last = None;
    let mut gaps = Vec::new();
    let mut parsing = Vec::new();
    let mut frames = 0;
    let mut keyframes = 0;
    let mut total_bytes = 0u64;
    let mut sizes = Vec::new();
    while frames < frame_limit {
        let (v, received, parse_us) = read(&mut input).await?;
        if v["type"] != "frame" {
            continue;
        }
        let bytes = v["data"].as_str().unwrap().len() * 3 / 4;
        if export.is_some() {
            let data = STANDARD.decode(v["data"].as_str().unwrap())?;
            ensure!(
                encoded.len() + data.len() <= 256 * 1024 * 1024,
                "export limit exceeded"
            );
            encoded.extend_from_slice(&data);
        }
        let key = v["key_frame"] == true;
        if frames == 0 {
            println!(
                "DIMENSIONS {}x{} encoded={}x{}",
                v["width"], v["height"], v["encoded_width"], v["encoded_height"]
            );
        }
        if let Some(last) = last {
            let gap: std::time::Duration = received - last;
            gaps.push(gap.as_micros() as u64);
            if gap.as_millis() >= 30 || key {
                println!(
                    "FRAME at_ms={} index={} gap_us={} parse_us={} bytes={} key={}",
                    received.duration_since(origin).as_millis(),
                    frames,
                    gap.as_micros(),
                    parse_us,
                    bytes,
                    key
                );
            }
        }
        if key {
            keyframes += 1;
        }
        parsing.push(parse_us);
        total_bytes += bytes as u64;
        sizes.push(bytes as u64);
        frames += 1;
        last = Some(received);
        framing::json(
            &mut output,
            &json!({"version":1,"type":"video.feedback","frame_id":v["frame_id"]}),
        )
        .await?;
    }
    framing::json(&mut output, &json!({"version":1,"type":"session.detach"})).await?;
    if let Some(path) = export {
        std::fs::write(path, encoded)?;
    }
    for (stage, values) in [
        ("frame_gap_us", &mut gaps),
        ("parse_us", &mut parsing),
        ("frame_bytes", &mut sizes),
    ] {
        values.sort_unstable();
        println!(
            "SUMMARY stage={} p50={} p95={} p99={} max={}",
            stage,
            values[values.len() / 2],
            values[values.len() * 95 / 100],
            values[values.len() * 99 / 100],
            values.last().unwrap()
        );
    }
    println!(
        "SUMMARY frames={} keys={} elapsed_ms={} mbps={:.2}",
        frames,
        keyframes,
        origin.elapsed().as_millis(),
        total_bytes as f64 * 8.0 / origin.elapsed().as_secs_f64() / 1e6
    );
    Ok(())
}
