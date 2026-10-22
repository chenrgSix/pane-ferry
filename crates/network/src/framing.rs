use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
pub const MAX: usize = 16 * 1024 * 1024;
pub async fn line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        ensure!(!buf.is_empty(), "connection closed");
        let n = buf
            .iter()
            .position(|c| *c == b'\n')
            .map_or(buf.len(), |n| n + 1);
        ensure!(bytes.len() + n <= MAX, "message too large");
        let done = buf[n - 1] == b'\n';
        bytes.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if done {
            return Ok(serde_json::from_slice(&bytes)?);
        }
    }
}
pub async fn json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() < MAX, "message too large");
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
/// RWN/2: u32 JSON length, u32 binary length, u64 control watermark,
/// JSON metadata, binary H.264/PNG payload. All integers are big endian.
#[derive(Debug)]
pub struct Packet {
    pub value: Value,
    pub payload: Vec<u8>,
    pub barrier: u64,
    reservation: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Packet {
    pub fn new(mut value: Value, barrier: u64) -> Result<Self> {
        let mut payload = Vec::new();
        if value["type"] == "frame" {
            let key = if value["codec"] == "h264" {
                "data"
            } else {
                "png"
            };
            let encoded = value
                .as_object_mut()
                .and_then(|o| o.remove(key))
                .ok_or_else(|| anyhow::anyhow!("missing frame"))?;
            payload = STANDARD.decode(
                encoded
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid frame"))?,
            )?;
            ensure!(payload.len() <= 12 * 1024 * 1024, "frame too large");
        }
        Ok(Self {
            value,
            payload,
            barrier,
            reservation: None,
        })
    }
    pub async fn reserve(&mut self) -> Result<()> {
        if self.reservation.is_none() {
            self.reservation = Some(budget(self.bytes()).await?);
        }
        Ok(())
    }
    pub fn restore(mut self) -> Value {
        if self.value["type"] == "frame" {
            let key = if self.value["codec"] == "h264" {
                "data"
            } else {
                "png"
            };
            self.value[key] = Value::String(STANDARD.encode(self.payload));
        }
        self.value
    }
    pub async fn write<W: AsyncWrite + Unpin>(&self, writer: &mut W) -> Result<()> {
        let meta = serde_json::to_vec(&self.value)?;
        ensure!(meta.len() + self.payload.len() <= MAX, "packet too large");
        writer.write_u32(meta.len() as u32).await?;
        writer.write_u32(self.payload.len() as u32).await?;
        writer.write_u64(self.barrier).await?;
        writer.write_all(&meta).await?;
        writer.write_all(&self.payload).await?;
        writer.flush().await?;
        Ok(())
    }
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self> {
        let meta = reader.read_u32().await? as usize;
        let binary = reader.read_u32().await? as usize;
        ensure!(
            meta > 0 && meta <= MAX && binary <= 12 * 1024 * 1024 && meta + binary <= MAX,
            "packet too large"
        );
        let barrier = reader.read_u64().await?;
        let reservation = Some(budget(meta + binary).await?);
        let mut bytes = vec![0; meta];
        reader.read_exact(&mut bytes).await?;
        let value: Value = serde_json::from_slice(&bytes)?;
        ensure!(value.is_object(), "invalid packet");
        ensure!(
            binary == 0 || value["type"] == "frame",
            "unexpected binary payload"
        );
        let mut payload = vec![0; binary];
        reader.read_exact(&mut payload).await?;
        Ok(Self {
            value,
            payload,
            barrier,
            reservation,
        })
    }
    pub fn bytes(&self) -> usize {
        self.payload.len() + self.value.to_string().len()
    }
}

async fn budget(bytes: usize) -> Result<tokio::sync::OwnedSemaphorePermit> {
    static BUDGET: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    Ok(tokio::time::timeout(
        std::time::Duration::from_secs(15),
        BUDGET
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(128 * 1024 * 1024)))
            .clone()
            .acquire_many_owned(bytes as u32),
    )
    .await??)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn binary_frame_roundtrip_and_oversize_before_allocation() {
        let packet = Packet::new(
            serde_json::json!({"type":"frame","codec":"h264","data":"AAECAw==","seq":55}),
            9,
        )
        .unwrap();
        let mut wire = Vec::new();
        packet.write(&mut wire).await.unwrap();
        let packet = Packet::read(&mut wire.as_slice()).await.unwrap();
        assert_eq!(packet.barrier, 9);
        assert_eq!(packet.restore()["data"], "AAECAw==");
        let mut hostile: &[u8] = &[255, 255, 255, 255, 255, 255, 255, 255];
        assert!(Packet::read(&mut hostile).await.is_err());
    }
}
