use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use rand::RngCore;
use sha2::{Digest, Sha512};
use zeroize::Zeroize;

fn hash_to_scalar(parts: &[&[u8]]) -> Scalar {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&digest);
    Scalar::from_bytes_mod_order_wide(&wide)
}

fn hash_to_point(parts: &[&[u8]]) -> RistrettoPoint {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0x1f]);
    }
    let digest = hasher.finalize();
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&digest);
    RistrettoPoint::from_uniform_bytes(&wide)
}

fn ring_hash_challenge(message: &[u8], left: &RistrettoPoint, right: &RistrettoPoint) -> Scalar {
    let left_bytes = left.compress().to_bytes();
    let right_bytes = right.compress().to_bytes();
    hash_to_scalar(&[b"zoka-clsag-challenge", message, &left_bytes, &right_bytes])
}

pub fn encode_signature(c0: Scalar, key_image: &RistrettoPoint, responses: &[Scalar]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64 + responses.len() * 32);
    bytes.extend_from_slice(&c0.to_bytes());
    bytes.extend_from_slice(&key_image.compress().to_bytes());
    for response in responses {
        bytes.extend_from_slice(&response.to_bytes());
    }
    bytes
}

pub fn decode_signature(
    signature: &[u8],
    ring_len: usize,
) -> Option<(Scalar, RistrettoPoint, Vec<Scalar>)> {
    let expected_len = 64usize.checked_add(ring_len.checked_mul(32)?)?;
    if ring_len == 0 || signature.len() != expected_len {
        return None;
    }

    let mut c0_bytes = [0u8; 32];
    c0_bytes.copy_from_slice(&signature[..32]);
    let c0 = Option::<Scalar>::from(Scalar::from_canonical_bytes(c0_bytes))?;

    let mut key_image_bytes = [0u8; 32];
    key_image_bytes.copy_from_slice(&signature[32..64]);
    let key_image = CompressedRistretto(key_image_bytes).decompress()?;

    let mut responses = Vec::with_capacity(ring_len);
    for chunk in signature[64..].chunks_exact(32) {
        let mut response_bytes = [0u8; 32];
        response_bytes.copy_from_slice(chunk);
        responses.push(Option::<Scalar>::from(Scalar::from_canonical_bytes(
            response_bytes,
        ))?);
    }

    Some((c0, key_image, responses))
}

pub fn sign_single_input(
    ring_points: &[RistrettoPoint],
    signer_index: usize,
    signer_secret: Scalar,
    signer_key_image: RistrettoPoint,
    message: &[u8],
) -> Option<Vec<u8>> {
    let ring_len = ring_points.len();
    if ring_len < 2 || signer_index >= ring_len {
        return None;
    }

    let mut alpha_bytes = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut alpha_bytes);
    let alpha = Scalar::from_bytes_mod_order_wide(&alpha_bytes);
    alpha_bytes.zeroize();
    sign_single_input_with_nonce(
        ring_points,
        signer_index,
        signer_secret,
        signer_key_image,
        message,
        alpha,
    )
}

pub fn sign_single_input_with_nonce(
    ring_points: &[RistrettoPoint],
    signer_index: usize,
    signer_secret: Scalar,
    signer_key_image: RistrettoPoint,
    message: &[u8],
    alpha: Scalar,
) -> Option<Vec<u8>> {
    let ring_len = ring_points.len();
    if ring_len < 2 || signer_index >= ring_len {
        return None;
    }
    let signer_hash_point = hash_to_point(&[
        b"zoka-key-image",
        ring_points[signer_index].compress().as_bytes(),
    ]);
    let mut responses = vec![Scalar::ZERO; ring_len];
    let mut challenges = vec![Scalar::ZERO; ring_len];

    let alpha_left = alpha * RISTRETTO_BASEPOINT_POINT;
    let alpha_right = alpha * signer_hash_point;
    challenges[(signer_index + 1) % ring_len] =
        ring_hash_challenge(message, &alpha_left, &alpha_right);

    let mut i = (signer_index + 1) % ring_len;
    while i != signer_index {
        responses[i] = hash_to_scalar(&[
            b"zoka-clsag-response",
            &alpha.to_bytes(),
            &i.to_le_bytes(),
            message,
            ring_points[i].compress().as_bytes(),
        ]);

        let member_point = ring_points[i];
        let member_hash_point =
            hash_to_point(&[b"zoka-key-image", member_point.compress().as_bytes()]);
        let left = responses[i] * RISTRETTO_BASEPOINT_POINT + challenges[i] * member_point;
        let right = responses[i] * member_hash_point + challenges[i] * signer_key_image;
        challenges[(i + 1) % ring_len] = ring_hash_challenge(message, &left, &right);
        i = (i + 1) % ring_len;
    }

    responses[signer_index] = alpha - (challenges[signer_index] * signer_secret);
    Some(encode_signature(
        challenges[0],
        &signer_key_image,
        &responses,
    ))
}

