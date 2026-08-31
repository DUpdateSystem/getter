//! Verification of the official F-Droid v1 signed `index.jar`.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use base64::Engine as _;
use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::{ObjectIdentifier, OctetString};
use der::{Decode, Encode};
use rsa::pkcs1v15::{Signature as RsaSignature, VerifyingKey};
use rsa::RsaPublicKey;
use sha2::{Digest, Sha256};
use sha2_11::Sha256 as RsaSha256;
use signature::Verifier;
use spki::DecodePublicKey;
use subtle::ConstantTimeEq;
use thiserror::Error;
use zip::ZipArchive;

pub const OFFICIAL_INDEX_JAR_URL: &str = "https://f-droid.org/repo/index.jar";
pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 48 * 1024 * 1024;
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 52 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_FILE_BYTES: u64 = 64 * 1024;
const MAX_SIGNATURE_BLOCK_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 4;
const OFFICIAL_CERT_SHA256: [u8; 32] = [
    0x43, 0x23, 0x8d, 0x51, 0x2c, 0x1e, 0x5e, 0xb2, 0xd6, 0x56, 0x9f, 0x4a, 0x3a, 0xfb, 0xf5, 0x52,
    0x34, 0x18, 0xb8, 0x2e, 0x0a, 0x3e, 0xd1, 0x55, 0x27, 0x70, 0xab, 0xb9, 0xa9, 0xc9, 0xcc, 0xab,
];

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("F-Droid index.jar exceeds the {MAX_ARCHIVE_BYTES}-byte limit")]
    ArchiveTooLarge,
    #[error("invalid ZIP archive: {0}")]
    InvalidZip(String),
    #[error("unsafe ZIP entry name: {0}")]
    UnsafeEntry(String),
    #[error("duplicate ZIP entry: {0}")]
    DuplicateEntry(String),
    #[error("unexpected ZIP entry: {0}")]
    UnexpectedEntry(String),
    #[error("missing required ZIP entry: {0}")]
    MissingEntry(&'static str),
    #[error("ZIP entry exceeds its size limit: {0}")]
    EntryTooLarge(String),
    #[error("invalid JAR manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid PKCS#7 signature block: {0}")]
    InvalidSignature(String),
    #[error("JAR v1 signature was not verified")]
    SignatureNotVerified,
    #[error("the signing certificate is not the pinned official F-Droid certificate")]
    WrongCertificate,
    #[error("digest mismatch for {0}")]
    DigestMismatch(&'static str),
    #[error("index.xml is not UTF-8")]
    InvalidXmlUtf8,
}

struct JarParts {
    xml: Vec<u8>,
    manifest: Vec<u8>,
    sf: Vec<u8>,
    rsa: Vec<u8>,
}

/// Verifies an already bounded official F-Droid `index.jar` and returns its XML.
pub fn verify_fdroid_index_jar(jar: &[u8]) -> Result<String, VerifyError> {
    if jar.len() > MAX_ARCHIVE_BYTES {
        return Err(VerifyError::ArchiveTooLarge);
    }
    let parts = read_strict_jar(jar)?;

    verify_cms_signature(&parts.rsa, &parts.sf)?;

    let sf = ParsedManifest::parse(&parts.sf)?;
    sf.require_exact("Signature-Version", "1.0")?;
    sf.require_sha256("SHA-256-Digest-Manifest", &parts.manifest, "MANIFEST.MF")?;
    let manifest_section = find_named_section_bytes(&parts.manifest, "index.xml")?;
    sf.section("index.xml")?.require_sha256(
        "SHA-256-Digest",
        manifest_section,
        "index.xml manifest section",
    )?;

    let manifest = ParsedManifest::parse(&parts.manifest)?;
    manifest.require_exact("Manifest-Version", "1.0")?;
    manifest
        .section("index.xml")?
        .require_sha256("SHA-256-Digest", &parts.xml, "index.xml")?;

    String::from_utf8(parts.xml).map_err(|_| VerifyError::InvalidXmlUtf8)
}

fn read_strict_jar(jar: &[u8]) -> Result<JarParts, VerifyError> {
    let declared_entries = eocd_entry_count(jar)?;
    let mut archive = ZipArchive::new(Cursor::new(jar))
        .map_err(|error| VerifyError::InvalidZip(error.to_string()))?;
    if declared_entries != archive.len() {
        return Err(VerifyError::DuplicateEntry(
            "central directory contains duplicate names".to_owned(),
        ));
    }
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    let mut values = HashMap::new();
    let mut signer_stem: Option<String> = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| VerifyError::InvalidZip(error.to_string()))?;
        let name = entry.name().to_owned();
        if !safe_entry_name(&name) {
            return Err(VerifyError::UnsafeEntry(name));
        }
        if !seen.insert(name.clone()) {
            return Err(VerifyError::DuplicateEntry(name));
        }
        let kind_limit = match name.as_str() {
            "index.xml" => MAX_ENTRY_BYTES,
            "META-INF/MANIFEST.MF" => MAX_MANIFEST_BYTES,
            _ if signature_name(&name, ".SF").is_some() => MAX_SIGNATURE_FILE_BYTES,
            _ if signature_name(&name, ".RSA").is_some() => MAX_SIGNATURE_BLOCK_BYTES,
            _ => return Err(VerifyError::UnexpectedEntry(name)),
        };
        if entry.is_dir() || entry.size() > kind_limit {
            return Err(VerifyError::EntryTooLarge(name));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| VerifyError::EntryTooLarge(name.clone()))?;
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(VerifyError::EntryTooLarge(name));
        }
        if let Some(stem) = signature_name(&name, ".SF").or_else(|| signature_name(&name, ".RSA")) {
            match &signer_stem {
                Some(existing) if existing != stem => {
                    return Err(VerifyError::UnexpectedEntry(name));
                }
                None => signer_stem = Some(stem.to_owned()),
                _ => {}
            }
        }
        let capacity =
            usize::try_from(entry.size()).map_err(|_| VerifyError::EntryTooLarge(name.clone()))?;
        let mut bytes = Vec::with_capacity(capacity);
        entry
            .take(kind_limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| VerifyError::InvalidZip(error.to_string()))?;
        if bytes.len() as u64 > kind_limit {
            return Err(VerifyError::EntryTooLarge(name));
        }
        values.insert(name, bytes);
    }

    if seen.len() != MAX_ENTRIES {
        return Err(VerifyError::UnexpectedEntry(format!(
            "archive has {} entries",
            seen.len()
        )));
    }
    let stem = signer_stem.ok_or(VerifyError::MissingEntry("META-INF/<signer>.SF/.RSA"))?;
    Ok(JarParts {
        xml: take_required(&mut values, "index.xml")?,
        manifest: take_required(&mut values, "META-INF/MANIFEST.MF")?,
        sf: take_required(&mut values, &format!("META-INF/{stem}.SF"))?,
        rsa: take_required(&mut values, &format!("META-INF/{stem}.RSA"))?,
    })
}

