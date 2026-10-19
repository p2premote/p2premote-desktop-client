use super::*;

#[test]
fn public_network_providers_reject_invalid_or_incomplete_payloads_without_io() {
    let providers: Vec<Box<dyn PublicNetworkProvider>> = vec![
        Box::new(IpWhoIsProvider),
        Box::new(CipCcProvider),
        Box::new(MyIpIpipProvider),
        Box::new(PConlineProvider),
        Box::new(IpInfoProvider),
        Box::new(IfconfigCoProvider),
    ];
    for provider in providers {
        assert!(!provider.name().is_empty());
        assert!(provider.endpoint().starts_with("https://"));
        for bytes in [b"".as_slice(), b"not JSON", b"{}", b"{\"ip\":\"invalid\"}"] {
            assert!(
                provider.parse(bytes).is_none(),
                "{} accepted incomplete payload",
                provider.name()
            );
        }
    }
    assert!(IpWhoIsProvider
        .parse(br#"{"success":false,"ip":"8.8.8.8","country":"CN"}"#)
        .is_none());
    assert!(CipCcProvider
        .parse(b"IP : 8.8.8.8\nDATA: English only")
        .is_none());
    assert!(CipCcProvider
        .parse("IP : 8.8.8.8\n数据二 :   ".as_bytes())
        .is_none());
    assert!(MyIpIpipProvider
        .parse("当前 IP：8.8.8.8 来自于： ".as_bytes())
        .is_none());
    assert!(PConlineProvider.parse(&[0x81]).is_none());
}

#[test]
fn heterogeneous_public_network_payloads_keep_locations_and_isp() {
    for (provider, payload, expected) in [
        (
            &IpWhoIsProvider as &dyn PublicNetworkProvider,
            r#"{"ip":"8.8.8.8","country":"CN","region":"Sichuan","city":"Chengdu","connection":{"isp":"ISP"}}"#,
            "CN · Sichuan · Chengdu · ISP",
        ),
        (
            &IpInfoProvider as &dyn PublicNetworkProvider,
            r#"{"ip":"8.8.8.8","country":"CN","region":"Sichuan","city":"Chengdu","org":"ISP"}"#,
            "CN · Sichuan · Chengdu · ISP",
        ),
        (
            &IfconfigCoProvider as &dyn PublicNetworkProvider,
            r#"{"ip":"8.8.8.8","country":"CN","region_name":"Sichuan","city":"Chengdu","asn_org":"ISP"}"#,
            "CN · Sichuan · Chengdu · ISP",
        ),
    ] {
        let network = provider.parse(payload.as_bytes()).unwrap();
        assert_eq!(network.ip, "8.8.8.8");
        assert_eq!(network.location, expected);
    }
    let text = "unstructured preamble\nIP : 8.8.8.8\n数据二 : 中国 四川 | ISP\n数据三 : ignored";
    assert_eq!(
        CipCcProvider.parse(text.as_bytes()).unwrap().location,
        "中国 · 四川 · ISP"
    );
    assert_eq!(
        MyIpIpipProvider
            .parse("当前 IP：8.8.8.8 来自于：中国 四川 ISP".as_bytes())
            .unwrap()
            .location,
        "中国 · 四川 · ISP"
    );
    let json = r#"{"ip":"8.8.8.8","pro":"四川","city":"成都","addr":"四川成都 ISP"}"#;
    let (gbk, _, error) = encoding_rs::GBK.encode(json);
    assert!(!error);
    assert_eq!(
        PConlineProvider.parse(&gbk).unwrap().location,
        "四川 · 成都 · ISP"
    );
}

#[test]
fn location_formatting_handles_empty_parts_and_nonduplicated_carrier_names() {
    assert!(public_network_from_parts("invalid", &["CN"], None).is_none());
    assert!(public_network_from_parts("8.8.8.8", &["", " "], Some("ISP")).is_none());
    assert_eq!(
        public_network_from_parts("8.8.8.8", &["CN", " "], Some(" "))
            .unwrap()
            .location,
        "CN"
    );
    assert_eq!(format_cip_cc_location(""), "");
    assert_eq!(format_cip_cc_location("中国 四川"), "中国 · 四川");
    assert_eq!(
        pconline_public_network("8.8.8.8", "四川", "成都", "separate carrier")
            .unwrap()
            .location,
        "四川 · 成都 · separate carrier"
    );
    assert_eq!(
        pconline_public_network("8.8.8.8", "四川", "成都", "四川成都")
            .unwrap()
            .location,
        "四川 · 成都"
    );
    assert!(pconline_public_network("8.8.8.8", "", "", "ISP").is_none());
}
