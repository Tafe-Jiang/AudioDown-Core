use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use audiodown_network_proxy::{
    policy::{ProxyPolicy, ProxyPolicyError},
    resolver::StaticResolver,
};
use audiodown_plugin_api::manifest::PluginManifest;

#[test]
fn allows_exact_and_single_label_wildcard_hosts_with_https_defaults() {
    let mut resolver = StaticResolver::new([
        ("api.virtual.invalid", vec![public_v4(10)]),
        ("cdn.media.virtual.invalid", vec![public_v4(11)]),
    ]);
    let policy = ProxyPolicy::production(&manifest([
        "api.virtual.invalid",
        "*.media.virtual.invalid",
    ]));

    let exact = policy
        .authorize_url("https://api.virtual.invalid/resource", &mut resolver)
        .expect("exact host");
    assert_eq!(exact.host(), "api.virtual.invalid");
    assert_eq!(exact.port(), 443);
    assert_eq!(exact.pinned_addresses(), &[public_v4(10)]);

    let wildcard = policy
        .authorize_url("https://cdn.media.virtual.invalid:8443/path", &mut resolver)
        .expect("wildcard host");
    assert_eq!(wildcard.host(), "cdn.media.virtual.invalid");
    assert_eq!(wildcard.port(), 8443);

    assert!(matches!(
        policy.authorize_url("https://deep.cdn.media.virtual.invalid", &mut resolver),
        Err(ProxyPolicyError::HostNotAllowed)
    ));
}

#[test]
fn rejects_unsafe_url_shapes_and_unauthorized_hosts() {
    let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![public_v4(10)])]);
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for url in [
        "http://api.virtual.invalid/resource",
        "https://user:pass@api.virtual.invalid/resource",
        "https://api.virtual.invalid/resource#fragment",
        "https://api.virtual.invalid:0/resource",
        "https://api.virtual.invalid:65536/resource",
        "https://93.184.216.34/resource",
        "https://[2606:2800:220:1:248:1893:25c8:1946]/resource",
    ] {
        assert!(
            matches!(
                policy.authorize_url(url, &mut resolver),
                Err(ProxyPolicyError::InvalidUrl)
            ),
            "{url} should be rejected"
        );
    }

    assert!(matches!(
        policy.authorize_url("https://other.virtual.invalid/resource", &mut resolver),
        Err(ProxyPolicyError::HostNotAllowed)
    ));
}

#[test]
fn rejects_private_special_documentation_and_mixed_dns_answers() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));
    for address in [
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
        IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10)),
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "fe80::1".parse().unwrap(),
        "fc00::1".parse().unwrap(),
        "ff02::1".parse().unwrap(),
        "2001:db8::10".parse().unwrap(),
    ] {
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(
            matches!(
                policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
                Err(ProxyPolicyError::BlockedAddress)
            ),
            "{address} should be blocked"
        );
    }

    let mut mixed = StaticResolver::new([(
        "api.virtual.invalid",
        vec![public_v4(10), IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
    )]);
    assert!(matches!(
        policy.authorize_url("https://api.virtual.invalid/resource", &mut mixed),
        Err(ProxyPolicyError::BlockedAddress)
    ));
}

#[test]
fn rejects_deprecated_ipv6_site_local_range() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for address in ["fec0::1", "feff:ffff::1"] {
        let address = address.parse::<IpAddr>().unwrap();
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(
            matches!(
                policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
                Err(ProxyPolicyError::BlockedAddress)
            ),
            "{address} should be blocked"
        );
    }
}

#[test]
fn rejects_rfc9637_documentation_ipv6_range_boundaries() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for address in ["3fff::1", "3fff:0fff:ffff:ffff:ffff:ffff:ffff:ffff"] {
        let address = address.parse::<IpAddr>().unwrap();
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(
            matches!(
                policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
                Err(ProxyPolicyError::BlockedAddress)
            ),
            "{address} should be blocked"
        );
    }
}

#[test]
fn rejects_ipv6_translation_and_special_prefixes() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for address in [
        "64:ff9b::c000:201",
        "64:ff9b:1::c000:201",
        "100::1",
        "2001::1",
        "2001:01ff:ffff::1",
        "2002:c000:0201::1",
    ] {
        let address = address.parse::<IpAddr>().unwrap();
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(
            matches!(
                policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
                Err(ProxyPolicyError::BlockedAddress)
            ),
            "{address} should be blocked"
        );
    }
}

#[test]
fn allows_ordinary_global_unicast_ipv6() {
    let address = "2606:4700:4700::1111".parse::<IpAddr>().unwrap();
    let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    let target = policy
        .authorize_url("https://api.virtual.invalid/resource", &mut resolver)
        .expect("ordinary global-unicast IPv6 address");
    assert_eq!(target.pinned_addresses(), &[address]);
}

