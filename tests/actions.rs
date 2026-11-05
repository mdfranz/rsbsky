//! The remote half of moderation actions against a scripted PDS: exact requests, their
//! order, and what happens when a step fails.

use rsbsky::bsky::{Client, Profile};
use rsbsky::store::Entry;
use rsbsky::tui::actions::{ActRequest, UndoEntry, run_act};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BLOCK: &str = "app.bsky.graph.block";
const FOLLOW: &str = "app.bsky.graph.follow";

async fn client(server: &MockServer) -> Client {
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.server.createSession"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"accessJwt":"JWT","did":"did:me","handle":"me.test"})),
        )
        .mount(server)
        .await;
    let mut c = Client::new(&server.uri());
    c.login("me.test", "pw").await.unwrap();
    c
}

async fn create_ok(server: &MockServer, collection: &str, uri: &str) {
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.createRecord"))
        .and(wiremock::matchers::body_partial_json(
            json!({"collection": collection}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"uri": uri, "cid": "c"})))
        .mount(server)
        .await;
}

async fn delete_ok(server: &MockServer, collection: &str) {
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.deleteRecord"))
        .and(wiremock::matchers::body_partial_json(
            json!({"collection": collection}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
}

fn failing(status: u16, error: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({"error": error, "message": "no thanks"}))
}

/// POSTs after login, as (nsid, body).
async fn writes(server: &MockServer) -> Vec<(String, Value)> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && !r.url.path().ends_with("createSession"))
        .map(|r| {
            (
                r.url.path().rsplit('.').next().unwrap().to_string(),
                serde_json::from_slice(&r.body).unwrap(),
            )
        })
        .collect()
}

fn entry(decision: &str, prev: Option<&str>, record: &str) -> UndoEntry {
    UndoEntry {
        idx: 0,
        profile: Profile {
            did: "did:them".into(),
            handle: "them.test".into(),
            ..Default::default()
        },
        decision: decision.into(),
        record: record.into(),
        prev: prev.map(|d| Entry::now("them.test", d)),
    }
}

fn decide(e: UndoEntry, lift: bool, known: &str) -> ActRequest {
    ActRequest::Decide {
        entry: e,
        lift,
        known: known.into(),
    }
}

#[tokio::test]
async fn block_creates_a_block_record_about_the_account() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    create_ok(&server, BLOCK, "at://did:me/app.bsky.graph.block/new").await;
    let msg = run_act(&c, decide(entry("blocked", None, ""), false, "")).await;
    assert_eq!(
        (msg.err, msg.new_uri.as_str(), msg.lifted),
        (None, "at://did:me/app.bsky.graph.block/new", false)
    );
    let w = writes(&server).await;
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].0, "createRecord");
    assert_eq!(w[0].1["repo"], "did:me");
    assert_eq!(w[0].1["collection"], BLOCK);
    assert_eq!(w[0].1["record"]["$type"], BLOCK);
    assert_eq!(w[0].1["record"]["subject"], "did:them");
    assert!(
        chrono::DateTime::parse_from_rfc3339(w[0].1["record"]["createdAt"].as_str().unwrap())
            .is_ok()
    );
}

#[tokio::test]
async fn follow_and_keep_use_the_right_remote_steps() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    create_ok(&server, FOLLOW, "at://did:me/app.bsky.graph.follow/f").await;
    let msg = run_act(&c, decide(entry("followed", None, ""), false, "")).await;
    assert_eq!(msg.new_uri, "at://did:me/app.bsky.graph.follow/f");
    let kept = run_act(&c, decide(entry("kept", None, ""), false, "")).await;
    assert!(kept.err.is_none() && kept.new_uri.is_empty());
    assert_eq!(
        writes(&server).await.len(),
        1,
        "keep never touches the remote"
    );
}

#[tokio::test]
async fn failed_create_reports_the_api_error() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.createRecord"))
        .respond_with(failing(400, "InvalidRequest"))
        .mount(&server)
        .await;
    let msg = run_act(&c, decide(entry("blocked", None, ""), false, "")).await;
    assert_eq!(
        msg.err.as_deref(),
        Some("com.atproto.repo.createRecord: no thanks (InvalidRequest)")
    );
    assert!(msg.new_uri.is_empty());
}

