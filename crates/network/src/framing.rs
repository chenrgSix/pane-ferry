use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
pub const MAX: usize = 16 * 1024 * 1024;
pub async fn line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value> {
    line_limit(reader, MAX).await
}
/// Publishers only accept small controls; video compatibility bridges use MAX.
pub async fn line_limit<R: AsyncBufRead + Unpin>(reader: &mut R, limit: usize) -> Result<Value> {
    ensure!(limit > 0 && limit <= MAX, "invalid message limit");
    let mut bytes = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        ensure!(!buf.is_empty(), "connection closed");
        let n = buf
            .iter()
            .position(|c| *c == b'\n')
            .map_or(buf.len(), |n| n + 1);
        ensure!(bytes.len() + n <= limit, "message too large");
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
        Self::read_optional(reader)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed"))
    }
    pub async fn read_optional<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Self>> {
        Self::read_bounded(reader, MAX, 12 * 1024 * 1024, true).await
    }
    /// Before authentication (and for stream headers), never allocate from the
    /// media pool or accept a frame-sized metadata/body declaration.
    pub async fn read_header<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self> {
        Self::read_bounded(reader, 8192, 0, false)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed"))
    }
    async fn read_bounded<R: AsyncRead + Unpin>(
        reader: &mut R,
        limit: usize,
        binary_limit: usize,
        reserve: bool,
    ) -> Result<Option<Self>> {
        let mut first = [0u8; 1];
        if reader.read(&mut first).await? == 0 {
            return Ok(None);
        }
        // Idle streams are valid. Once a packet begins, partial progress cannot
        // extend its absolute completion deadline indefinitely.
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut length = [0u8; 4];
            length[0] = first[0];
            reader.read_exact(&mut length[1..]).await?;
            let meta = u32::from_be_bytes(length) as usize;
            let binary = reader.read_u32().await? as usize;
            ensure!(
                meta > 0 && meta <= limit && binary <= binary_limit && meta + binary <= limit,
                "packet too large"
            );
            let barrier = reader.read_u64().await?;
            let reservation = if reserve {
                Some(budget(meta + binary).await?)
            } else {
                None
            };
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
            Ok(Some(Self {
                value,
                payload,
                barrier,
                reservation,
            }))
        })
        .await?
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
    async fn handshake_rejects_large_or_binary_headers_before_reading_a_body() {
        for (meta, binary) in [(8193u32, 0u32), (32, 1), (16 * 1024 * 1024, 0)] {
            let mut header = Vec::new();
            header.extend(meta.to_be_bytes());
            header.extend(binary.to_be_bytes());
            assert!(Packet::read_header(&mut header.as_slice())
                .await
                .unwrap_err()
                .to_string()
                .contains("packet too large"));
        }
        let mut bytes = Vec::new();
        Packet::new(serde_json::json!({"op":"auth","device_id":"test"}), 0)
            .unwrap()
            .write(&mut bytes)
            .await
            .unwrap();
        assert_eq!(
            Packet::read_header(&mut bytes.as_slice())
                .await
                .unwrap()
                .value["op"],
            "auth"
        );
    }
    #[tokio::test(start_paused = true)]
    async fn packet_deadline_starts_with_first_byte_but_idle_is_not_eof() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        let task = tokio::spawn(async move { Packet::read_optional(&mut reader).await });
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        assert!(!task.is_finished());
        writer.write_all(&[0]).await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(16)).await;
        assert!(task.await.unwrap().is_err());
        assert!(Packet::read_optional(&mut &b""[..])
            .await
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn publisher_control_limit_rejects_large_partial_lines() {
        let data = vec![b'a'; 65 * 1024];
        let mut read = tokio::io::BufReader::new(data.as_slice());
        assert!(line_limit(&mut read, 64 * 1024).await.is_err());
        let mut read = tokio::io::BufReader::new(&b"{\"type\":\"hello\"}\n"[..]);
        assert_eq!(line_limit(&mut read, 64).await.unwrap()["type"], "hello");
    }
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