fn eocd_entry_count(jar: &[u8]) -> Result<usize, VerifyError> {
    const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
    const MIN_EOCD_SIZE: usize = 22;
    let search_start = jar.len().saturating_sub(65_535 + MIN_EOCD_SIZE);
    let relative = jar[search_start..]
        .windows(EOCD_SIGNATURE.len())
        .rposition(|window| window == EOCD_SIGNATURE)
        .ok_or_else(|| {
            VerifyError::InvalidZip("missing end-of-central-directory record".to_owned())
        })?;
    let offset = search_start + relative;
    let eocd = jar.get(offset..offset + MIN_EOCD_SIZE).ok_or_else(|| {
        VerifyError::InvalidZip("truncated end-of-central-directory record".to_owned())
    })?;
    let disk_entries = u16::from_le_bytes([eocd[8], eocd[9]]);
    let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
    let comment_len = usize::from(u16::from_le_bytes([eocd[20], eocd[21]]));
    if offset + MIN_EOCD_SIZE + comment_len != jar.len() {
        return Err(VerifyError::InvalidZip(
            "trailing bytes or malformed ZIP comment".to_owned(),
        ));
    }
    if disk_entries == u16::MAX || total_entries == u16::MAX || disk_entries != total_entries {
        return Err(VerifyError::InvalidZip(
            "multi-disk and ZIP64 archives are not accepted".to_owned(),
        ));
    }
    Ok(usize::from(total_entries))
}

