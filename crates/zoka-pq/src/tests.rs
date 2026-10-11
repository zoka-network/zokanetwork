use super::*;
use ml_kem::Decapsulate;
use serde_json::Value;

fn decode(case: &Value, field: &str) -> Vec<u8> {
    hex::decode(
        case[field]
            .as_str()
            .unwrap_or_else(|| panic!("missing {field}")),
    )
    .unwrap()
}

fn fixtures(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
}

#[test]
fn mlkem_roundtrip_export_import_and_fresh_randomness() {
    let key = MlKem768SecretKey::generate().unwrap();
    let public = MlKem768PublicKey::from_bytes(&key.public_key().to_bytes()).unwrap();
    let restored = MlKem768SecretKey::from_seed(key.export_seed().expose_secret()).unwrap();
    let (ciphertext, sent) = public.encapsulate();
    let parsed = MlKem768Ciphertext::from_bytes(ciphertext.as_bytes()).unwrap();
    assert_eq!(
        sent.expose_secret(),
        restored.decapsulate(&parsed).expose_secret()
    );
    let (other, _) = public.encapsulate();
    assert_ne!(ciphertext.as_bytes(), other.as_bytes());
}

#[test]
fn mlkem_implicit_rejection_for_wrong_key_and_tampering() {
    let key = MlKem768SecretKey::from_seed(&[41; 64]).unwrap();
    let wrong_key = MlKem768SecretKey::from_seed(&[42; 64]).unwrap();
    let (ct, sent) = key.public_key().encapsulate();
    assert_ne!(
        sent.expose_secret(),
        wrong_key.decapsulate(&ct).expose_secret()
    );
    let mut altered = ct.as_bytes().to_vec();
    altered[17] ^= 1;
    let altered = MlKem768Ciphertext::from_bytes(&altered).unwrap();
    let rejected = key.decapsulate(&altered);
    assert_ne!(sent.expose_secret(), rejected.expose_secret());
    assert_eq!(
        rejected.expose_secret(),
        key.decapsulate(&altered).expose_secret()
    );
}

#[test]
fn kem_parsers_reject_wrong_sizes_and_noncanonical_mlkem_coefficients() {
    for length in [0, 63, 65] {
        assert!(MlKem768SecretKey::from_seed(&vec![0; length]).is_err());
    }
    for length in [0, 1183, 1185] {
        assert!(MlKem768PublicKey::from_bytes(&vec![0; length]).is_err());
    }
    for length in [0, 1087, 1089] {
        assert!(MlKem768Ciphertext::from_bytes(&vec![0; length]).is_err());
    }
    for length in [0, 31, 33] {
        assert!(XWingSecretKey::from_seed(&vec![0; length]).is_err());
    }
    for length in [0, 1215, 1217] {
        assert!(XWingPublicKey::from_bytes(&vec![0; length]).is_err());
    }
    for length in [0, 1119, 1121] {
        assert!(XWingCiphertext::from_bytes(&vec![0; length]).is_err());
    }
    let mut kem = MlKem768SecretKey::from_seed(&[4; 64])
        .unwrap()
        .public_key()
        .to_bytes();
    // First packed 12-bit coefficient becomes 4095, exceeding modulus 3329.
    kem[0] = 0xff;
    kem[1] |= 0x0f;
    assert!(matches!(
        MlKem768PublicKey::from_bytes(&kem),
        Err(Error::InvalidPublicKey)
    ));
    let mut xwing = XWingSecretKey::from_seed(&[4; 32])
        .unwrap()
        .public_key()
        .to_bytes();
    xwing[0] = 0xff;
    xwing[1] |= 0x0f;
    assert!(matches!(
        XWingPublicKey::from_bytes(&xwing),
        Err(Error::InvalidPublicKey)
    ));
}

#[test]
fn xwing_roundtrip_wrong_key_and_both_components_tampered() {
    let key = XWingSecretKey::generate().unwrap();
    let restored = XWingSecretKey::from_seed(key.export_seed().expose_secret()).unwrap();
    let public = XWingPublicKey::from_bytes(&key.public_key().to_bytes()).unwrap();
    let (ct, sent) = public.encapsulate();
    let parsed = XWingCiphertext::from_bytes(ct.as_bytes()).unwrap();
    assert_eq!(
        sent.expose_secret(),
        restored.decapsulate(&parsed).unwrap().expose_secret()
    );
    let wrong = XWingSecretKey::from_seed(&[90; 32]).unwrap();
    assert_ne!(
        sent.expose_secret(),
        wrong.decapsulate(&ct).unwrap().expose_secret()
    );
    for offset in [0, ML_KEM_768_CIPHERTEXT_BYTES] {
        let mut altered = ct.as_bytes().to_vec();
        altered[offset] ^= 1;
        let altered = XWingCiphertext::from_bytes(&altered).unwrap();
        assert_ne!(
            sent.expose_secret(),
            key.decapsulate(&altered).unwrap().expose_secret()
        );
    }
}

