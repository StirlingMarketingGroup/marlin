use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use marlin_gdrive::{AccessTokens, OAuthClient, SavedSignIn, SignInError, KEYRING_SERVICE};
use marlin_vfs::VfsError;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, Respond, ResponseTemplate};

fn client(server: &MockServer) -> OAuthClient {
    OAuthClient {
        client_id: "client-id".into(),
        client_secret: "client-secret".into(),
        token_uri: format!("{}/token", server.uri()),
    }
}

struct BodyContains(String);

impl Match for BodyContains {
    fn matches(&self, request: &Request) -> bool {
        String::from_utf8_lossy(&request.body).contains(&self.0)
    }
}

fn token_ok(access_token: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "access_token": access_token,
        "expires_in": 3600,
    }))
}

#[tokio::test]
async fn saved_refresh_token_is_traded_for_an_access_token() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "trade@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-trade");

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains("client_id=client-id".into()))
        .and(BodyContains("client_secret=client-secret".into()))
        .and(BodyContains("refresh_token=refresh-trade".into()))
        .and(BodyContains("grant_type=refresh_token".into()))
        .respond_with(token_ok("access-trade"))
        .expect(1)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-trade");
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-trade");
}

#[tokio::test]
async fn invalid_grant_asks_the_user_to_sign_in_again() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "rejected@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-old");
    let sign_in = SavedSignIn::new(email, client(&server));

    {
        let _guard = Mock::given(method("POST"))
            .and(path("/token"))
            .and(BodyContains("refresh_token=refresh-old".into()))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#),
            )
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        assert_eq!(sign_in.valid_token().await, Err(SignInError::Rejected));
        assert_eq!(sign_in.access_token().await, Err(VfsError::SignInNeeded));
        assert_eq!(sign_in.valid_token().await, Err(SignInError::Rejected));
    }

    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-new");
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains("refresh_token=refresh-new".into()))
        .respond_with(token_ok("access-new"))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-new");
}

#[tokio::test]
async fn a_missing_keychain_entry_asks_the_user_to_sign_in() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "missing@example.com";

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_ok("access-added"))
        .expect(1)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    assert_eq!(sign_in.valid_token().await, Err(SignInError::NoSavedSignIn));
    assert_eq!(sign_in.access_token().await, Err(VfsError::SignInNeeded));

    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-added");
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-added");
}

#[tokio::test]
async fn an_unreachable_token_endpoint_is_offline() {
    marlin_test_keychain::install();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let email = "offline@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-offline");
    let sign_in = SavedSignIn::new(
        email,
        OAuthClient {
            client_id: "client-id".into(),
            client_secret: "client-secret".into(),
            token_uri: format!("http://127.0.0.1:{port}/token"),
        },
    );

    assert!(matches!(
        sign_in.valid_token().await,
        Err(SignInError::Offline(_))
    ));
    assert_eq!(sign_in.access_token().await, Err(VfsError::Offline));
}

#[tokio::test]
async fn a_token_answer_hides_secrets() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let refresh = "refresh-hidden";
    let secret = "client-secret";

    let email = "status-hides@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, refresh);
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains(format!("refresh_token={refresh}")))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(format!(r#"{{"error":"{refresh} {secret}"}}"#)),
        )
        .expect(2)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    let Err(SignInError::Other(text)) = sign_in.valid_token().await else {
        panic!("expected Other");
    };
    assert!(!text.contains(refresh), "{text}");
    assert!(!text.contains(secret), "{text}");
    marlin_test_keychain::remove(KEYRING_SERVICE, email);
    assert_eq!(
        sign_in.access_token().await,
        Err(VfsError::Other(
            "Marlin could not use the saved Google sign-in".into()
        ))
    );

    server.verify().await;
    server.reset().await;
    let email = "body-hides@example.com";
    let refresh = "refresh-unreadable";
    marlin_test_keychain::set(KEYRING_SERVICE, email, refresh);
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains(format!("refresh_token={refresh}")))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"{{"access_token":12,"expires_in":3600,"echo":"{refresh} {secret}"}}"#
        )))
        .expect(2)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    let Err(SignInError::Other(text)) = sign_in.valid_token().await else {
        panic!("expected Other");
    };
    assert!(!text.contains(refresh), "{text}");
    assert!(!text.contains(secret), "{text}");
    assert_eq!(text, "Google sent a token answer Marlin could not read");
    marlin_test_keychain::remove(KEYRING_SERVICE, email);
    assert_eq!(
        sign_in.access_token().await,
        Err(VfsError::Other(
            "Marlin could not use the saved Google sign-in".into()
        ))
    );
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_refresh_token() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "keeps-refresh@example.com";
    let refresh = "refresh-kept";
    marlin_test_keychain::set(KEYRING_SERVICE, email, refresh);

    let seen = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(FailThenSucceed { seen: seen.clone() })
        .expect(2)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    assert!(matches!(
        sign_in.valid_token().await,
        Err(SignInError::Other(_))
    ));
    marlin_test_keychain::remove(KEYRING_SERVICE, email);
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-kept");

    let bodies = seen.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].contains(&format!("refresh_token={refresh}")));
}

