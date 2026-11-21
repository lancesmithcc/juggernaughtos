//! nftables. Install creates the table; the steward only ever adds/removes
//! elements of the `banned` sets (with kernel-side timeouts, so bans expire
//! even if the steward is down).

/// Base ruleset written at install (`/etc/nftables.d/juggernaught.nft`).
/// Deny inbound by default; allow 80/443 (public site + ACME), SSH only from
/// the admin allow-list or the Tailscale range when configured.
pub fn base_ruleset(ssh_allow: &[String]) -> String {
    let ssh = if ssh_allow.is_empty() {
        "tcp dport 22 ct state new limit rate 6/minute accept".to_string()
    } else {
        format!("ip saddr {{ {} }} tcp dport 22 accept", ssh_allow.join(", "))
    };
    format!(
        "table inet juggernaught {{\n\
         \tset banned4 {{ type ipv4_addr; flags timeout; }}\n\
         \tset banned6 {{ type ipv6_addr; flags timeout; }}\n\
         \tchain input {{\n\
         \t\ttype filter hook input priority 0; policy drop;\n\
         \t\tiif lo accept\n\
         \t\tct state established,related accept\n\
         \t\tct state invalid drop\n\
         \t\tip saddr @banned4 drop\n\
         \t\tip6 saddr @banned6 drop\n\
         \t\tip protocol icmp limit rate 10/second accept\n\
         \t\tip6 nexthdr ipv6-icmp limit rate 10/second accept\n\
         \t\ttcp dport {{ 80, 443 }} accept\n\
         \t\tudp dport 443 accept\n\
         \t\t{ssh}\n\
         \t}}\n\
         }}\n"
    )
}

/// argv for `nft` to ban an address (validated: must parse as an IP).
pub fn ban_command(ip: &str, secs: u64) -> Option<Vec<String>> {
    let addr: std::net::IpAddr = ip.parse().ok()?;
    let set = if addr.is_ipv4() { "banned4" } else { "banned6" };
    Some(vec!["add".into(), "element".into(), "inet".into(), "juggernaught".into(), set.into(), format!("{{ {addr} timeout {secs}s }}")])
}

#[cfg(test)]
mod tests {
    #[test]
    fn ban_rejects_injection() {
        assert!(super::ban_command("1.2.3.4; rm -rf /", 60).is_none());
        assert_eq!(super::ban_command("2001:db8::1", 60).unwrap()[4], "banned6");
        assert!(super::base_ruleset(&[]).contains("policy drop"));
    }
}