fn take_required(
    values: &mut HashMap<String, Vec<u8>>,
    name: &str,
) -> Result<Vec<u8>, VerifyError> {
    values
        .remove(name)
        .ok_or(VerifyError::MissingEntry(match name {
            "index.xml" => "index.xml",
            "META-INF/MANIFEST.MF" => "META-INF/MANIFEST.MF",
            _ if name.ends_with(".SF") => "META-INF/<signer>.SF",
            _ => "META-INF/<signer>.RSA",
        }))
}

fn safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.starts_with('\\')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !name.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

fn signature_name<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let stem = name.strip_prefix("META-INF/")?.strip_suffix(suffix)?;
    (!stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(stem)
}

#[derive(Debug)]
struct ParsedManifest {
    main: Attributes,
    named: HashMap<String, Attributes>,
}

type Attributes = HashMap<String, String>;

impl ParsedManifest {
    fn parse(bytes: &[u8]) -> Result<Self, VerifyError> {
        let sections = split_sections(bytes)?;
        let mut iter = sections.into_iter();
        let main = parse_attributes(iter.next().ok_or_else(|| {
            VerifyError::InvalidManifest("manifest has no main section".to_owned())
        })?)?;
        let mut named = HashMap::new();
        for section in iter {
            let mut attributes = parse_attributes(section)?;
            let name = attributes.remove("Name").ok_or_else(|| {
                VerifyError::InvalidManifest("named section has no Name attribute".to_owned())
            })?;
            if named.insert(name.clone(), attributes).is_some() {
                return Err(VerifyError::InvalidManifest(format!(
                    "duplicate section for {name}"
                )));
            }
        }
        Ok(Self { main, named })
    }

    fn section(&self, name: &str) -> Result<&Attributes, VerifyError> {
        self.named
            .get(name)
            .ok_or_else(|| VerifyError::InvalidManifest(format!("missing section for {name}")))
    }

    fn require_sha256(
        &self,
        key: &str,
        bytes: &[u8],
        subject: &'static str,
    ) -> Result<(), VerifyError> {
        self.main.require_sha256(key, bytes, subject)
    }

    fn require_exact(&self, key: &str, expected: &str) -> Result<(), VerifyError> {
        if self.main.get(key).is_some_and(|value| value == expected) {
            Ok(())
        } else {
            Err(VerifyError::InvalidManifest(format!(
                "missing or unsupported {key}"
            )))
        }
    }
}

trait DigestAttribute {
    fn require_sha256(
        &self,
        key: &str,
        bytes: &[u8],
        subject: &'static str,
    ) -> Result<(), VerifyError>;
}

impl DigestAttribute for Attributes {
    fn require_sha256(
        &self,
        key: &str,
        bytes: &[u8],
        subject: &'static str,
    ) -> Result<(), VerifyError> {
        let encoded = self
            .get(key)
            .ok_or_else(|| VerifyError::InvalidManifest(format!("missing {key}")))?;
        let expected = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| VerifyError::InvalidManifest(format!("invalid base64 in {key}")))?;
        if expected.len() != 32 {
            return Err(VerifyError::InvalidManifest(format!(
                "{key} is not a SHA-256 digest"
            )));
        }
        let actual = Sha256::digest(bytes);
        if !bool::from(expected.as_slice().ct_eq(actual.as_slice())) {
            return Err(VerifyError::DigestMismatch(subject));
        }
        Ok(())
    }
}

fn split_sections(bytes: &[u8]) -> Result<Vec<&[u8]>, VerifyError> {
    if bytes.is_empty() {
        return Err(VerifyError::InvalidManifest("empty manifest".to_owned()));
    }
    let mut sections = Vec::new();
    let mut start = 0;
    let mut position = 0;
    while position < bytes.len() {
        let line_end = bytes[position..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|relative| position + relative + 1)
            .unwrap_or(bytes.len());
        let line = &bytes[position..line_end];
        let content = line
            .strip_suffix(b"\r\n")
            .or_else(|| line.strip_suffix(b"\n"))
            .unwrap_or(line);
        if content.is_empty() {
            sections.push(&bytes[start..line_end]);
            start = line_end;
        }
        position = line_end;
    }
    if start < bytes.len() {
        sections.push(&bytes[start..]);
    }
    Ok(sections)
}