#[test]
fn detects_dns_rebinding_and_redirect_host_changes() {
    let mut resolver = StaticResolver::new([
        ("api.virtual.invalid", vec![public_v4(10)]),
        ("other.virtual.invalid", vec![public_v4(10)]),
    ]);
    let policy =
        ProxyPolicy::production(&manifest(["api.virtual.invalid", "other.virtual.invalid"]));
    let pinned = policy
        .authorize_url("https://api.virtual.invalid/resource", &mut resolver)
        .expect("initial target");

    resolver.set("api.virtual.invalid", vec![public_v4(12)]);
    assert!(matches!(
        policy.authorize_redirect(&pinned, "https://api.virtual.invalid/next", &mut resolver),
        Err(ProxyPolicyError::DnsRebinding)
    ));

    resolver.set("api.virtual.invalid", vec![public_v4(10)]);
    assert!(matches!(
        policy.authorize_redirect(&pinned, "https://other.virtual.invalid/next", &mut resolver),
        Err(ProxyPolicyError::RedirectHostChanged)
    ));
}

#[test]
fn developer_fixture_mappings_are_exact_and_rejected_in_production() {
    let mut resolver = StaticResolver::empty();
    let production = ProxyPolicy::production(&manifest(["fixture.virtual.invalid"]));
    assert!(matches!(
        production.with_fixture_mapping("fixture.virtual.invalid", public_v4(10)),
        Err(ProxyPolicyError::DeveloperModeRequired)
    ));

    let developer = ProxyPolicy::developer(&manifest(["fixture.virtual.invalid"]))
        .with_fixture_mapping("fixture.virtual.invalid", IpAddr::V4(Ipv4Addr::LOCALHOST))
        .expect("developer fixture");
    let target = developer
        .authorize_url("http://fixture.virtual.invalid/audio", &mut resolver)
        .expect("developer fixture allows http");
    assert_eq!(
        target.pinned_addresses(),
        &[IpAddr::V4(Ipv4Addr::LOCALHOST)]
    );

    assert!(matches!(
        developer.with_fixture_mapping("*.virtual.invalid", IpAddr::V4(Ipv4Addr::LOCALHOST)),
        Err(ProxyPolicyError::InvalidFixtureMapping)
    ));
}

#[test]
fn developer_mode_requires_fixture_mapping_for_http() {
    let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![public_v4(10)])]);
    let policy = ProxyPolicy::developer(&manifest(["api.virtual.invalid"]));

    assert!(matches!(
        policy.authorize_url("http://api.virtual.invalid/resource", &mut resolver),
        Err(ProxyPolicyError::InvalidUrl)
    ));

    let target = policy
        .authorize_url("https://api.virtual.invalid/resource", &mut resolver)
        .expect("unmapped developer host remains available over https");
    assert_eq!(target.pinned_addresses(), &[public_v4(10)]);
}

#[test]
fn policy_uses_allowed_hosts_from_the_plugin_manifest() {
    let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![public_v4(10)])]);
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    assert!(policy
        .authorize_url("https://api.virtual.invalid/resource", &mut resolver)
        .is_ok());
    assert!(matches!(
        policy.authorize_url("https://unlisted.virtual.invalid/resource", &mut resolver),
        Err(ProxyPolicyError::HostNotAllowed)
    ));
}

#[test]
fn blocks_ipv4_mapped_ipv6_addresses_using_ipv4_rules() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for address in [
        "::ffff:127.0.0.1".parse::<IpAddr>().unwrap(),
        "::ffff:10.0.0.1".parse::<IpAddr>().unwrap(),
        "::ffff:169.254.1.1".parse::<IpAddr>().unwrap(),
        "::ffff:169.254.169.254".parse::<IpAddr>().unwrap(),
    ] {
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(matches!(
            policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
            Err(ProxyPolicyError::BlockedAddress)
        ));
    }
}

#[test]
fn blocks_ipv4_compatible_ipv6_addresses_using_ipv4_rules() {
    let policy = ProxyPolicy::production(&manifest(["api.virtual.invalid"]));

    for address in [
        "::127.0.0.1".parse::<IpAddr>().unwrap(),
        "::10.0.0.1".parse::<IpAddr>().unwrap(),
        "::169.254.1.1".parse::<IpAddr>().unwrap(),
        "::169.254.169.254".parse::<IpAddr>().unwrap(),
    ] {
        let mut resolver = StaticResolver::new([("api.virtual.invalid", vec![address])]);
        assert!(matches!(
            policy.authorize_url("https://api.virtual.invalid/resource", &mut resolver),
            Err(ProxyPolicyError::BlockedAddress)
        ));
    }
}

fn manifest<const N: usize>(allowed_hosts: [&str; N]) -> PluginManifest {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.example.virtual.content",
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": ["system.health"],
        "network": {"allowedHosts": allowed_hosts.to_vec()}
    }))
    .expect("valid plugin manifest")
}

fn public_v4(last_octet: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(93, 184, 216, last_octet))
}
