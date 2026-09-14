use nodeharbor_core::verify_signed_update;

// Public interoperability vector from minisign-verify 0.2.5 (MIT).
const KEY: &str =
    "untrusted comment: public test key\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

#[test]
fn signatures_authenticate_streamed_payload_and_trusted_metadata() {
    assert!(verify_signed_update(&b"test"[..], KEY, SIGNATURE).is_ok());
    assert!(verify_signed_update(&b"Test"[..], KEY, SIGNATURE).is_err());
    assert!(verify_signed_update(&b"test extra bytes"[..], KEY, SIGNATURE).is_err());
    assert!(verify_signed_update(
        &b"test"[..],
        KEY,
        &SIGNATURE.replace("file:test", "file:other")
    )
    .is_err());
    assert!(verify_signed_update(
        &b"test"[..],
        include_str!("../../../nodeharbor.minisign.pub"),
        SIGNATURE
    )
    .is_err());
}

#[test]
fn android_callers_cannot_replace_the_embedded_trust_key() {
    let request = serde_json::json!({"operation":"verifyUpdate", "path":"/missing.apk", "signature":SIGNATURE, "publicKey":KEY});
    assert!(nodeharbor_core::android_request(&request.to_string()).is_err());
}
