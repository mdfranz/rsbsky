//! Client behaviour against a scripted fake PDS (wiremock).

use rsbsky::bsky::Client;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn login_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.server.createSession"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"accessJwt":"JWT","did":"did:me","handle":"me.test"})),
        )
        .mount(server)
        .await;
}

async fn logged_in(server: &MockServer) -> Client {
    login_mock(server).await;
    let mut c = Client::new(&server.uri());
    c.login("me.test", "pw").await.unwrap();
    c
}

fn repost(by: &str, rkey: &str) -> serde_json::Value {
    json!({"post":{"uri":format!("at://did:x/app.bsky.feed.post/{rkey}"),"cid":"c",
        "author":{"did":"did:x","handle":"x.test"},"record":{"text":"t","createdAt":"2024-01-01T00:00:00Z"}},
        "reason":{"$type":"app.bsky.feed.defs#reasonRepost","by":{"did":"did:by","handle":by},"indexedAt":"2024-01-02T00:00:00Z"}})
}

#[tokio::test]
async fn login_posts_credentials_and_stores_identity() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    assert_eq!((c.did.as_str(), c.handle.as_str()), ("did:me", "me.test"));
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body, json!({"identifier":"me.test","password":"pw"}));
}

#[tokio::test]
async fn login_error_uses_nsid_message_error_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error":"AuthenticationRequired","message":"Invalid \u{1b}[31midentifier"}),
        ))
        .mount(&server)
        .await;
    let err = Client::new(&server.uri())
        .login("a", "b")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "com.atproto.server.createSession: Invalid identifier (AuthenticationRequired)"
    );
}

#[tokio::test]
async fn non_200_without_error_body_reports_status() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).set_body_string("<html/>"))
        .mount(&server)
        .await;
    let err = c.get_profile("x").await.unwrap_err();
    assert_eq!(err.to_string(), "app.bsky.actor.getProfile: HTTP 503");
}

#[tokio::test]
async fn timeline_sends_bearer_limit_and_cursor() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/xrpc/app.bsky.feed.getTimeline"))
        .and(header("authorization", "Bearer JWT"))
        .and(query_param("limit", "50"))
        .and(query_param("cursor", "abc"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"feed":[repost("r.test","1")],"cursor":"next"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (items, next) = c.get_timeline("abc", 50).await.unwrap();
    assert_eq!(next, "next");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].by, "r.test");
    assert_eq!(items[0].reposted_at, "2024-01-02T00:00:00Z");
    assert!(
        !items[0].post.repost,
        "reposts are reported as the original post plus `by`"
    );
    assert_eq!(items[0].key(), "at://did:x/app.bsky.feed.post/1|r.test");
}

#[tokio::test]
async fn repost_scan_pages_stop_at_cursor_end_and_make_no_writes() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(query_param("limit", "100"))
        .and(query_param_is_missing("cursor"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"feed":[repost("a","1")],"cursor":"c2"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("cursor", "c2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"feed":[repost("b","2")]})))
        .mount(&server)
        .await;
    // pages = 0 still reads one page; pages = 5 stops when the cursor ends.
    assert_eq!(c.get_timeline_reposts(0).await.unwrap().len(), 1);
    let all = c.get_timeline_reposts(5).await.unwrap();
    assert_eq!(
        all.iter().map(|r| r.by.handle.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    for r in server.received_requests().await.unwrap() {
        assert!(
            r.method.as_str() == "GET" || r.url.path().ends_with("createSession"),
            "unexpected {r:?}"
        );
    }
}

#[tokio::test]
async fn get_profiles_batches_of_25() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/xrpc/app.bsky.actor.getProfiles"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"profiles":[{"did":"did:p","handle":"p"}]})),
        )
        .mount(&server)
        .await;
    let actors: Vec<String> = (0..60).map(|i| format!("did:{i}")).collect();
    let got = c.get_profiles(&actors).await.unwrap();
    assert_eq!(got.len(), 3);
    let sizes: Vec<usize> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("getProfiles"))
        .map(|r| r.url.query_pairs().filter(|(k, _)| k == "actors").count())
        .collect();
    assert_eq!(sizes, [25, 25, 10]);
}

#[tokio::test]
async fn profile_text_is_sanitized_and_viewer_parsed() {
    let server = MockServer::start().await;
    let c = logged_in(&server).await;
    Mock::given(method("GET"))
        .and(path("/xrpc/app.bsky.actor.getProfile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "did":"did:p","handle":"p.test\u{1b}[1m","displayName":"Name\u{7}","description":null,
            "postsCount":7,"viewer":{"following":"at://f","blocking":null}})))
        .mount(&server)
        .await;
    let p = c.get_profile("p.test").await.unwrap();
    assert_eq!(
        (
            p.handle.as_str(),
            p.display_name.as_str(),
            p.description.as_str()
        ),
        ("p.test", "Name", "")
    );
    assert_eq!(p.posts_count, 7);
    assert_eq!(
        (p.viewer.following.as_str(), p.viewer.blocking.as_str()),
        ("at://f", "")
    );
}
