//! Property test: a backup file is attacker-controlled once it leaves the box
//! (off-site storage). Opening garbage must fail cleanly.
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn open_rejects_garbage(junk in proptest::collection::vec(any::<u8>(), 0..4096), magic in any::<bool>()) {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("b.jugg");
        let mut bytes = if magic { b"JBK1".to_vec() } else { Vec::new() };
        bytes.extend_from_slice(&junk);
        std::fs::write(&f, &bytes).unwrap();
        let ring = jugg_crypto::Keyring::ephemeral();
        prop_assert!(jugg_backup::open(&f, jugg_backup::Unlock::Box(&ring), &[], Some(&dir.path().join("x"))).is_err());
    }
}
