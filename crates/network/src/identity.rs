//! TLS proves possession of each device key. Application authorization is a
//! separate, revocable certificate pin; an unknown TLS client can only pair.
use anyhow::{bail, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rustls::{
    client::danger::HandshakeSignatureValid,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

pub const ALPN: &[u8] = b"lan-remoteapp/2";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: String,
    pub certificate: String,
    key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub device_id: String,
    pub name: String,
    pub certificate: String,
    pub address: String,
}
impl Peer {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.device_id)?.to_string() == self.device_id,
            "invalid device"
        );
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 128
                && !self.name.chars().any(char::is_control),
            "invalid device name"
        );
        ensure!(self.certificate.len() <= 8192, "invalid certificate");
        self.address.parse::<std::net::SocketAddr>()?;
        Ok(())
    }
}
pub fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    result
}
pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let info = fs::symlink_metadata(path)?;
    ensure!(
        info.file_type().is_file() && info.len() < 1024 * 1024,
        "invalid private file"
    );
    ensure!(
        info.uid() == unsafe { libc::geteuid() } && info.mode() & 0o077 == 0,
        "unsafe private file permissions"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
impl Identity {
    pub fn load(directory: &Path, id: Option<&str>) -> Result<Self> {
        remoteapp_core::node::private_directory(directory)?;
        let path = directory.join("identity.json");
        if path.exists() {
            let value: Self = read(&path)?;
            ensure!(id.is_none_or(|id| id == value.id), "identity mismatch");
            return Ok(value);
        }
        let certified = rcgen::generate_simple_self_signed(vec!["lan-remoteapp".into()])?;
        let id = id
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        uuid::Uuid::parse_str(&id)?;
        let value = Self {
            id,
            certificate: STANDARD.encode(certified.cert.der()),
            key: STANDARD.encode(certified.signing_key.serialize_der()),
        };
        save(&path, &value)?;
        Ok(value)
    }
    fn key(&self) -> Result<PrivatePkcs8KeyDer<'static>> {
        Ok(STANDARD.decode(&self.key)?.into())
    }
    pub fn cert(&self) -> Result<CertificateDer<'static>> {
        certificate(&self.certificate)
    }
    pub fn server(&self) -> Result<quinn::ServerConfig> {
        let mut crypto = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(PossessionVerifier))
        .with_single_cert(vec![self.cert()?], self.key()?.into())?;
        crypto.alpn_protocols = vec![ALPN.to_vec()];
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
        ));
        config.transport_config(transport());
        Ok(config)
    }
    pub fn client(&self, pinned: &str) -> Result<quinn::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate(pinned)?)?;
        let mut crypto = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![self.cert()?], self.key()?.into())?;
        crypto.alpn_protocols = vec![ALPN.to_vec()];
        crypto.enable_early_data = false;
        let mut config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
        ));
        config.transport_config(transport());
        Ok(config)
    }
}
fn transport() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config
        .max_concurrent_bidi_streams(128u32.into())
        .max_concurrent_uni_streams(0u32.into())
        .max_idle_timeout(Some(std::time::Duration::from_secs(30).try_into().unwrap()))
        .keep_alive_interval(Some(std::time::Duration::from_secs(5)))
        .stream_receive_window((4u32 * 1024 * 1024).into())
        .receive_window((16u32 * 1024 * 1024).into())
        .send_window(16 * 1024 * 1024);
    Arc::new(config)
}
pub fn certificate(value: &str) -> Result<CertificateDer<'static>> {
    ensure!(value.len() <= 8192, "invalid certificate");
    Ok(STANDARD.decode(value)?.into())
}
pub fn fingerprint(value: &str) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(certificate(value)?.as_ref())
    ))
}
pub fn remote_certificate(connection: &quinn::Connection) -> Result<String> {
    let certs = connection
        .peer_identity()
        .ok_or_else(|| anyhow::anyhow!("missing certificate"))?
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| anyhow::anyhow!("invalid certificate"))?;
    if certs.len() != 1 {
        bail!("invalid certificate chain");
    }
    Ok(STANDARD.encode(certs[0].as_ref()))
}
#[derive(Debug)]
struct PossessionVerifier;
impl ClientCertVerifier for PossessionVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() || cert.len() > 6144 {
            return Err(rustls::Error::General("invalid device certificate".into()));
        }
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone().into_owned())?;
        rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|_| rustls::Error::General("invalid device certificate".into()))?
        .verify_client_cert(cert, intermediates, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
