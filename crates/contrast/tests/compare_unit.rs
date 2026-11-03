use contrast::{compare, Case, Observed};

fn obs(status: u16, headers: &[(&str, &str)], body: &str) -> Observed {
    Observed {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body: body.as_bytes().to_vec(),
    }
}

fn json(body: &str) -> Observed {
    obs(200, &[], body)
}

fn case() -> Case {
    Case::get("c", "/x")
}

fn with_ignore(fields: &[&str]) -> Case {
    let mut c = case();
    c.ignore_fields = fields.iter().map(|s| s.to_string()).collect();
    c
}

#[test]
fn key_order_is_irrelevant() {
    assert!(compare(
        &json(r#"{"a":1,"b":2}"#),
        &json(r#"{"b":2,"a":1}"#),
        &case()
    )
    .is_ok());
}

#[test]
fn array_order_is_significant() {
    let d = compare(&json("[1,2]"), &json("[2,1]"), &case()).unwrap_err();
    assert!(d.to_string().contains("$.0"), "{d}");
}

#[test]
fn one_equals_one_point_zero() {
    assert!(compare(&json(r#"{"n":1}"#), &json(r#"{"n":1.0}"#), &case()).is_ok());
    assert!(compare(&json(r#"{"n":1}"#), &json(r#"{"n":2.0}"#), &case()).is_err());
}

#[test]
fn reports_first_differing_path_and_values() {
    let d = compare(
        &json(r#"{"a":{"b":[1,{"c":"node-val"}]}}"#),
        &json(r#"{"a":{"b":[1,{"c":"rust-val"}]}}"#),
        &case(),
    )
    .unwrap_err();
    let s = d.to_string();
    assert!(s.contains("$.a.b.1.c"), "{s}");
    assert!(s.contains("node-val") && s.contains("rust-val"), "{s}");
}

#[test]
fn missing_and_extra_keys_are_diffs() {
    let d = compare(&json(r#"{"a":1}"#), &json(r#"{"a":1,"b":2}"#), &case()).unwrap_err();
    assert!(d.to_string().contains("$.b"), "{d}");
    assert!(compare(&json(r#"{"a":1,"b":2}"#), &json(r#"{"a":1}"#), &case()).is_err());
}

#[test]
fn simple_and_wildcard_ignore_paths() {
    let n = json(
        r#"{"timestamp":1,"users":[{"id":1,"last_seen":5},{"id":2,"last_seen":6}],"data":[{"id":9,"k":1}]}"#,
    );
    let r = json(
        r#"{"timestamp":2,"users":[{"id":1,"last_seen":7},{"id":2,"last_seen":8}],"data":[{"id":10,"k":1}]}"#,
    );
    let ign = with_ignore(&["timestamp", "users.*.last_seen", "data.0.id"]);
    assert!(compare(&n, &r, &case()).is_err());
    assert!(compare(&n, &r, &ign).is_ok());
    // un comodin no oculta otras diferencias
    let r2 = json(
        r#"{"timestamp":2,"users":[{"id":1,"last_seen":7},{"id":3,"last_seen":8}],"data":[{"id":10,"k":1}]}"#,
    );
    assert!(compare(&n, &r2, &ign).is_err());
}

#[test]
fn ignore_works_when_key_missing_on_one_side() {
    assert!(compare(
        &json(r#"{"a":1,"t":5}"#),
        &json(r#"{"a":1}"#),
        &with_ignore(&["t"])
    )
    .is_ok());
}

#[test]
fn non_json_bodies_compare_bytes() {
    assert!(compare(&json("hola"), &json("hola"), &case()).is_ok());
    let d = compare(&json("hola"), &json("hole"), &case()).unwrap_err();
    assert!(d.to_string().contains("body"), "{d}");
    // uno JSON y otro no
    assert!(compare(&json("{}"), &json("nope"), &case()).is_err());
    // espacios distintos en JSON no importan
    assert!(compare(&json("{ \"a\" : 1 }"), &json("{\"a\":1}"), &case()).is_ok());
}

#[test]
fn status_mismatch() {
    let d = compare(&obs(200, &[], ""), &obs(404, &[], ""), &case()).unwrap_err();
    let s = d.to_string();
    assert!(
        s.contains("status") && s.contains("200") && s.contains("404"),
        "{s}"
    );
}

#[test]
fn expect_status_checked_on_both_sides() {
    let mut c = case();
    c.expect_status = Some(200);
    assert!(compare(&obs(200, &[], ""), &obs(200, &[], ""), &c).is_ok());
    assert!(compare(&obs(500, &[], ""), &obs(500, &[], ""), &c).is_err());
}

#[test]
fn content_type_charset_case_and_param_order() {
    let a = obs(
        200,
        &[("Content-Type", "application/json; charset=utf-8")],
        "{}",
    );
    let b = obs(
        200,
        &[("content-type", "Application/JSON;charset=UTF-8")],
        "{}",
    );
    assert!(compare(&a, &b, &case()).is_ok());
    let c = obs(
        200,
        &[("content-type", "text/html; charset=utf-8; x=1")],
        "{}",
    );
    let d = obs(
        200,
        &[("content-type", "text/html; x=1; charset=utf-8")],
        "{}",
    );
    assert!(compare(&c, &d, &case()).is_ok());
    let e = obs(200, &[("content-type", "text/html")], "{}");
    let d = compare(&a, &e, &case()).unwrap_err();
    assert!(d.to_string().contains("content-type"), "{d}");
}

#[test]
fn cache_control_directive_order_insensitive() {
    let a = obs(200, &[("cache-control", "public, Max-Age=60")], "");
    let b = obs(200, &[("Cache-Control", "max-age=60,public")], "");
    assert!(compare(&a, &b, &case()).is_ok());
    let c = obs(200, &[("cache-control", "max-age=61, public")], "");
    assert!(compare(&a, &c, &case()).is_err());
    let none = obs(200, &[], "");
    assert!(compare(&a, &none, &case()).is_err());
}

#[test]
fn location_is_exact() {
    let a = obs(302, &[("location", "/a")], "");
    let b = obs(302, &[("Location", "/a")], "");
    let c = obs(302, &[("location", "/b")], "");
    assert!(compare(&a, &b, &case()).is_ok());
    let d = compare(&a, &c, &case()).unwrap_err();
    assert!(d.to_string().contains("location"), "{d}");
}

#[test]
fn set_cookie_ignores_value_and_expires() {
    let a = obs(
        200,
        &[("set-cookie", "auth_token=AAA; Path=/; Expires=Wed, 21 Oct 2026 07:28:00 GMT; HttpOnly; SameSite=Lax; Max-Age=28800")],
        "",
    );
    let b = obs(
        200,
        &[("Set-Cookie", "auth_token=BBB; max-age=28800; samesite=lax; HttpOnly; path=/; Expires=Thu, 22 Oct 2026 08:00:00 GMT")],
        "",
    );
    assert!(compare(&a, &b, &case()).is_ok());
}

#[test]
fn set_cookie_attribute_differences_reported() {
    let a = obs(
        200,
        &[("set-cookie", "t=1; Path=/; HttpOnly; Max-Age=10")],
        "",
    );
    let no_http_only = obs(200, &[("set-cookie", "t=1; Path=/; Max-Age=10")], "");
    let other_age = obs(
        200,
        &[("set-cookie", "t=1; Path=/; HttpOnly; Max-Age=11")],
        "",
    );
    let other_name = obs(
        200,
        &[("set-cookie", "u=1; Path=/; HttpOnly; Max-Age=10")],
        "",
    );
    for b in [no_http_only, other_age, other_name] {
        let d = compare(&a, &b, &case()).unwrap_err();
        assert!(d.to_string().contains("set-cookie"), "{d}");
    }
}

#[test]
fn multiple_set_cookies_compare_as_set() {
    let a = obs(
        200,
        &[("set-cookie", "a=1; Path=/"), ("set-cookie", "b=2; Path=/")],
        "",
    );
    let b = obs(
        200,
        &[("set-cookie", "b=9; Path=/"), ("set-cookie", "a=8; Path=/")],
        "",
    );
    assert!(compare(&a, &b, &case()).is_ok());
    let c = obs(200, &[("set-cookie", "a=1; Path=/")], "");
    assert!(compare(&a, &c, &case()).is_err());
}

#[test]
fn other_headers_are_not_compared() {
    let a = obs(
        200,
        &[
            ("date", "x"),
            ("etag", "1"),
            ("x-powered-by", "Express"),
            ("content-length", "2"),
        ],
        "{}",
    );
    let b = obs(
        200,
        &[("date", "y"), ("etag", "2"), ("vary", "Origin")],
        "{}",
    );
    assert!(compare(&a, &b, &case()).is_ok());
}

#[test]
fn ignore_headers_skips_comparison() {
    let a = obs(302, &[("location", "/a")], "");
    let b = obs(302, &[("location", "/b")], "");
    let mut c = case();
    c.ignore_headers = vec!["Location".to_string()];
    assert!(compare(&a, &b, &c).is_ok());
}

#[test]
fn long_values_are_truncated_in_report() {
    let big_a = format!("\"{}\"", "a".repeat(5000));
    let big_b = format!("\"{}\"", "b".repeat(5000));
    let d = compare(&json(&big_a), &json(&big_b), &case()).unwrap_err();
    assert!(d.to_string().len() < 1500, "{}", d.to_string().len());
}