#[tokio::test]
async fn overriding_a_block_deletes_the_known_record_before_anything_else() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    delete_ok(&server, BLOCK).await;
    create_ok(&server, FOLLOW, "at://did:me/app.bsky.graph.follow/f").await;
    let req = decide(
        entry("followed", Some("blocked"), ""),
        true,
        "at://did:me/app.bsky.graph.block/old1",
    );
    let msg = run_act(&c, req).await;
    assert!(msg.err.is_none() && msg.lifted);
    let w = writes(&server).await;
    assert_eq!(
        w.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["deleteRecord", "createRecord"],
        "lift first, then create"
    );
    assert_eq!(
        w[0].1,
        json!({"repo": "did:me", "collection": BLOCK, "rkey": "old1"})
    );
    assert_eq!(w[1].1["collection"], FOLLOW);
}

#[tokio::test]
async fn an_unknown_record_is_looked_up_from_the_profile() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/xrpc/app.bsky.actor.getProfile"))
        .and(query_param("actor", "did:them"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"did":"did:them","handle":"them.test",
            "viewer":{"blocking":"at://did:me/app.bsky.graph.block/looked-up"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    delete_ok(&server, BLOCK).await;
    let msg = run_act(&c, decide(entry("kept", Some("blocked"), ""), true, "")).await;
    assert!(msg.err.is_none() && msg.lifted);
    let w = writes(&server).await;
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].1["rkey"], "looked-up");
}

#[tokio::test]
async fn a_lookup_that_fails_or_finds_nothing_still_counts_as_lifted_without_a_delete() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("GET"))
        .respond_with(failing(500, "Boom"))
        .mount(&server)
        .await;
    let msg = run_act(&c, decide(entry("kept", Some("blocked"), ""), true, "")).await;
    assert!(
        msg.err.is_none() && msg.lifted,
        "nothing to delete is not an error"
    );
    assert!(writes(&server).await.is_empty());
}

#[tokio::test]
async fn a_failed_lift_stops_before_creating_anything() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.deleteRecord"))
        .respond_with(failing(500, "InternalServerError"))
        .mount(&server)
        .await;
    create_ok(&server, FOLLOW, "at://x").await;
    let msg = run_act(
        &c,
        decide(
            entry("followed", Some("blocked"), ""),
            true,
            "at://did:me/app.bsky.graph.block/old",
        ),
    )
    .await;
    assert_eq!(
        msg.err.as_deref(),
        Some("com.atproto.repo.deleteRecord: no thanks (InternalServerError)")
    );
    assert!(!msg.lifted, "the record may still exist");
    let w = writes(&server).await;
    assert_eq!(w.len(), 1, "no create after a failed delete");
}

#[tokio::test]
async fn undo_deletes_the_created_record_and_restores_a_lifted_one() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    delete_ok(&server, FOLLOW).await;
    create_ok(
        &server,
        BLOCK,
        "at://did:me/app.bsky.graph.block/replacement",
    )
    .await;
    // follow had lifted an earlier block: undoing the follow brings the block back
    let msg = run_act(
        &c,
        ActRequest::Undo(entry(
            "followed",
            Some("blocked"),
            "at://did:me/app.bsky.graph.follow/f9",
        )),
    )
    .await;
    assert!(msg.err.is_none() && msg.is_undo);
    assert_eq!(msg.new_uri, "at://did:me/app.bsky.graph.block/replacement");
    let w = writes(&server).await;
    assert_eq!(
        w.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["deleteRecord", "createRecord"]
    );
    assert_eq!(w[0].1["rkey"], "f9");
    assert_eq!(w[0].1["collection"], FOLLOW);
    assert_eq!(
        (
            w[1].1["collection"].as_str(),
            w[1].1["record"]["subject"].as_str()
        ),
        (Some(BLOCK), Some("did:them"))
    );
}

