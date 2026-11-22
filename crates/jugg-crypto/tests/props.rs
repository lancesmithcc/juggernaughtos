//! Property tests: decryption of hostile input must fail cleanly, never panic,
//! and any single-byte change to a sealed value must be rejected.
use jugg_crypto::{open, seal, FileCipher, Keyring};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn envelope_roundtrips_and_rejects_tampering(pt in proptest::collection::vec(any::<u8>(), 0..512), aad in "[a-z:0-9]{0,32}", flip in any::<prop::sample::Index>()) {
        let ring = Keyring::ephemeral();
        let sealed = seal(&ring, &pt, aad.as_bytes()).unwrap();
        prop_assert_eq!(open(&ring, &sealed, aad.as_bytes()).unwrap(), pt);
        let mut bad = sealed.clone();
        let i = flip.index(bad.len());
        bad[i] ^= 0x01;
        prop_assert!(open(&ring, &bad, aad.as_bytes()).is_err());
        prop_assert!(open(&ring, &sealed, b"other:aad").is_err());
    }

    #[test]
    fn envelope_open_survives_garbage(junk in proptest::collection::vec(any::<u8>(), 0..256)) {
        let ring = Keyring::ephemeral();
        let _ = open(&ring, &junk, b"t:c:1");
    }

    #[test]
    fn file_cipher_roundtrips_and_survives_garbage(pt in proptest::collection::vec(any::<u8>(), 0..4096), junk in proptest::collection::vec(any::<u8>(), 0..512)) {
        let ring = Keyring::ephemeral();
        let ct = FileCipher::encrypt(&ring, &pt, "f1").unwrap();
        prop_assert_eq!(FileCipher::decrypt(&ring, &ct, "f1").unwrap(), pt);
        prop_assert!(FileCipher::decrypt(&ring, &ct, "f2").is_err());
        let _ = FileCipher::decrypt(&ring, &junk, "f1");
    }
}
