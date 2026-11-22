//! Property tests for everything the steward parses from attackers: journal
//! lines, request paths and IPs headed for the firewall.
use jugg_steward::{daemon::sshd_event, firewall::ban_command, honeytokens::{classify, not_found}};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn sshd_parser_never_panics(line in ".{0,300}", ip in "[0-9a-f.:]{0,45}") {
        let _ = sshd_event(&line);
        let _ = sshd_event(&format!("Failed password for invalid user {line} from {ip} port 22 ssh2"));
        let _ = sshd_event(&format!("Invalid user {line} from {ip}"));
    }

    #[test]
    fn honeytokens_never_panic(path in ".{0,200}", ip in ".{0,60}") {
        let _ = classify(&path, &ip);
        let _ = not_found(&path, &ip);
    }

    #[test]
    fn only_real_ips_reach_nft(ip in ".{0,60}", secs in any::<u64>()) {
        if let Some(cmd) = ban_command(&ip, secs) {
            prop_assert!(ip.parse::<std::net::IpAddr>().is_ok(), "ban_command accepted {:?}", ip);
            for arg in &cmd {
                prop_assert!(!arg.contains(';') && !arg.contains('\n') && !arg.contains('`'), "{:?}", cmd);
            }
        }
    }
}
