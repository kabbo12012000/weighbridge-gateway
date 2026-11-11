//! Optional wss:// support for HTTPS-hosted frontends.
//!
//! `setup()` creates a one-off local CA, signs a certificate for 127.0.0.1/localhost with it,
//! **discards the CA private key** (so the trusted root can never sign anything else), and adds
//! the CA to the current user's Windows "Trusted Root" store via certutil (Windows asks the
//! operator to confirm). Chrome and Edge use that store; Firefox needs
//! `security.enterprise_roots.enabled = true`.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use rand::{rngs::OsRng, RngCore};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, SerialNumber,
};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::{self, ServerConfig};
use tokio_rustls::TlsAcceptor;

const CERT: &str = "cert.pem";
const KEY: &str = "key.pem";
const CA: &str = "ca.crt";
const CA_SERIAL: &str = "ca-serial.txt";

pub fn load_acceptor(dir: &Path) -> Result<Option<TlsAcceptor>, String> {
    let (cert_path, key_path) = (dir.join(CERT), dir.join(KEY));
    if !cert_path.exists() || !key_path.exists() {
        return Ok(None);
    }
    let cert_pem = fs::read(&cert_path).map_err(|e| e.to_string())?;
    let key_pem = fs::read(&key_path).map_err(|e| e.to_string())?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cert.pem: {e:?}"))?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| format!("key.pem: {e:?}"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

pub fn setup(dir: &Path, device_id: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let err = |e: rcgen::Error| e.to_string();

    let mut serial = [0u8; 16];
    OsRng.fill_bytes(&mut serial);
    serial[0] &= 0x7f; // keep it a positive integer

    let mut ca = CertificateParams::new(Vec::<String>::new()).map_err(err)?;
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.serial_number = Some(SerialNumber::from(serial.to_vec()));
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("Weighbridge Gateway Local CA ({device_id})"));
    ca.distinguished_name = dn;
    let ca_key = KeyPair::generate().map_err(err)?;
    let ca_cert = ca.self_signed(&ca_key).map_err(err)?;

    let mut leaf = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()]).map_err(err)?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Weighbridge Gateway (localhost)");
    leaf.distinguished_name = dn;
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_key = KeyPair::generate().map_err(err)?;
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).map_err(err)?;
    drop(ca_key); // never persisted

    // Remove any previously installed CA from this gateway before trusting the new one.
    let _ = untrust(dir);

    fs::write(dir.join(CERT), format!("{}{}", leaf_cert.pem(), ca_cert.pem())).map_err(|e| e.to_string())?;
    fs::write(dir.join(KEY), leaf_key.serialize_pem()).map_err(|e| e.to_string())?;
    fs::write(dir.join(CA), ca_cert.pem()).map_err(|e| e.to_string())?;
    fs::write(dir.join(CA_SERIAL), hex::encode(serial)).map_err(|e| e.to_string())?;

    certutil(&["-user", "-addstore", "Root", &dir.join(CA).display().to_string()])
        .map_err(|e| format!("Certificate created but not trusted: {e}"))
}

pub fn remove(dir: &Path) -> Result<(), String> {
    let result = untrust(dir);
    for f in [CERT, KEY, CA, CA_SERIAL] {
        let _ = fs::remove_file(dir.join(f));
    }
    result
}

fn untrust(dir: &Path) -> Result<(), String> {
    match fs::read_to_string(dir.join(CA_SERIAL)) {
        Ok(serial) => certutil(&["-user", "-delstore", "Root", serial.trim()]),
        Err(_) => Ok(()),
    }
}

#[cfg(windows)]
fn certutil(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("certutil")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&out.stdout);
        Err(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("certutil failed").trim().to_string())
    }
}

#[cfg(not(windows))]
fn certutil(_args: &[&str]) -> Result<(), String> {
    Err("Automatic trust is only implemented on Windows; trust ca.crt manually".into())
}