#[test]
fn xwing_profile_rejects_noncontributory_ciphertexts_at_decapsulation() {
    let key = XWingSecretKey::from_seed(&[12; 32]).unwrap();
    let (ct, _) = key.public_key().encapsulate();
    // u=0 and u=1 are both small-order X25519 inputs.
    for first in [0, 1] {
        let mut altered = ct.as_bytes().to_vec();
        altered[ML_KEM_768_CIPHERTEXT_BYTES..].fill(0);
        altered[ML_KEM_768_CIPHERTEXT_BYTES] = first;
        let altered = XWingCiphertext::from_bytes(&altered).unwrap();
        assert!(matches!(
            key.decapsulate(&altered),
            Err(Error::NonContributoryCiphertext)
        ));
    }
    // Public key parsing deliberately retains RFC 7748 byte decoding. It does
    // not prove possession or reject all low-order public keys at encapsulation.
    let mut public = key.public_key().to_bytes();
    public[ML_KEM_768_PUBLIC_KEY_BYTES..].fill(0);
    let public = XWingPublicKey::from_bytes(&public).unwrap();
    let _ = public.encapsulate();
}

#[test]
fn mldsa_roundtrip_and_message_context_key_signature_binding() {
    let key = MlDsa65SecretKey::generate().unwrap();
    let restored = MlDsa65SecretKey::from_seed(key.export_seed().expose_secret()).unwrap();
    let public = MlDsa65PublicKey::from_bytes(&key.public_key().to_bytes()).unwrap();
    let msg = b"synthetic snapshot manifest";
    let context = b"zoka-pq-experiment";
    let sig = restored.sign(msg, context).unwrap();
    let parsed = MlDsa65Signature::from_bytes(&sig.to_bytes()).unwrap();
    public.verify(msg, context, &parsed).unwrap();
    assert!(public
        .verify(b"different snapshot", context, &parsed)
        .is_err());
    assert!(public.verify(msg, b"other-context", &parsed).is_err());
    let wrong = MlDsa65SecretKey::from_seed(&[53; 32]).unwrap().public_key();
    assert!(wrong.verify(msg, context, &parsed).is_err());
    let mut altered = sig.to_bytes();
    altered[0] ^= 1;
    let altered = MlDsa65Signature::from_bytes(&altered).unwrap();
    assert!(public.verify(msg, context, &altered).is_err());
    assert_eq!(
        sig.to_bytes(),
        restored.sign(msg, context).unwrap().to_bytes()
    );
}

#[test]
fn mldsa_encoding_and_context_boundaries() {
    for length in [0, 31, 33] {
        assert!(MlDsa65SecretKey::from_seed(&vec![0; length]).is_err());
    }
    for length in [0, 1951, 1953] {
        assert!(MlDsa65PublicKey::from_bytes(&vec![0; length]).is_err());
    }
    for length in [0, 3308, 3310] {
        assert!(MlDsa65Signature::from_bytes(&vec![0; length]).is_err());
    }
    assert!(MlDsa65Signature::from_bytes(&[0xff; ML_DSA_65_SIGNATURE_BYTES]).is_err());
    let key = MlDsa65SecretKey::from_seed(&[1; 32]).unwrap();
    for context in [vec![], vec![0; 255]] {
        let sig = key.sign(b"", &context).unwrap();
        key.public_key().verify(b"", &context, &sig).unwrap();
    }
    assert!(matches!(
        key.sign(b"", &[0; 256]),
        Err(Error::InvalidContext)
    ));
    let sig = key.sign(b"", b"").unwrap();
    assert_eq!(
        key.public_key().verify(b"", &[0; 256], &sig),
        Err(Error::InvalidContext)
    );
}

#[test]
fn all_secret_debug_output_is_redacted() {
    let kem = MlKem768SecretKey::from_seed(&[17; 64]).unwrap();
    let xwing = XWingSecretKey::from_seed(&[17; 32]).unwrap();
    let dsa = MlDsa65SecretKey::from_seed(&[17; 32]).unwrap();
    let (_, shared) = kem.public_key().encapsulate();
    assert_eq!(format!("{kem:?}"), "MlKem768SecretKey([REDACTED])");
    assert_eq!(format!("{xwing:?}"), "XWingSecretKey([REDACTED])");
    assert_eq!(format!("{dsa:?}"), "MlDsa65SecretKey([REDACTED])");
    assert_eq!(format!("{shared:?}"), "SharedSecret([REDACTED])");
    assert_eq!(format!("{:?}", kem.export_seed()), "SecretSeed([REDACTED])");
    assert_eq!(
        format!("{:?}", xwing.export_seed()),
        "SecretSeed([REDACTED])"
    );
    assert_eq!(format!("{:?}", dsa.export_seed()), "SecretSeed([REDACTED])");
}

