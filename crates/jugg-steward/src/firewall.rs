//! nftables. Install creates the table; the steward only ever adds/removes
//! elements of the `banned` sets (with kernel-side timeouts, so bans expire
//! even if the steward is down).

/// Base ruleset written at install (`/etc/nftables.d/juggernaught.nft`).
/// Deny inbound by default; allow 80/443 (public site + ACME), SSH only from
/// the admin allow-list or the Tailscale range when configured.
pub fn base_ruleset(ssh_allow: &[String], ssh_port: u16) -> String {
    let (v4, v6): (Vec<&String>, Vec<&String>) = ssh_allow.iter().partition(|a| !a.contains(':'));
    let ssh = if ssh_allow.is_empty() {
        format!("tcp dport {ssh_port} ct state new limit rate 6/minute accept")
    } else {
        let mut rules = Vec::new();
        if !v4.is_empty() {
            rules.push(format!("ip saddr {{ {} }} tcp dport {ssh_port} accept", v4.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if !v6.is_empty() {
            rules.push(format!("ip6 saddr {{ {} }} tcp dport {ssh_port} accept", v6.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        rules.join("\n\t\t")
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
        let r = super::base_ruleset(&[], 2222);
        assert!(r.contains("policy drop") && r.contains("tcp dport 2222"));
        let r = super::base_ruleset(&["100.64.0.0/10".into(), "fd7a:115c:a1e0::/48".into()], 22);
        assert!(r.contains("ip saddr { 100.64.0.0/10 }") && r.contains("ip6 saddr { fd7a:115c:a1e0::/48 }"));
    }
}