fn parse_attributes(section: &[u8]) -> Result<Attributes, VerifyError> {
    let text = std::str::from_utf8(section)
        .map_err(|_| VerifyError::InvalidManifest("attributes are not UTF-8".to_owned()))?;
    let mut unfolded: Vec<String> = Vec::new();
    for raw in text.split_terminator('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(continuation) = line.strip_prefix(' ') {
            let previous = unfolded.last_mut().ok_or_else(|| {
                VerifyError::InvalidManifest("orphan continuation line".to_owned())
            })?;
            previous.push_str(continuation);
        } else {
            unfolded.push(line.to_owned());
        }
    }
    let mut attributes = HashMap::new();
    for line in unfolded {
        let (key, value) = line
            .split_once(": ")
            .ok_or_else(|| VerifyError::InvalidManifest(format!("malformed attribute: {line}")))?;
        if key.is_empty()
            || attributes
                .insert(key.to_owned(), value.to_owned())
                .is_some()
        {
            return Err(VerifyError::InvalidManifest(format!(
                "empty or duplicate attribute: {key}"
            )));
        }
    }
    Ok(attributes)
}

fn find_named_section_bytes<'a>(manifest: &'a [u8], wanted: &str) -> Result<&'a [u8], VerifyError> {
    let mut found = None;
    for section in split_sections(manifest)?.into_iter().skip(1) {
        let attributes = parse_attributes(section)?;
        if attributes.get("Name").is_some_and(|name| name == wanted)
            && found.replace(section).is_some()
        {
            return Err(VerifyError::InvalidManifest(format!(
                "duplicate section for {wanted}"
            )));
        }
    }
    found.ok_or_else(|| VerifyError::InvalidManifest(format!("missing section for {wanted}")))
}

fn verify_cms_signature(signature_block: &[u8], signature_file: &[u8]) -> Result<(), VerifyError> {
    const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
    const DATA: &str = "1.2.840.113549.1.7.1";
    const SHA256: &str = "2.16.840.1.101.3.4.2.1";
    const RSA_ENCRYPTION: &str = "1.2.840.113549.1.1.1";
    const SHA256_WITH_RSA: &str = "1.2.840.113549.1.1.11";
    const CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
    const MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";

    let content_info = ContentInfo::from_der(signature_block).map_err(invalid_der)?;
    if content_info.content_type != oid(SIGNED_DATA)? {
        return invalid_signature("CMS ContentInfo is not SignedData");
    }
    let signed_data_der = content_info.content.to_der().map_err(invalid_der)?;
    let signed_data = SignedData::from_der(&signed_data_der).map_err(invalid_der)?;
    if signed_data.signer_infos.0.len() != 1 {
        return invalid_signature("CMS SignedData must contain exactly one signer");
    }
    let signer = signed_data
        .signer_infos
        .0
        .get(0)
        .ok_or(VerifyError::SignatureNotVerified)?;
    let signature_oid = signer.signature_algorithm.oid;
    if signer.digest_alg.oid != oid(SHA256)?
        || (signature_oid != oid(RSA_ENCRYPTION)? && signature_oid != oid(SHA256_WITH_RSA)?)
    {
        return invalid_signature("unsupported CMS signature algorithm; expected SHA256withRSA");
    }

    let certificates = signed_data.certificates.as_ref().ok_or_else(|| {
        VerifyError::InvalidSignature("signature block has no certificate".into())
    })?;
    let matching: Vec<_> = certificates
        .0
        .iter()
        .filter_map(|choice| match choice {
            CertificateChoices::Certificate(cert) if signer_matches_certificate(signer, cert) => {
                Some(cert)
            }
            _ => None,
        })
        .collect();
    if matching.len() != 1 {
        return invalid_signature("CMS signer must match exactly one certificate");
    }
    let certificate = matching[0];
    let certificate_der = certificate.to_der().map_err(invalid_der)?;
    if !bool::from(
        Sha256::digest(&certificate_der)
            .as_slice()
            .ct_eq(&OFFICIAL_CERT_SHA256),
    ) {
        return Err(VerifyError::WrongCertificate);
    }

    let signed_content = if let Some(attrs) = signer.signed_attrs.as_ref() {
        let content_type_der = single_attribute_value(attrs, CONTENT_TYPE)?
            .to_der()
            .map_err(invalid_der)?;
        if ObjectIdentifier::from_der(&content_type_der).map_err(invalid_der)? != oid(DATA)? {
            return Err(VerifyError::SignatureNotVerified);
        }
        let message_digest_der = single_attribute_value(attrs, MESSAGE_DIGEST)?
            .to_der()
            .map_err(invalid_der)?;
        let message_digest = OctetString::from_der(&message_digest_der).map_err(invalid_der)?;
        if !bool::from(
            Sha256::digest(signature_file)
                .as_slice()
                .ct_eq(message_digest.as_bytes()),
        ) {
            return Err(VerifyError::SignatureNotVerified);
        }

        // RFC 5652 section 5.4 signs the DER SET OF SignedAttrs, not the
        // context-specific [0] IMPLICIT encoding used inside SignerInfo.
        attrs.to_der().map_err(invalid_der)?
    } else {
        // The official F-Droid JAR signature is a detached CMS signature
        // without signed attributes, so its signature covers the exact .SF.
        signature_file.to_vec()
    };

    let public_key_der = certificate
        .tbs_certificate()
        .subject_public_key_info()
        .to_der()
        .map_err(invalid_der)?;
    let public_key = RsaPublicKey::from_public_key_der(&public_key_der)
        .map_err(|error| VerifyError::InvalidSignature(error.to_string()))?;
    let signature = RsaSignature::try_from(signer.signature.as_bytes())
        .map_err(|_| VerifyError::SignatureNotVerified)?;
    VerifyingKey::<RsaSha256>::new(public_key)
        .verify(&signed_content, &signature)
        .map_err(|_| VerifyError::SignatureNotVerified)
}