struct FailThenSucceed {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Respond for FailThenSucceed {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body = String::from_utf8_lossy(&request.body).into_owned();
        let count = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(body);
            seen.len()
        };
        if count == 1 {
            ResponseTemplate::new(503)
        } else {
            token_ok("access-kept")
        }
    }
}

#[tokio::test]
async fn rejected_drops_only_the_token_google_refused() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "reject-one@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-reject");

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(NumberedTokens {
            issued: Arc::new(AtomicUsize::new(0)),
        })
        .expect(2)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-1");
    sign_in.rejected("access-1");
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-2");
    sign_in.rejected("some-other-token");
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-2");
}

struct NumberedTokens {
    issued: Arc<AtomicUsize>,
}

impl Respond for NumberedTokens {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let number = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        token_ok(&format!("access-{number}"))
    }
}

#[tokio::test]
async fn calls_that_arrive_together_refresh_once() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;
    let email = "together@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-together");

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_ok("access-together").set_delay(Duration::from_millis(200)))
        .expect(1)
        .mount(&server)
        .await;

    let sign_in = SavedSignIn::new(email, client(&server));
    let (a, b, c, d, e, f, g, h) = tokio::join!(
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
        sign_in.valid_token(),
    );
    for token in [a, b, c, d, e, f, g, h] {
        assert_eq!(token.unwrap(), "access-together");
    }
}

#[tokio::test]
async fn signed_in_uses_its_access_token_until_it_is_near_expiry() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;

    let email = "signed-in-later@example.com";
    let later = SavedSignIn::signed_in(
        email,
        client(&server),
        "refresh-later".into(),
        "access-later".into(),
        SystemTime::now() + Duration::from_secs(60 * 60),
    );
    assert_eq!(later.valid_token().await.unwrap(), "access-later");
    assert!(marlin_test_keychain::get(KEYRING_SERVICE, email).is_none());

    let email = "signed-in-soon@example.com";
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains("refresh_token=refresh-soon".into()))
        .respond_with(token_ok("access-soon"))
        .expect(1)
        .mount(&server)
        .await;
    let soon = SavedSignIn::signed_in(
        email,
        client(&server),
        "refresh-soon".into(),
        "access-stale".into(),
        SystemTime::now() + Duration::from_secs(2 * 60),
    );
    assert_eq!(soon.valid_token().await.unwrap(), "access-soon");
    assert!(marlin_test_keychain::get(KEYRING_SERVICE, email).is_none());
}

#[tokio::test]
async fn a_new_refresh_token_is_written_to_the_keychain() {
    marlin_test_keychain::install();
    let server = MockServer::start().await;

    let email = "rotate@example.com";
    marlin_test_keychain::set(KEYRING_SERVICE, email, "refresh-before");
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(BodyContains("refresh_token=refresh-before".into()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-rotated",
            "expires_in": 3600,
            "refresh_token": "refresh-after",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let sign_in = SavedSignIn::new(email, client(&server));
    assert_eq!(sign_in.valid_token().await.unwrap(), "access-rotated");
    assert_eq!(
        marlin_test_keychain::get(KEYRING_SERVICE, email).as_deref(),
        Some("refresh-after")
    );
}

#[test]
fn debug_hides_the_client_secret() {
    let client = OAuthClient {
        client_id: "client-id".into(),
        client_secret: "client-secret".into(),
        token_uri: "https://oauth2.googleapis.com/token".into(),
    };
    let printed = format!("{:?}", client);
    assert!(!printed.contains("client-secret"), "{printed}");
}
