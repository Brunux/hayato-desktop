//! Cryptographic identity, self-signed TLS acceptor, SPKI fingerprinting,
//! and 6-digit Short Authentication String (SAS) derivation for Hayato Connect.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use openssl::asn1::Asn1Time;
use openssl::base64::encode_block;
use openssl::bn::BigNum;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::sha::sha256;
use openssl::ssl::{SslAcceptor, SslMethod};
use openssl::x509::extension::{BasicConstraints, KeyUsage, SubjectAlternativeName};
use openssl::x509::{X509, X509Builder, X509NameBuilder};

use crate::system::share::server::generate_token;

pub struct ConnectIdentity {
    pub cert: X509,
    pub key: PKey<Private>,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub spki_fp: String,
    pub cert_ip: String,
}

impl ConnectIdentity {
    /// Load or create the device ECDSA P-256 identity key and sign a LAN cert
    /// containing `lan_ip` in its SubjectAlternativeName extension.
    pub fn load_default(lan_ip: &str) -> Result<Self, String> {
        let dir = default_identity_dir();
        Self::load_or_create(&dir, lan_ip)
    }

    pub fn load_or_create(dir: &Path, lan_ip: &str) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("create connect identity dir: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        let key_path = dir.join("identity.key");
        let cert_path = dir.join("identity.pem");
        let ip_marker = dir.join("identity.ip");

        let (key, key_pem) = if key_path.is_file() {
            let bytes = fs::read(&key_path).map_err(|e| format!("read identity key: {e}"))?;
            let key = PKey::private_key_from_pem(&bytes)
                .map_err(|e| format!("parse identity key: {e}"))?;
            (key, bytes)
        } else {
            let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)
                .map_err(|e| format!("init P-256 group: {e}"))?;
            let ec = EcKey::generate(&group).map_err(|e| format!("generate P-256 key: {e}"))?;
            let key = PKey::from_ec_key(ec).map_err(|e| format!("wrap EC key: {e}"))?;
            let pem = key
                .private_key_to_pem_pkcs8()
                .map_err(|e| format!("encode key PEM: {e}"))?;
            write_secret_file(&key_path, &pem)?;
            (key, pem)
        };

        let cached_ip = fs::read_to_string(&ip_marker).unwrap_or_default();
        let (cert, cert_pem) = if cert_path.is_file() && cached_ip.trim() == lan_ip {
            let bytes = fs::read(&cert_path).map_err(|e| format!("read identity cert: {e}"))?;
            match X509::from_pem(&bytes) {
                Ok(cert) => (cert, bytes),
                Err(_) => sign_cert_and_save(&key, lan_ip, &cert_path, &ip_marker)?,
            }
        } else {
            sign_cert_and_save(&key, lan_ip, &cert_path, &ip_marker)?
        };

        let spki_fp = spki_fingerprint_b64url(&cert)?;
        Ok(Self {
            cert,
            key,
            cert_pem,
            key_pem,
            spki_fp,
            cert_ip: lan_ip.to_string(),
        })
    }

    pub fn ssl_acceptor(&self) -> Result<SslAcceptor, String> {
        let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
            .map_err(|e| format!("create TLS acceptor: {e}"))?;
        builder
            .set_certificate(&self.cert)
            .map_err(|e| format!("set TLS certificate: {e}"))?;
        builder
            .set_private_key(&self.key)
            .map_err(|e| format!("set TLS private key: {e}"))?;
        builder
            .check_private_key()
            .map_err(|e| format!("verify TLS keypair: {e}"))?;
        Ok(builder.build())
    }
}

pub fn default_identity_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        })
        .join("hayato/connect")
}

/// Compute URL-safe base64 SHA-256 fingerprint of the certificate DER.
pub fn spki_fingerprint_b64url(cert: &X509) -> Result<String, String> {
    let der = cert.to_der().map_err(|e| format!("encode cert DER: {e}"))?;
    let hash = sha256(&der);
    Ok(b64url_no_pad(&hash))
}

/// Derive a deterministic 6-digit Short Authentication String (`XXX-YYY`)
/// bound to the server certificate fingerprint, session nonce, and phone nonce.
pub fn derive_sas(cert_fp: &str, session_nonce: &str, peer_nonce: &str) -> String {
    let input = format!("{cert_fp}\0{session_nonce}\0{peer_nonce}");
    let digest = sha256(input.as_bytes());
    let num = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{:03}-{:03}", num / 1000, num % 1000)
}