fn signer_matches_certificate(
    signer: &cms::signed_data::SignerInfo,
    certificate: &x509_cert::Certificate,
) -> bool {
    match &signer.sid {
        SignerIdentifier::IssuerAndSerialNumber(id) => {
            id.issuer == *certificate.tbs_certificate().issuer()
                && id.serial_number == *certificate.tbs_certificate().serial_number()
        }
        SignerIdentifier::SubjectKeyIdentifier(_) => false,
    }
}

fn single_attribute_value<'a>(
    attrs: &'a cms::signed_data::SignedAttributes,
    wanted: &str,
) -> Result<&'a der::Any, VerifyError> {
    let wanted = oid(wanted)?;
    let mut matches = attrs.iter().filter(|attribute| attribute.oid == wanted);
    let attribute = matches.next().ok_or(VerifyError::SignatureNotVerified)?;
    if matches.next().is_some() || attribute.values.len() != 1 {
        return Err(VerifyError::SignatureNotVerified);
    }
    attribute
        .values
        .get(0)
        .ok_or(VerifyError::SignatureNotVerified)
}

fn oid(value: &str) -> Result<ObjectIdentifier, VerifyError> {
    ObjectIdentifier::new(value).map_err(|error| VerifyError::InvalidSignature(error.to_string()))
}

fn invalid_der(error: der::Error) -> VerifyError {
    VerifyError::InvalidSignature(error.to_string())
}

fn invalid_signature<T>(message: &str) -> Result<T, VerifyError> {
    Err(VerifyError::InvalidSignature(message.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{find_named_section_bytes, ParsedManifest};

    #[test]
    fn parses_crlf_and_continuation_lines() {
        let manifest = ParsedManifest::parse(
            b"Manifest-Version: 1.0\r\nLong: first\r\n second\r\n\r\nName: index.xml\r\nSHA-256-Digest: abc\r\n def\r\n\r\n",
        )
        .unwrap();
        assert_eq!(manifest.main.get("Long").unwrap(), "firstsecond");
        assert_eq!(
            manifest
                .section("index.xml")
                .unwrap()
                .get("SHA-256-Digest")
                .unwrap(),
            "abcdef"
        );
    }

    #[test]
    fn retains_exact_named_section_bytes_with_lf_endings() {
        let bytes = b"Manifest-Version: 1.0\n\nName: index.xml\nSHA-256-Digest: abc\n def\n\n";
        assert_eq!(
            find_named_section_bytes(bytes, "index.xml").unwrap(),
            b"Name: index.xml\nSHA-256-Digest: abc\n def\n\n"
        );
    }
}
