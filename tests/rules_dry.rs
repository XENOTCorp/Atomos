use atomos::atom::AtomCtx;
use atomos::rules::Ruleset;

#[test]
fn dry_test_reports_overlap() {
    let ctx = AtomCtx::test();
    let v = ctx
        .run(
            "rules.dry_test",
            serde_json::json!({"rules":[
                {"id":"a","module":"static","methods":["GET"],"include":["/*"],"exclude":[]},
                {"id":"b","module":"api","methods":["GET"],"include":["/*"],"exclude":[]}
            ]}),
        )
        .unwrap();
    assert_eq!(v["ok"], false);
    assert!(v.get("example_path").is_some());
}

#[test]
fn parse_rejects_overlap() {
    assert!(Ruleset::parse(
        br#"{"rules":[
      {"id":"a","module":"static","methods":["GET"],"include":["/*"],"exclude":[]},
      {"id":"b","module":"api","methods":["GET"],"include":["/*"],"exclude":[]}
    ]}"#
    )
    .is_err());
}
