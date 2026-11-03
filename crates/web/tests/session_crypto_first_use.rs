//! Regression: sign/verify must work as the very first jsonwebtoken use in the
//! process (a single crypto backend must be enabled workspace-wide). Keep this
//! test alone in its binary so no earlier call can mask the problem.

use web::session::{new_claims, sign, verify};

#[test]
fn sign_and_verify_work_as_first_use_in_process() {
    let secret = b"test-secret-do-not-use-in-prod";
    let c = new_claims(
        "1".into(),
        "u".into(),
        None,
        "user".into(),
        false,
        None,
        1_900_000_000,
    );
    let token = sign(&c, secret).expect("sign");
    let back = verify(&token, secret).expect("verify");
    assert_eq!(back.user_id, "1");
}
