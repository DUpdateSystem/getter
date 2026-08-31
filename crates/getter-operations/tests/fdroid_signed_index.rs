use std::io::{Cursor, Read, Write};

use getter_operations::fdroid_signed_index::{verify_fdroid_index_jar, VerifyError};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const FIXTURE: &[u8] = include_bytes!("fixtures/fdroid-official-index.jar");
const WRONG_CERT_FIXTURE: &[u8] = include_bytes!("fixtures/wrong-cert-index.jar");

#[test]
fn verifies_official_fdroid_signed_index() {
    let xml = verify_fdroid_index_jar(FIXTURE).expect("official fixture should verify");
    assert!(xml.starts_with("<?xml"));
    assert!(xml.contains("<repo"));
}

#[test]
fn rejects_xml_mutation() {
    let jar = rewrite(|name, bytes| {
        if name == "index.xml" {
            bytes[100] ^= 1;
        }
    });
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::DigestMismatch("index.xml"))
    ));
}

#[test]
fn rejects_manifest_mutation() {
    let jar = rewrite(|name, bytes| {
        if name == "META-INF/MANIFEST.MF" {
            let offset = bytes.iter().position(|byte| *byte == b'J').unwrap();
            bytes[offset] = b'K';
        }
    });
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::DigestMismatch("MANIFEST.MF"))
    ));
}

#[test]
fn rejects_signature_file_mutation() {
    let jar = rewrite(|name, bytes| {
        if name.ends_with(".SF") {
            bytes[20] ^= 1;
        }
    });
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::SignatureNotVerified)
    ));
}

#[test]
fn rejects_valid_signature_from_unpinned_certificate() {
    assert!(matches!(
        verify_fdroid_index_jar(WRONG_CERT_FIXTURE),
        Err(VerifyError::WrongCertificate)
    ));
}

#[test]
fn rejects_signature_or_certificate_mutation() {
    let jar = rewrite(|name, bytes| {
        if name.ends_with(".RSA") {
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
        }
    });
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::InvalidSignature(_)) | Err(VerifyError::SignatureNotVerified)
    ));
}

#[test]
fn rejects_duplicate_entry() {
    let jar = append_duplicate_index();
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::DuplicateEntry(_))
    ));
}

#[test]
fn rejects_oversize_entry() {
    let jar = replace_entry("index.xml", &vec![b'x'; 48 * 1024 * 1024 + 1]);
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::EntryTooLarge(name)) if name == "index.xml"
    ));
}

#[test]
fn rejects_unsafe_entry() {
    let jar = append_entry("../index.xml", b"unsafe");
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::UnsafeEntry(name)) if name == "../index.xml"
    ));
}

#[test]
fn rejects_unexpected_entry() {
    let jar = append_entry("extra.txt", b"unexpected");
    assert!(matches!(
        verify_fdroid_index_jar(&jar),
        Err(VerifyError::UnexpectedEntry(_))
    ));
}

#[test]
fn rejects_outer_size_limit_before_zip_processing() {
    let oversized = vec![0; 64 * 1024 * 1024 + 1];
    assert!(matches!(
        verify_fdroid_index_jar(&oversized),
        Err(VerifyError::ArchiveTooLarge)
    ));
}

fn entries() -> Vec<(String, Vec<u8>)> {
    let mut archive = ZipArchive::new(Cursor::new(FIXTURE)).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect()
}

fn build(entries: impl IntoIterator<Item = (String, Vec<u8>)>) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut output);
        for (name, bytes) in entries {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    output.into_inner()
}

fn rewrite(mut mutation: impl FnMut(&str, &mut Vec<u8>)) -> Vec<u8> {
    build(entries().into_iter().map(|(name, mut bytes)| {
        mutation(&name, &mut bytes);
        (name, bytes)
    }))
}

fn append_entry(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut all = entries();
    all.push((name.to_owned(), bytes.to_vec()));
    build(all)
}

fn append_duplicate_index() -> Vec<u8> {
    let mut jar = append_entry("dupe0.xml", b"duplicate");
    let needle = b"dupe0.xml";
    let replacement = b"index.xml";
    let mut matches = Vec::new();
    for offset in 0..=jar.len() - needle.len() {
        if jar[offset..].starts_with(needle) {
            matches.push(offset);
        }
    }
    assert_eq!(matches.len(), 2);
    for offset in matches {
        jar[offset..offset + needle.len()].copy_from_slice(replacement);
    }
    jar
}

fn replace_entry(name: &str, replacement: &[u8]) -> Vec<u8> {
    build(entries().into_iter().map(|(entry_name, bytes)| {
        if entry_name == name {
            (entry_name, replacement.to_vec())
        } else {
            (entry_name, bytes)
        }
    }))
}
