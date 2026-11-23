use super::*;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app() -> Router {
    router(AppState {
        network: Arc::new(Network::load().unwrap()),
        db: Db::temporary().await,
        site_url: "https://tubele.test".into(),
    })
}

async fn body(response: Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn the_page_mints_a_player_and_sets_a_strict_policy() {
    let response = app()
        .await
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(
        cookie.starts_with("tubele=") && cookie.contains("HttpOnly") && cookie.contains("Secure")
    );
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(!csp.contains("unsafe-inline"));
    assert!(body(response).await.contains("Service information"));
}

#[tokio::test]
async fn a_post_without_the_datastar_header_is_refused() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/guess")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"q":"bank"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_guess_patches_the_board_and_clears_the_field() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/guess")
                .header("datastar-request", "true")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"q":"bank","active":0,"mode":"daily","puzzle":0}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let text = body(response).await;
    assert!(text.contains("event: datastar-patch-elements"));
    assert!(text.contains("Guess 1 of 6. Bank") || text.contains("Correct. Bank"));
    assert!(text.contains(r#"signals {"_count":0,"_open":false,"active":0,"q":""}"#));
}

#[tokio::test]
async fn unknown_paths_are_real_404s() {
    let response = app()
        .await
        .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_oversized_body_is_refused_before_it_is_read() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/guess")
                .header("datastar-request", "true")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"q":"{}","active":0}}"#,
                    "a".repeat(MAX_BODY)
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_panicking_handler_answers_500_rather_than_dropping_the_connection() {
    async fn boom() -> &'static str {
        panic!("handler exploded");
    }
    let response = perimeter(Router::new().route("/boom", get(boom)))
        .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn the_stylesheet_names_its_fonts_by_urls_cached_for_good() {
    let app = app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(CSS_URL.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let css = body(response).await;
    for (path, _) in FONTS {
        let start = css
            .find(&format!("url(\"{path}?v="))
            .unwrap_or_else(|| panic!("{path} is not fingerprinted"));
        let url = css[start + 5..].split('"').next().unwrap();
        let response = app
            .clone()
            .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
    }
}