pub fn verify_single_input(
    ring_points: &[RistrettoPoint],
    expected_key_image: RistrettoPoint,
    message: &[u8],
    signature: &[u8],
) -> bool {
    if ring_points.len() < 2 {
        return false;
    }
    let Some((c0, key_image, responses)) = decode_signature(signature, ring_points.len()) else {
        return false;
    };
    if key_image != expected_key_image {
        return false;
    }

    let mut challenge = c0;
    for (row_idx, response) in responses.iter().enumerate() {
        let member_point = ring_points[row_idx];
        let member_hash_point =
            hash_to_point(&[b"zoka-key-image", member_point.compress().as_bytes()]);
        let left = *response * RISTRETTO_BASEPOINT_POINT + challenge * member_point;
        let right = *response * member_hash_point + challenge * key_image;
        challenge = ring_hash_challenge(message, &left, &right);
    }

    challenge == c0
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
    use serde::Deserialize;
    use sha2::{Digest, Sha512};

    #[derive(Debug, Deserialize)]
    struct ClsagVector {
        name: String,
        ring_pubkeys: Vec<String>,
        key_image: String,
        message_hex: String,
        signature_hex: String,
    }

    fn ring_points(size: usize) -> Vec<RistrettoPoint> {
        (0..size)
            .map(|idx| hash_to_point(&[b"zoka-clsag-test-member", &idx.to_le_bytes()]))
            .collect()
    }

    fn scalar_from_label(label: &[u8]) -> Scalar {
        hash_to_scalar(&[b"zoka-clsag-vector-scalar", label])
    }

    fn point_to_hex(point: &RistrettoPoint) -> String {
        hex::encode(point.compress().to_bytes())
    }

    fn point_from_hex(hex_str: &str) -> Option<RistrettoPoint> {
        let bytes = hex::decode(hex_str).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        CompressedRistretto(arr).decompress()
    }

    fn reference_hash_to_scalar(parts: &[&[u8]]) -> Scalar {
        let mut hasher = Sha512::new();
        for part in parts {
            hasher.update(part);
            hasher.update([0x1f]);
        }
        let digest = hasher.finalize();
        let mut wide = [0u8; 64];
        wide.copy_from_slice(&digest);
        Scalar::from_bytes_mod_order_wide(&wide)
    }

    fn reference_hash_to_point(parts: &[&[u8]]) -> RistrettoPoint {
        let mut hasher = Sha512::new();
        for part in parts {
            hasher.update(part);
            hasher.update([0x1f]);
        }
        let digest = hasher.finalize();
        let mut wide = [0u8; 64];
        wide.copy_from_slice(&digest);
        RistrettoPoint::from_uniform_bytes(&wide)
    }

    fn verify_single_input_reference(
        ring_points: &[RistrettoPoint],
        expected_key_image: RistrettoPoint,
        message: &[u8],
        signature: &[u8],
    ) -> bool {
        let Some((c0, key_image, responses)) = decode_signature(signature, ring_points.len())
        else {
            return false;
        };
        if key_image != expected_key_image {
            return false;
        }

        let mut challenge = c0;
        for (idx, response) in responses.iter().enumerate() {
            let point = ring_points[idx];
            let hashed_point =
                reference_hash_to_point(&[b"zoka-key-image", point.compress().as_bytes()]);
            let left = *response * RISTRETTO_BASEPOINT_POINT + challenge * point;
            let right = *response * hashed_point + challenge * key_image;
            let left_bytes = left.compress().to_bytes();
            let right_bytes = right.compress().to_bytes();
            challenge = reference_hash_to_scalar(&[
                b"zoka-clsag-challenge",
                message,
                &left_bytes,
                &right_bytes,
            ]);
        }
        challenge == c0
    }

    #[test]
    fn test_clsag_sign_verify_ring_sizes_2_to_16() {
        let message = b"clsag-test-message";
        for size in 2..=16 {
            let mut ring = ring_points(size);
            let signer_index = size / 2;
            let signer_secret =
                hash_to_scalar(&[b"zoka-clsag-test-secret", &(size as u64).to_le_bytes()]);
            ring[signer_index] = signer_secret * RISTRETTO_BASEPOINT_POINT;
            let signer_key_image = signer_secret
                * hash_to_point(&[b"zoka-key-image", ring[signer_index].compress().as_bytes()]);
            let sig = sign_single_input(
                &ring,
                signer_index,
                signer_secret,
                signer_key_image,
                message,
            )
            .expect("signature");
            assert!(verify_single_input(&ring, signer_key_image, message, &sig));
            assert_eq!(sig.len(), 64 + 32 * size);
        }
    }

    #[test]
    fn test_clsag_rejects_tampered_message_and_signature() {
        let mut ring = ring_points(8);
        let signer_index = 3usize;
        let signer_secret = hash_to_scalar(&[b"zoka-clsag-test-secret-2"]);
        ring[signer_index] = signer_secret * RISTRETTO_BASEPOINT_POINT;
        let signer_key_image = signer_secret
            * hash_to_point(&[b"zoka-key-image", ring[signer_index].compress().as_bytes()]);
        let message = b"clsag-tamper-message";
        let mut sig = sign_single_input(
            &ring,
            signer_index,
            signer_secret,
            signer_key_image,
            message,
        )
        .expect("signature");

        assert!(verify_single_input(&ring, signer_key_image, message, &sig));
        assert!(!verify_single_input(
            &ring,
            signer_key_image,
            b"clsag-tamper-message-modified",
            &sig
        ));

        sig[70] ^= 0x01;
        assert!(!verify_single_input(&ring, signer_key_image, message, &sig));
    }

    #[test]
    fn test_clsag_deterministic_vector_with_fixed_nonce() {
        let secret_r0 = hash_to_scalar(&[b"r0"]);
        let secret_r1 = hash_to_scalar(&[b"r1"]);
        let secret_r2 = hash_to_scalar(&[b"r2"]);
        let ring = vec![
            secret_r0 * RISTRETTO_BASEPOINT_POINT,
            secret_r1 * RISTRETTO_BASEPOINT_POINT,
            secret_r2 * RISTRETTO_BASEPOINT_POINT,
        ];
        let signer_index = 1usize;
        let signer_secret = secret_r1;
        let signer_key_image = signer_secret
            * hash_to_point(&[b"zoka-key-image", ring[signer_index].compress().as_bytes()]);
        let alpha = hash_to_scalar(&[b"alpha-fixed"]);
        let message = b"clsag-vector-message";

        let sig = sign_single_input_with_nonce(
            &ring,
            signer_index,
            signer_secret,
            signer_key_image,
            message,
            alpha,
        )
        .expect("deterministic signature");
        let sig_hex = hex::encode(&sig);

        assert_eq!(
            sig_hex,
            "32e64da4c2f6a59b2c64d0cc487a4a88c9c06738782c08dcff6bdfb00f06c2078c3c2449e89fbfbe3a2aedde837702d7f36c5eb0286cfc80a7288fa43b688b77eaa0afc44a9f18ec33a8376e35c09a9bfcd4647d9b5b886bbccf795e92adc40a7468dd4fb22d2f8551f2ab467335ee4430181f409b05f9f8f2ab5c8e5379f30cd8028c3de90b09bd5c056b0114ce704c7d06c8b72d60892d68d8df6afa707703"
        );
        assert!(verify_single_input(&ring, signer_key_image, message, &sig));
    }

    #[test]
    fn test_clsag_vector_corpus_cross_verification() {
        let raw = include_str!("clsag_vectors.json");
        let vectors: Vec<ClsagVector> = serde_json::from_str(raw).expect("parse clsag vectors");
        assert!(!vectors.is_empty(), "vector corpus must not be empty");
        for vector in vectors {
            let ring = vector
                .ring_pubkeys
                .iter()
                .map(|p| point_from_hex(p).expect("valid ring point"))
                .collect::<Vec<_>>();
            let key_image = point_from_hex(&vector.key_image).expect("valid key image");
            let message = hex::decode(&vector.message_hex).expect("valid message hex");
            let signature = hex::decode(&vector.signature_hex).expect("valid signature hex");

            assert!(
                verify_single_input(&ring, key_image, &message, &signature),
                "primary verifier failed for vector {}",
                vector.name
            );
            assert!(
                verify_single_input_reference(&ring, key_image, &message, &signature),
                "reference verifier failed for vector {}",
                vector.name
            );
            assert_eq!(
                signature.len(),
                64 + 32 * ring.len(),
                "signature size mismatch for vector {}",
                vector.name
            );
        }
    }

    #[test]
    #[ignore]
    fn dump_clsag_vector_candidates() {
        let scenarios = vec![
            (
                "v1_ring3",
                3usize,
                1usize,
                b"vector-msg-1".as_slice(),
                b"alpha-v1".as_slice(),
            ),
            (
                "v2_ring5",
                5usize,
                2usize,
                b"vector-msg-2".as_slice(),
                b"alpha-v2".as_slice(),
            ),
            (
                "v3_ring8",
                8usize,
                4usize,
                b"vector-msg-3".as_slice(),
                b"alpha-v3".as_slice(),
            ),
        ];

        for (name, ring_size, signer_index, message, alpha_label) in scenarios {
            let secrets = (0..ring_size)
                .map(|idx| scalar_from_label(format!("{name}-secret-{idx}").as_bytes()))
                .collect::<Vec<_>>();
            let ring = secrets
                .iter()
                .map(|secret| *secret * RISTRETTO_BASEPOINT_POINT)
                .collect::<Vec<_>>();
            let signer_secret = secrets[signer_index];
            let key_image = signer_secret
                * hash_to_point(&[b"zoka-key-image", ring[signer_index].compress().as_bytes()]);
            let alpha = scalar_from_label(alpha_label);
            let signature = sign_single_input_with_nonce(
                &ring,
                signer_index,
                signer_secret,
                key_image,
                message,
                alpha,
            )
            .expect("deterministic sign");

            println!("name={name}");
            println!("message_hex={}", hex::encode(message));
            println!("key_image={}", point_to_hex(&key_image));
            println!("signature_hex={}", hex::encode(signature));
            println!(
                "ring=[{}]",
                ring.iter()
                    .map(point_to_hex)
                    .map(|v| format!("\"{v}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            println!();
        }
    }
}