/// Compute SHA-256 hex digest (used for clipboard echo prevention and secrets).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Short readable random secret for paired device tokens.
pub fn mint_device_secret() -> String {
    format!("{}{}", generate_token(), generate_token())
}

fn b64url_no_pad(bytes: &[u8]) -> String {
    encode_block(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    f.write_all(bytes)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

fn sign_cert_and_save(
    key: &PKey<Private>,
    lan_ip: &str,
    cert_path: &Path,
    ip_marker: &Path,
) -> Result<(X509, Vec<u8>), String> {
    let mut builder = X509Builder::new().map_err(|e| format!("init X509 builder: {e}"))?;
    builder.set_version(2).map_err(|e| e.to_string())?;

    let mut serial = BigNum::new().map_err(|e| e.to_string())?;
    serial
        .rand(64, openssl::bn::MsbOption::MAYBE_ZERO, false)
        .map_err(|e| e.to_string())?;
    let serial_asn1 = serial.to_asn1_integer().map_err(|e| e.to_string())?;
    builder
        .set_serial_number(&serial_asn1)
        .map_err(|e| e.to_string())?;

    let mut name = X509NameBuilder::new().map_err(|e| e.to_string())?;
    name.append_entry_by_text("CN", "Hayato Connect")
        .map_err(|e| e.to_string())?;
    let name = name.build();
    builder.set_subject_name(&name).map_err(|e| e.to_string())?;
    builder.set_issuer_name(&name).map_err(|e| e.to_string())?;
    builder.set_pubkey(key).map_err(|e| e.to_string())?;

    let not_before = Asn1Time::days_from_now(0).map_err(|e| e.to_string())?;
    let not_after = Asn1Time::days_from_now(3650).map_err(|e| e.to_string())?;
    builder
        .set_not_before(&not_before)
        .map_err(|e| e.to_string())?;
    builder
        .set_not_after(&not_after)
        .map_err(|e| e.to_string())?;

    let bc = BasicConstraints::new()
        .build()
        .map_err(|e| format!("BasicConstraints: {e}"))?;
    builder.append_extension(bc).map_err(|e| e.to_string())?;

    let ku = KeyUsage::new()
        .digital_signature()
        .key_encipherment()
        .build()
        .map_err(|e| format!("KeyUsage: {e}"))?;
    builder.append_extension(ku).map_err(|e| e.to_string())?;

    let mut san = SubjectAlternativeName::new();
    san.dns("hayato.local");
    san.ip("127.0.0.1");
    if !lan_ip.is_empty() && lan_ip != "127.0.0.1" {
        if lan_ip.parse::<std::net::IpAddr>().is_ok() {
            san.ip(lan_ip);
        } else {
            san.dns(lan_ip);
        }
    }
    let san_ext = san
        .build(&builder.x509v3_context(None, None))
        .map_err(|e| format!("build SAN: {e}"))?;
    builder
        .append_extension(san_ext)
        .map_err(|e| e.to_string())?;

    builder
        .sign(key, MessageDigest::sha256())
        .map_err(|e| format!("sign X509 cert: {e}"))?;
    let cert = builder.build();
    let pem = cert.to_pem().map_err(|e| format!("encode cert PEM: {e}"))?;
    write_secret_file(cert_path, &pem)?;
    let _ = fs::write(ip_marker, lan_ip.as_bytes());
    Ok((cert, pem))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn identity_persists_key_and_updates_san_across_ips() {
        let dir = tempfile::tempdir().unwrap();
        let id1 = ConnectIdentity::load_or_create(dir.path(), "192.168.1.42").unwrap();
        let mode = fs::metadata(dir.path().join("identity.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(id1.cert_ip, "192.168.1.42");
        assert!(!id1.spki_fp.is_empty());

        // Same IP reloads cleanly with identical cert fingerprint
        let id2 = ConnectIdentity::load_or_create(dir.path(), "192.168.1.42").unwrap();
        assert_eq!(id1.spki_fp, id2.spki_fp);
        assert!(id2.ssl_acceptor().is_ok());

        // Different IP re-signs using the same key
        let id3 = ConnectIdentity::load_or_create(dir.path(), "10.0.0.19").unwrap();
        assert_eq!(id1.key_pem, id3.key_pem);
        assert_eq!(id3.cert_ip, "10.0.0.19");
    }

    #[test]
    fn derive_sas_is_deterministic_and_sensitive() {
        let a = derive_sas("fp1", "sessA", "peer1");
        let b = derive_sas("fp1", "sessA", "peer1");
        let c = derive_sas("fp1", "sessA", "peer2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 7);
        assert_eq!(&a[3..4], "-");
    }
}