#[test]
fn nist_mlkem768_key_generation_known_answers() {
    let data = fixtures(include_str!("../tests/vectors/mlkem768-keygen.json"));
    for group in data["groups"].as_array().unwrap() {
        for case in group["tests"].as_array().unwrap() {
            let seed = [decode(case, "d"), decode(case, "z")].concat();
            let key = MlKem768SecretKey::from_seed(&seed).unwrap();
            assert_eq!(key.public_key().to_bytes().as_slice(), decode(case, "ek"));
            assert_eq!(key.export_seed().expose_secret().as_slice(), seed);
            // Expanded encoding is only used here to compare legacy official
            // ACVP fixtures. The public wrapper supports seeds exclusively.
            #[allow(deprecated)]
            {
                use ml_kem::ExpandedKeyEncoding;
                assert_eq!(key.0.to_expanded_bytes().as_slice(), decode(case, "dk"));
            }
        }
    }
}

#[test]
fn nist_mlkem768_encapsulation_and_decapsulation_known_answers() {
    let data = fixtures(include_str!("../tests/vectors/mlkem768-encap-decap.json"));
    for group in data["groups"].as_array().unwrap() {
        for case in group["tests"].as_array().unwrap() {
            if group["function"] == "encapsulation" {
                let key = MlKem768PublicKey::from_bytes(&decode(case, "ek")).unwrap();
                let randomness = ml_kem::B32::try_from(decode(case, "m").as_slice()).unwrap();
                // Deterministic hazardous primitive is restricted to this KAT.
                let (ct, ss) = key.0.encapsulate_deterministic(&randomness);
                assert_eq!(ct.as_slice(), decode(case, "c"));
                assert_eq!(ss.as_slice(), decode(case, "k"));
            }
            #[allow(deprecated)]
            let key = {
                let expanded = ml_kem::ExpandedDecapsulationKey::<ml_kem::MlKem768>::try_from(
                    decode(case, "dk").as_slice(),
                )
                .unwrap();
                ml_kem::DecapsulationKey768::from_expanded(&expanded).unwrap()
            };
            let ct = MlKem768Ciphertext::from_bytes(&decode(case, "c")).unwrap();
            assert_eq!(key.decapsulate(&ct.0).as_slice(), decode(case, "k"));
        }
    }
}

#[test]
fn nist_mldsa65_key_generation_known_answers() {
    let data = fixtures(include_str!("../tests/vectors/mldsa65-keygen.json"));
    for group in data["groups"].as_array().unwrap() {
        for case in group["tests"].as_array().unwrap() {
            let key = MlDsa65SecretKey::from_seed(&decode(case, "seed")).unwrap();
            assert_eq!(key.public_key().to_bytes().as_slice(), decode(case, "pk"));
            #[allow(deprecated)]
            {
                assert_eq!(
                    key.0.expanded_key().to_expanded().as_slice(),
                    decode(case, "sk")
                );
            }
        }
    }
}

#[test]
fn nist_mldsa65_external_pure_deterministic_signature_known_answers() {
    let data = fixtures(include_str!("../tests/vectors/mldsa65-sign.json"));
    for group in data["groups"].as_array().unwrap() {
        for case in group["tests"].as_array().unwrap() {
            let message = decode(case, "message");
            let context = decode(case, "context");
            let expected = decode(case, "signature");
            let public = MlDsa65PublicKey::from_bytes(&decode(case, "pk")).unwrap();
            let signature = MlDsa65Signature::from_bytes(&expected).unwrap();
            public.verify(&message, &context, &signature).unwrap();
            let expanded = ml_dsa::ExpandedSigningKeyBytes::<ml_dsa::MlDsa65>::try_from(
                decode(case, "sk").as_slice(),
            )
            .unwrap();
            #[allow(deprecated)]
            let key = ml_dsa::ExpandedSigningKey::<ml_dsa::MlDsa65>::from_expanded(&expanded);
            let actual = key.sign_deterministic(&message, &context).unwrap();
            assert_eq!(actual.encode().as_slice(), expected);
        }
    }
}

#[test]
fn official_xwing_specification_known_answers() {
    let data = fixtures(include_str!("../tests/vectors/xwing.json"));
    for case in data["tests"].as_array().unwrap() {
        let key = XWingSecretKey::from_seed(&decode(case, "sk")).unwrap();
        assert_eq!(key.public_key().to_bytes().as_slice(), decode(case, "pk"));
        let public = XWingPublicKey::from_bytes(&decode(case, "pk")).unwrap();
        let randomness = ml_kem::array::Array::<u8, ml_kem::array::sizes::U64>::try_from(
            decode(case, "eseed").as_slice(),
        )
        .unwrap();
        let (actual_ct, actual_ss) = public.0.encapsulate_deterministic(&randomness);
        assert_eq!(actual_ct.as_slice(), decode(case, "ct"));
        assert_eq!(actual_ss.as_slice(), decode(case, "ss"));
        let ct = XWingCiphertext::from_bytes(&decode(case, "ct")).unwrap();
        assert_eq!(
            key.decapsulate(&ct).unwrap().expose_secret().as_slice(),
            decode(case, "ss")
        );
    }
}
