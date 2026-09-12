use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use nodeharbor_controller::{router, State};
use tower::ServiceExt;

#[tokio::test]
async fn dashboard_auth_requires_both_the_trusted_gateway_and_an_allowed_identity() {
    let state = State::open("sqlite::memory:", "test-admin")
        .await
        .unwrap()
        .with_proxy_auth(
            "gateway-test-secret",
            vec!["owner@example.com".into()],
            "https://workers.example.com",
        )
        .unwrap();
    let app = router(state);
    for (secret, email, origin, expected) in [
        (
            "",
            "owner@example.com",
            "https://workers.example.com",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "gateway-test-secret",
            "stranger@example.com",
            "https://workers.example.com",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "gateway-test-secret",
            "owner@example.com",
            "https://evil.example.com",
            StatusCode::FORBIDDEN,
        ),
        (
            "gateway-test-secret",
            "owner@example.com",
            "https://workers.example.com",
            StatusCode::CREATED,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/enrollment-codes")
                    .header("x-nodeharbor-proxy-token", secret)
                    .header("x-auth-request-email", email)
                    .header("origin", origin)
                    .header("x-nodeharbor-request", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}
