use std::path::Path;

use contrast::load_cases;

fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (n, c) in files {
        std::fs::write(d.path().join(n), c).unwrap();
    }
    d
}

#[test]
fn loads_sorted_by_file_name_with_defaults() {
    let d = dir_with(&[
        ("b.toml", "[[case]]\nname='second'\npath='/b'\n"),
        (
            "a.toml",
            "[[case]]\nname='first'\npath='/a'\n[[case]]\nname='first2'\npath='/a2'\n",
        ),
        ("README.md", "ignored"),
        ("c.toml.disabled", "garbage ["),
    ]);
    let cases = load_cases(d.path()).unwrap();
    let names: Vec<_> = cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["first", "first2", "second"]);
    assert_eq!(cases[0].method, "GET");
    assert!(
        cases[0].headers.is_empty() && cases[0].body.is_none() && cases[0].cookie_jwt.is_none()
    );
    assert!(cases[0].ignore_fields.is_empty() && cases[0].expect_status.is_none());
}

#[test]
fn unknown_key_reports_file_and_case() {
    let d = dir_with(&[(
        "x.toml",
        "[[case]]\nname='ok'\npath='/a'\n[[case]]\nname='bad one'\npath='/b'\nbogus=1\n",
    )]);
    let e = load_cases(d.path()).unwrap_err().to_string();
    assert!(
        e.contains("x.toml") && e.contains("bad one") && e.contains("bogus"),
        "{e}"
    );
}

#[test]
fn unknown_key_in_cookie_jwt_rejected() {
    let d = dir_with(&[(
        "x.toml",
        "[[case]]\nname='n'\npath='/a'\n[case.cookie_jwt]\nuser_id='1'\nusername='u'\nsecret='no'\n",
    )]);
    let e = load_cases(d.path()).unwrap_err().to_string();
    assert!(e.contains("x.toml") && e.contains("secret"), "{e}");
}

#[test]
fn required_fields_validated() {
    let d = dir_with(&[("x.toml", "[[case]]\nname='n'\n")]);
    let e = load_cases(d.path()).unwrap_err().to_string();
    assert!(e.contains("x.toml") && e.contains("path"), "{e}");
    let d = dir_with(&[("y.toml", "[[case]]\npath='/a'\n")]);
    let e = load_cases(d.path()).unwrap_err().to_string();
    assert!(e.contains("y.toml") && e.contains("name"), "{e}");
    let d = dir_with(&[("z.toml", "[[case]]\nname='n'\npath='no-slash'\n")]);
    assert!(load_cases(d.path()).is_err());
}

#[test]
fn invalid_toml_and_missing_dir_error() {
    let d = dir_with(&[("x.toml", "[[case]\n")]);
    assert!(load_cases(d.path())
        .unwrap_err()
        .to_string()
        .contains("x.toml"));
    assert!(load_cases(Path::new("definitely/does/not/exist")).is_err());
}

#[test]
fn documented_example_parses() {
    let cases = load_cases(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")).unwrap();
    assert_eq!(cases.len(), 2);
    let c = &cases[0];
    assert_eq!(c.method, "POST");
    assert_eq!(c.expect_status, Some(200));
    assert_eq!(c.ignore_fields.len(), 3);
    assert_eq!(c.headers.len(), 2);
    let jwt = c.cookie_jwt.as_ref().unwrap();
    assert_eq!(jwt.user_id, "123");
    assert!(jwt.is_admin);
    assert_eq!(cases[1].name, "minimo");
}
