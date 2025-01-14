use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    str::FromStr,
};

use age::{x25519, DecryptError, Decryptor, Identity};
use sha2::{Digest, Sha256};
use test_case::test_case;

#[test_case("header_crlf")]
#[test_case("hmac_bad")]
#[test_case("hmac_extra_space")]
#[test_case("hmac_garbage")]
#[test_case("hmac_missing")]
#[test_case("hmac_no_space")]
#[test_case("hmac_not_canonical")]
#[test_case("hmac_trailing_space")]
#[test_case("hmac_truncated")]
#[test_case("scrypt")]
#[test_case("scrypt_and_x25519")]
#[test_case("scrypt_bad_tag")]
#[test_case("scrypt_double")]
#[test_case("scrypt_extra_argument")]
#[test_case("scrypt_long_file_key")]
#[test_case("scrypt_no_match")]
#[test_case("scrypt_not_canonical_body")]
#[test_case("scrypt_not_canonical_salt")]
#[test_case("scrypt_salt_long")]
#[test_case("scrypt_salt_missing")]
#[test_case("scrypt_salt_short")]
#[test_case("scrypt_uppercase")]
#[test_case("scrypt_work_factor_23")]
#[test_case("scrypt_work_factor_hex")]
#[test_case("scrypt_work_factor_leading_garbage")]
#[test_case("scrypt_work_factor_leading_plus")]
#[test_case("scrypt_work_factor_leading_zero_decimal")]
#[test_case("scrypt_work_factor_leading_zero_octal")]
#[test_case("scrypt_work_factor_missing")]
#[test_case("scrypt_work_factor_negative")]
#[test_case("scrypt_work_factor_overflow")]
#[test_case("scrypt_work_factor_trailing_garbage")]
#[test_case("scrypt_work_factor_wrong")]
#[test_case("scrypt_work_factor_zero")]
#[test_case("stanza_bad_start")]
#[test_case("stanza_base64_padding")]
#[test_case("stanza_empty_argument")]
#[test_case("stanza_empty_body")]
#[test_case("stanza_empty_last_line")]
#[test_case("stanza_invalid_character")]
#[test_case("stanza_long_line")]
#[test_case("stanza_missing_body")]
#[test_case("stanza_missing_final_line")]
#[test_case("stanza_multiple_short_lines")]
#[test_case("stanza_no_arguments")]
#[test_case("stanza_not_canonical")]
#[test_case("stanza_spurious_cr")]
#[test_case("stanza_valid_characters")]
#[test_case("stream_bad_tag")]
#[test_case("stream_bad_tag_second_chunk")]
#[test_case("stream_bad_tag_second_chunk_full")]
#[test_case("stream_empty_payload")]
#[test_case("stream_last_chunk_empty")]
#[test_case("stream_last_chunk_full")]
#[test_case("stream_last_chunk_full_second")]
#[test_case("stream_missing_tag")]
#[test_case("stream_no_chunks")]
#[test_case("stream_no_final")]
#[test_case("stream_no_final_full")]
#[test_case("stream_no_final_two_chunks")]
#[test_case("stream_no_final_two_chunks_full")]
#[test_case("stream_no_nonce")]
#[test_case("stream_short_chunk")]
#[test_case("stream_short_nonce")]
#[test_case("stream_short_second_chunk")]
#[test_case("stream_three_chunks")]
#[test_case("stream_trailing_garbage_long")]
#[test_case("stream_trailing_garbage_short")]
#[test_case("stream_two_chunks")]
#[test_case("stream_two_final_chunks")]
#[test_case("version_unsupported")]
#[test_case("x25519")]
#[test_case("x25519_bad_tag")]
#[test_case("x25519_extra_argument")]
#[test_case("x25519_grease")]
#[test_case("x25519_identity")]
#[test_case("x25519_long_file_key")]
#[test_case("x25519_long_share")]
#[test_case("x25519_lowercase")]
#[test_case("x25519_low_order")]
#[test_case("x25519_multiple_recipients")]
#[test_case("x25519_no_match")]
#[test_case("x25519_not_canonical_body")]
#[test_case("x25519_not_canonical_share")]
#[test_case("x25519_short_share")]
fn testkit(filename: &str) {
    let testfile = TestFile::parse(filename);
    let comment = testfile
        .comment
        .map(|c| format!(" ({})", c))
        .unwrap_or_default();

    match Decryptor::new(&testfile.age_file[..]).and_then(|d| match d {
        Decryptor::Recipients(d) => {
            assert_eq!(
                testfile.passphrases.len(),
                // `scrypt_uppercase` uses the stanza tag `Scrypt` instead of `scrypt`, so
                // even though there is a valid passphrase, the decryptor treats it as a
                // different recipient stanza kind.
                if filename == "scrypt_uppercase" { 1 } else { 0 }
            );
            let identities: Vec<x25519::Identity> = testfile
                .identities
                .iter()
                .map(|s| s.as_str())
                .map(x25519::Identity::from_str)
                .collect::<Result<_, _>>()
                .unwrap();
            d.decrypt(identities.iter().map(|i| i as &dyn Identity))
        }
        Decryptor::Passphrase(d) => {
            assert_eq!(testfile.identities.len(), 0);
            match testfile.passphrases.len() {
                0 => panic!("Test file is missing passphrase{}", comment),
                1 => d.decrypt(
                    &testfile.passphrases.get(0).cloned().unwrap().into(),
                    Some(16),
                ),
                n => panic!("Too many passphrases ({}){}", n, comment),
            }
        }
    }) {
        Ok(mut r) => {
            let mut payload = vec![];
            let res = r.read_to_end(&mut payload);
            match (res, testfile.expect) {
                (Ok(_), Expect::Success { payload_sha256 }) => {
                    assert_eq!(Sha256::digest(&payload)[..], payload_sha256);
                }
                // These testfile failures are expected, because we maintains support for
                // parsing legacy age stanzas without an explicit short final line.
                (Ok(_), Expect::HeaderFailure)
                    if ["stanza_missing_body", "stanza_missing_final_line"].contains(&filename) => {
                }
                (Err(e), Expect::PayloadFailure { payload_sha256 }) => {
                    assert_eq!(
                        e.kind(),
                        if [
                            "stream_no_chunks",
                            "stream_no_final_full",
                            "stream_no_final_two_chunks_full",
                            "stream_trailing_garbage_long",
                            "stream_trailing_garbage_short",
                            "stream_two_final_chunks",
                        ]
                        .contains(&filename)
                        {
                            io::ErrorKind::UnexpectedEof
                        } else {
                            io::ErrorKind::InvalidData
                        }
                    );
                    // The tests with this expectation are checking that no partial STREAM
                    // blocks are written to the payload.
                    assert_eq!(Sha256::digest(&payload)[..], payload_sha256);
                }
                (actual, expected) => panic!(
                    "Expected {:?}, got {}{}",
                    expected,
                    if actual.is_ok() {
                        format!("payload '{}'", String::from_utf8_lossy(&payload))
                    } else {
                        format!("{:?}", actual)
                    },
                    comment,
                ),
            }
        }
        Err(e) => match e {
            DecryptError::ExcessiveWork { .. }
            | DecryptError::InvalidHeader
            | DecryptError::Io(_)
            | DecryptError::UnknownFormat => {
                assert_eq!(testfile.expect, Expect::HeaderFailure)
            }
            DecryptError::InvalidMac => assert_eq!(testfile.expect, Expect::HmacFailure),
            DecryptError::DecryptionFailed | DecryptError::NoMatchingKeys => {
                assert_eq!(testfile.expect, Expect::NoMatch)
            }
            DecryptError::KeyDecryptionFailed => todo!(),
            #[cfg(feature = "plugin")]
            DecryptError::MissingPlugin { .. } => todo!(),
            #[cfg(feature = "plugin")]
            DecryptError::Plugin(_) => todo!(),
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Expect {
    Success { payload_sha256: [u8; 32] },
    HeaderFailure,
    HmacFailure,
    NoMatch,
    PayloadFailure { payload_sha256: [u8; 32] },
}

struct TestFile {
    expect: Expect,
    identities: Vec<String>,
    passphrases: Vec<String>,
    comment: Option<String>,
    age_file: Vec<u8>,
}

impl TestFile {
    fn parse(filename: &str) -> Self {
        let file = File::open(format!("./tests/testdata/testkit/{}", filename)).unwrap();
        let mut r = BufReader::new(file);
        let mut line = String::new();

        fn data<'l>(line: &'l str, prefix: &str) -> &'l str {
            line.strip_prefix(prefix).unwrap().trim()
        }

        let expect = {
            r.read_line(&mut line).unwrap();
            match data(&line, "expect:") {
                "success" => {
                    line.clear();
                    r.read_line(&mut line).unwrap();
                    let payload = data(&line, "payload:");
                    Expect::Success {
                        payload_sha256: hex::decode(payload).unwrap().try_into().unwrap(),
                    }
                }
                "header failure" => Expect::HeaderFailure,
                "payload failure" => {
                    line.clear();
                    r.read_line(&mut line).unwrap();
                    let payload = data(&line, "payload:");
                    Expect::PayloadFailure {
                        payload_sha256: hex::decode(payload).unwrap().try_into().unwrap(),
                    }
                }
                "HMAC failure" => Expect::HmacFailure,
                "no match" => Expect::NoMatch,
                e => panic!("Unknown testkit failure '{}'", e),
            }
        };

        let _file_key = {
            line.clear();
            r.read_line(&mut line).unwrap();
            hex::decode(data(&line, "file key: ")).unwrap()
        };

        let mut identities = vec![];
        let mut passphrases = vec![];
        let mut comment = None;
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }

            let (prefix, data) = line.trim().split_once(": ").unwrap();
            match prefix {
                "identity" => identities.push(data.to_owned()),
                "passphrase" => passphrases.push(data.to_owned()),
                "comment" => comment = Some(data.to_owned()),
                _ => panic!("Unknown testkit metadata '{}'", prefix),
            }
        }

        let mut age_file = vec![];
        r.read_to_end(&mut age_file).unwrap();

        Self {
            expect,
            identities,
            passphrases,
            comment,
            age_file,
        }
    }
}