#[tokio::test]
async fn undo_of_local_only_decisions_or_unknown_records_makes_no_remote_call() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    for e in [
        entry("kept", None, ""),
        entry("blocked", None, ""),
        entry("kept", Some("kept"), ""),
    ] {
        let msg = run_act(&c, ActRequest::Undo(e)).await;
        assert!(msg.err.is_none());
    }
    assert!(writes(&server).await.is_empty());
}

#[tokio::test]
async fn undo_failures_are_reported_and_stop_the_sequence() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.deleteRecord"))
        .respond_with(failing(500, "InternalServerError"))
        .mount(&server)
        .await;
    create_ok(&server, BLOCK, "at://never").await;
    let msg = run_act(
        &c,
        ActRequest::Undo(entry(
            "followed",
            Some("blocked"),
            "at://did:me/app.bsky.graph.follow/f9",
        )),
    )
    .await;
    assert!(msg.err.as_deref().unwrap().contains("InternalServerError"));
    assert_eq!(
        writes(&server).await.len(),
        1,
        "the block is not restored when the follow could not be removed"
    );

    // and if restoring fails after a successful delete, that is reported too
    let server = MockServer::start().await;
    let c = client(&server).await;
    delete_ok(&server, FOLLOW).await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.createRecord"))
        .respond_with(failing(400, "InvalidRequest"))
        .mount(&server)
        .await;
    let msg = run_act(
        &c,
        ActRequest::Undo(entry(
            "followed",
            Some("blocked"),
            "at://did:me/app.bsky.graph.follow/f9",
        )),
    )
    .await;
    assert!(msg.err.as_deref().unwrap().contains("createRecord"));
}

#[tokio::test]
async fn unfollow_uses_the_known_record() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    delete_ok(&server, FOLLOW).await;
    let msg = run_act(
        &c,
        ActRequest::Unfollow(entry(
            "followed",
            None,
            "at://did:me/app.bsky.graph.follow/known",
        )),
    )
    .await;
    assert!(msg.err.is_none() && msg.is_unfollow);
    let w = writes(&server).await;
    assert_eq!(
        w[0].1,
        json!({"repo": "did:me", "collection": FOLLOW, "rkey": "known"})
    );
}

#[tokio::test]
async fn unfollow_without_a_record_looks_it_up_and_says_when_you_do_not_follow() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("GET"))
        .and(path("/xrpc/app.bsky.actor.getProfile"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"did":"did:them","handle":"them.test","viewer":{}})),
        )
        .mount(&server)
        .await;
    delete_ok(&server, FOLLOW).await;
    let msg = run_act(&c, ActRequest::Unfollow(entry("followed", None, ""))).await;
    assert_eq!(msg.err.as_deref(), Some("you do not follow @them.test"));
    assert!(writes(&server).await.is_empty());

    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"did":"did:them","handle":"them.test",
            "viewer":{"following":"at://did:me/app.bsky.graph.follow/found"}})),
        )
        .mount(&server)
        .await;
    delete_ok(&server, FOLLOW).await;
    let msg = run_act(&c, ActRequest::Unfollow(entry("followed", None, ""))).await;
    assert!(msg.err.is_none());
    assert_eq!(msg.undo.record, "at://did:me/app.bsky.graph.follow/found");
    assert_eq!(writes(&server).await[0].1["rkey"], "found");
}

#[tokio::test]
async fn unfollow_lookup_and_delete_errors_are_reported() {
    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("GET"))
        .respond_with(failing(502, "UpstreamFailure"))
        .mount(&server)
        .await;
    let msg = run_act(&c, ActRequest::Unfollow(entry("followed", None, ""))).await;
    assert!(msg.err.as_deref().unwrap().contains("getProfile"));

    let server = MockServer::start().await;
    let c = client(&server).await;
    Mock::given(method("POST"))
        .and(path("/xrpc/com.atproto.repo.deleteRecord"))
        .respond_with(failing(500, "InternalServerError"))
        .mount(&server)
        .await;
    let msg = run_act(
        &c,
        ActRequest::Unfollow(entry(
            "followed",
            None,
            "at://did:me/app.bsky.graph.follow/k",
        )),
    )
    .await;
    assert!(msg.err.as_deref().unwrap().contains("deleteRecord"));
}
