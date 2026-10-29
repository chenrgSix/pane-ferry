//! Same private named-pipe transport as the Windows publisher and QUIC router.
use paneferry_platform::ipc::LocalStream;
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub async fn read<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value, String> {
    let mut bytes = Vec::new();
    // Bound allocation even when a broken peer never sends a newline.
    reader.fill_buf().await.map_err(|_| "连接中断。")?;
    tokio::time::timeout(
        Duration::from_secs(15),
        reader
            .take(16 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| "远端消息超时。")?
    .map_err(|_| "连接中断。")?;
    if bytes.len() > 16 * 1024 * 1024 || bytes.last() != Some(&b'\n') {
        return Err("远端消息无效。".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "远端消息无效。")?;
    if !value.is_object() {
        return Err("远端消息无效。".into());
    }
    Ok(value)
}
pub async fn write<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    body: &Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(body).map_err(|_| "请求无效。")?;
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), writer.write_all(&bytes))
        .await
        .map_err(|_| "发送超时。")?
        .map_err(|_| "连接中断。".into())
}
pub fn exchange(path: &Path, body: Value, hello: bool) -> Result<Value, String> {
    let path = path.to_owned();
    // The launcher calls this from both sync and async commands. A dedicated
    // thread avoids nesting block_on inside Tauri's Tokio runtime.
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "无法初始化本机连接。")?;
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), async {
                let stream = LocalStream::connect(path)
                    .await
                    .map_err(|_| "本机服务尚未就绪。")?;
                let (input, mut output) = stream.into_split();
                let mut input = BufReader::new(input);
                if hello {
                    write(
                        &mut output,
                        &serde_json::json!({"type":"hello","version":1}),
                    )
                    .await?;
                    let response = read(&mut input).await?;
                    if response["type"] != "hello" || response["version"] != 1 {
                        return Err("本机服务协议无效。".into());
                    }
                }
                write(&mut output, &body).await?;
                let response = read(&mut input).await?;
                if response.get("error").is_some() || response["type"] == "error" {
                    return Err("操作未完成，请检查设备地址、邀请和共享状态。".into());
                }
                Ok(response)
            })
            .await
            .map_err(|_| "本机操作超时。".to_string())?
        })
    })
    .join()
    .map_err(|_| "本机操作失败。".to_string())?
}
