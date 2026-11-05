"""Compatibility matrix cases (Phase 0 and 1).

Each case is run against the Go and the Rust binary with an isolated HOME/state/audit and a
scripted fake PDS. Fields: name, args (Rust syntax; go_args overrides), env (None unsets),
files (relative to the temp dir), routes (fake PDS), stdin, tty (columns), jsonl (compare
stdout as parsed JSON lines), cmp (exact | status | stderr_first_line), defer (phase).

Deliberate differences from Go (see docs/compat.md) are expressed with cmp="status".
"""

import json

NO_CREDS = {"BSKY_HANDLE": None, "BSKY_APP_PASSWORD": None}
CASES = []


def case(name, args, **kw):
    CASES.append(dict(name=name, args=args, **kw))


# ---------------------------------------------------------------- fixture builders
def prof(did, handle, name="", **viewer):
    return {"did": did, "handle": handle, "displayName": name, "viewer": viewer}


def entry(rkey, author, text, by=None, created="{AGO:400d}", reason="app.bsky.feed.defs#reasonRepost",
          embed=None, view=None, facets=None, reply=None, parent=None, like=None):
    record = {"$type": "app.bsky.feed.post", "text": text, "createdAt": created}
    if embed:
        record["embed"] = embed
    if facets:
        record["facets"] = facets
    if reply:
        record["reply"] = reply
    post = {"uri": "at://%s/app.bsky.feed.post/%s" % (author["did"], rkey), "cid": "cid" + rkey,
            "author": author, "record": record, "likeCount": 1, "repostCount": 2, "replyCount": 3,
            "quoteCount": 4, "viewer": {"like": like} if like else {}}
    if view:
        post["embed"] = view
    e = {"post": post}
    if parent:
        e["reply"] = {"parent": {"author": parent[0], "record": {"text": parent[1]}}}
    if by:
        e["reason"] = {"$type": reason, "by": by, "indexedAt": "{AGO:5m}"}
    return e


def tl(entries, cursor=None, next=None, limit="100", status=200, body=None):
    r = {"path": "/xrpc/app.bsky.feed.getTimeline", "q": {"limit": limit, "cursor": cursor}, "status": status,
         "body": body if body is not None else {"feed": entries}}
    if next and body is None:
        r["body"]["cursor"] = next
    return r


A = prof("did:a", "a.test", "Alice")
B = prof("did:b", "b.test", "Bob", following="at://did:me/app.bsky.graph.follow/1")
C = prof("did:c", "c.test", "Carol", blocking="at://did:me/app.bsky.graph.block/1")
D = prof("did:d", "d.test", "Dan")
E = prof("did:e", "e.test", "Eve")
F = prof("did:me", "me.test", "Me")
G = prof("did:g", "g.test", "Gus")
H = prof("did:h", "h.test", "Hal")
I = prof("did:i", "i.test", "Iñés 日本語")
J = prof("did:j", "j.test\x1b[31m", "Jo\x1b]0;pwned\x07\x01x")
K = prof("did:k", "k.test", "Kim")
L = prof("did:l", "l.test", "Lou")
R1, R2, R3 = prof("did:r1", "r1.test"), prof("did:r2", "r2.test"), prof("did:r3", "r3.test")

IMG_EMBED = {"$type": "app.bsky.embed.images", "images": [{"alt": "  a   cat\nphoto ", "image": {}}, {"alt": "", "image": {}}]}
IMG_VIEW = {"$type": "app.bsky.embed.images#view", "images": [{"fullsize": "https://cdn/1.jpg"}, {"fullsize": "https://cdn/2.jpg"}]}
LINK_EMBED = {"$type": "app.bsky.embed.external", "external": {"uri": "https://example.com/a", "title": "Example   Title"}}
H_EMBED = {"$type": "app.bsky.embed.recordWithMedia",
           "media": {"$type": "app.bsky.embed.video", "alt": "a clip"},
           "record": {"record": {"uri": "at://did:q/app.bsky.feed.post/q1", "cid": "c"}}}
H_VIEW = {"$type": "app.bsky.embed.recordWithMedia#view", "media": {"$type": "app.bsky.embed.video#view"},
          "record": {"record": {"uri": "at://did:q/app.bsky.feed.post/q1", "author": prof("did:q", "q.test"),
                                "value": {"text": "quoted words"}}}}

UNI = "héllo exa… tail ✓"
_s = UNI.index("exa…")
_b = len(UNI[:_s].encode())
I_FACETS = [{"index": {"byteStart": _b, "byteEnd": _b + len("exa…".encode())},
             "features": [{"$type": "app.bsky.richtext.facet#link", "uri": "https://example.com/very/long/path"}]}]

PAGE1 = tl([
    entry("a1", A, "first post by A", by=R1),
    entry("g0", G, "original, not a repost"),
    entry("b1", B, "followed author", by=R1),
    entry("d1", D, "decided author", by=R2),
    entry("e1", E, "skipped author", by=R2),
    entry("a2", A, "second post by A", by=R2),
    entry("g1", G, "reply with a card", by=R3, embed=LINK_EMBED,
          reply={"root": {"uri": "at://did:g/app.bsky.feed.post/g0", "cid": "cg0"},
                 "parent": {"uri": "at://did:g/app.bsky.feed.post/g0", "cid": "cg0"}},
          parent=(prof("did:p", "p.test"), "parent text")),
    entry("h0", H, "pinned, not a repost", by=R1, reason="app.bsky.feed.defs#reasonPin"),
], next="p2")
PAGE2 = tl([
    entry("a3", A, "third post by A", by=R1),
    entry("c1", C, "blocked author", by=R1),
    entry("f1", F, "my own post", by=R1),
    entry("h1", H, "quote & video <b>", by=R3, embed=H_EMBED, view=H_VIEW),
    entry("i1", I, UNI, by=R2, facets=I_FACETS),
    entry("j1", J, "bell\x07 esc\x1b[31mred\x1b[0m\ttab", by=R1),
], cursor="p2", next="p3")
PAGE3 = tl([entry("k1", K, "", by=R1, embed=IMG_EMBED, view=IMG_VIEW)], cursor="p3", next="p4")
PAGE4 = tl([entry("l1", L, "only with four pages", by=R1)], cursor="p4")
TIMELINE = [PAGE1, PAGE2, PAGE3, PAGE4]

STATE = {"entries": {"did:d": {"handle": "d.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"},
                     "did:e": {"handle": "e.test", "decision": "skipped", "at": "2024-05-01T10:00:00.123456789+02:00"}}}

LIST = ["feed", "--list"]

# ---------------------------------------------------------------- Phase 0: help and usage
for n, args in [("help_long", ["--help"]), ("help_short", ["-h"]), ("help_command", ["help"]),
                ("help_after_feed", ["feed", "-h"]), ("help_after_block", ["block", "-h"])]:
    case(n, args, env=NO_CREDS, cmp="status")

# ---------------------------------------------------------------- Phase 0: credentials
case("no_creds_default", [], env=NO_CREDS)
case("no_creds_feed_list", LIST, env=NO_CREDS)
case("no_creds_review", ["review"], env=NO_CREDS)
case("no_creds_block", ["block", "x"], env=NO_CREDS)
case("only_handle", LIST, env={"BSKY_APP_PASSWORD": None})
case("only_password", LIST, env={"BSKY_HANDLE": None})
case("empty_handle", LIST, env={"BSKY_HANDLE": ""})

# ---------------------------------------------------------------- Phase 0: option validation
for n, args in [("pages_not_a_number", ["feed", "--pages", "abc"]),
                ("refresh_too_small", ["feed", "--refresh", "9s"]),
                ("refresh_no_unit", ["feed", "--refresh", "5"]),
                ("refresh_garbage", ["feed", "--refresh", "fast"]),
                ("theme_unknown_flag", ["feed", "--theme", "nope"]),
                ("theme_unknown_flag_with_list", ["feed", "--list", "--theme", "nope"]),
                ("unknown_flag", ["feed", "--bogus"]),
                ("review_rejects_pages", ["review", "--pages", "2"]),
                ("posts_rejects_backlog", ["posts", "--backlog"]),
                ("posts_json_unknown_theme_flag", ["posts", "--json", "--theme", "nope"])]:
    case(n, args, env=NO_CREDS, cmp="status")
for n, args in [("theme_env_unknown_feed", ["feed"]), ("theme_env_unknown_review", ["review"]),
                ("theme_env_unknown_posts", ["posts"]), ("theme_env_unknown_feed_json_tui", ["feed", "--json"])]:
    case(n, args, env=dict(NO_CREDS, BSKYBAN_THEME="nope"), cmp="status")
# printed lists ignore BSKYBAN_THEME, so they get as far as the credential check
for n, args in [("theme_env_ignored_feed_list", LIST), ("theme_env_ignored_posts_json", ["posts", "--json"]),
                ("theme_env_ignored_posts_list", ["posts", "--list"])]:
    case(n, args, env=dict(NO_CREDS, BSKYBAN_THEME="nope"))
for n, args in [("refresh_zero_ok", ["feed", "--refresh", "0"]), ("refresh_10s_ok", ["feed", "--refresh", "10s"]),
                ("refresh_compound_ok", ["feed", "--refresh", "1h30m"]), ("pages_zero_ok", ["feed", "--pages", "0"]),
                ("theme_case_insensitive", ["feed", "--theme", "DrAcUlA"]),
                ("theme_valid_env", ["review"])]:
    case(n, args, env=dict(NO_CREDS, BSKYBAN_THEME="Catppuccin") if n == "theme_valid_env" else NO_CREDS)

# deferred command bodies (credential-free or not yet ported)
case("log_empty", ["log"], env=NO_CREDS)
case("themes_preview", ["themes"], env=NO_CREDS)
case("themes_preview_no_color", ["themes"], env=dict(NO_CREDS, NO_COLOR="1"))
case("themes_preview_tty", ["themes"], env=NO_CREDS, tty=100, defer="colour escapes differ (lipgloss vs raw SGR); checked by unit tests")
case("themes_with_args", ["themes", "x"], env=NO_CREDS, cmp="status")
case("feed_without_terminal", ["feed"], routes=TIMELINE, cmp="stderr_first_line")  # both fail after login; wording differs
case("block_command", ["block", "someone"])

# ---------------------------------------------------------------- Phase 1: login and API errors
case("login_ok_unknown_command", ["bogus"], cmp="status")
case("login_401_json", LIST, routes=[{"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                                      "status": 401, "body": {"error": "AuthenticationRequired", "message": "Invalid identifier or password"}}])
case("login_401_plain", LIST, routes=[{"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                                       "status": 401, "body": "nope"}])
case("login_500_html", LIST, routes=[{"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                                      "status": 500, "body": "<html>boom</html>"}])
case("login_error_sanitized", LIST, routes=[{"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                                             "status": 400, "body": {"error": "Bad\x1b[31mThing", "message": "evil\x1b]0;t\x07 msg\x01"}}])
case("login_bad_json", LIST, routes=[{"method": "POST", "path": "/xrpc/com.atproto.server.createSession",
                                      "body": "not json"}], cmp="stderr_first_line")
case("timeline_500", LIST, routes=[tl([], status=500, body="<html>x</html>")])
case("timeline_api_error", LIST, routes=[tl([], status=400, body={"error": "ExpiredToken", "message": "Token has expired"})])
case("timeline_error_on_page_two", LIST, routes=[PAGE1, tl([], cursor="p2", status=502, body={"error": "UpstreamFailure", "message": "bad gateway"})])
case("timeline_not_json", LIST, routes=[tl([], body="garbage")], cmp="stderr_first_line")
case("timeline_no_route", LIST, routes=[])

# ---------------------------------------------------------------- Phase 1: feed --list
case("list_empty_feed", LIST, routes=[tl([])])
case("list_null_feed", LIST, routes=[tl([], body={"feed": None, "cursor": None})])
case("list_empty_page_with_cursor", LIST, routes=[tl([], next="p2"), PAGE2])
case("list_default_pages", LIST, routes=TIMELINE, files={"state.json": STATE})
case("list_default_json", LIST + ["--json"], routes=TIMELINE, files={"state.json": STATE}, jsonl=True)
case("list_json_bytes", LIST + ["--json"], routes=TIMELINE, files={"state.json": STATE})
case("list_backlog", LIST + ["--backlog"], routes=TIMELINE, files={"state.json": STATE})
case("list_backlog_json", LIST + ["--backlog", "--json"], routes=TIMELINE, files={"state.json": STATE}, jsonl=True)
case("list_pages_1", LIST + ["--pages", "1"], routes=TIMELINE, files={"state.json": STATE})
case("list_pages_2", LIST + ["--pages", "2"], routes=TIMELINE, files={"state.json": STATE})
case("list_pages_4", LIST + ["--pages", "4"], routes=TIMELINE, files={"state.json": STATE})
case("list_pages_0_is_one", LIST + ["--pages", "0"], routes=TIMELINE, files={"state.json": STATE})
case("list_no_state_file", LIST, routes=TIMELINE)
case("list_bare_flags_default_command", ["--list"], routes=TIMELINE, files={"state.json": STATE})
case("list_json_without_list_is_tui", ["--json"], routes=TIMELINE, cmp="stderr_first_line")
case("list_ignores_refresh_flag", LIST + ["--refresh", "0", "--fancy"], routes=TIMELINE, files={"state.json": STATE})
case("list_theme_flag_valid", LIST + ["--theme", "dracula"], routes=TIMELINE)

# ---------------------------------------------------------------- Phase 1: terminal width
case("list_tty_60", LIST, routes=TIMELINE, files={"state.json": STATE}, tty=60)
case("list_tty_20", LIST, routes=TIMELINE, files={"state.json": STATE}, tty=20)
case("list_tty_200", LIST, routes=TIMELINE, files={"state.json": STATE}, tty=200)
case("list_tty_json_untruncated", LIST + ["--json"], routes=TIMELINE, files={"state.json": STATE}, tty=30, jsonl=True)

# ---------------------------------------------------------------- Phase 1: state file shapes
ONE = [tl([entry("a1", A, "post", by=R1)])]
for n, content, bad in [("state_empty_object", "{}", False), ("state_null", "null", False),
                        ("state_entries_null", '{"entries":null}', False),
                        ("state_entry_null", '{"entries":{"did:a":null}}', False),
                        ("state_entry_empty_object", '{"entries":{"did:a":{}}}', False),
                        ("state_entry_skipped_missing_at", '{"entries":{"did:a":{"decision":"skipped"}}}', False),
                        ("state_unknown_fields", '{"x":1,"entries":{"did:a":{"decision":"kept","zzz":[1]}}}', False),
                        ("state_empty_file", "", True), ("state_truncated", '{"entries":{', True),
                        ("state_array", "[]", True), ("state_entries_array", '{"entries":[]}', True),
                        ("state_bad_time", '{"entries":{"did:a":{"decision":"kept","at":"yesterday"}}}', True),
                        ("state_trailing_garbage", "{} x", True)]:
    case(n, LIST, routes=ONE, files={"state.json": content}, cmp="stderr_first_line" if bad else "exact")
    if n in ("state_entry_skipped_missing_at",):
        case(n + "_backlog", LIST + ["--backlog"], routes=ONE, files={"state.json": content})
case("state_is_a_directory", LIST, routes=ONE, files={"state.json/x": "y"}, cmp="status")
case("state_malformed_before_login", LIST, routes=[], files={"state.json": "{"}, cmp="stderr_first_line")

# ---------------------------------------------------------------- Phase 1: path resolution
XDG_STATE = {"entries": {"did:a": {"handle": "a.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}
TWO = [tl([entry("a1", A, "from a", by=R1), entry("l1", L, "from l", by=R1)])]
PATHS_ENV = {"BSKYBAN_STATE": None, "BSKYBAN_AUDIT": None}
case("xdg_absolute", LIST, routes=TWO, env=dict(PATHS_ENV, XDG_STATE_HOME="{TMP}/xdg"),
     files={"xdg/bskyban/state.json": XDG_STATE})
case("xdg_relative_ignored", LIST, routes=TWO, env=dict(PATHS_ENV, XDG_STATE_HOME="rel/dir"),
     files={"home/.local/state/bskyban/state.json": XDG_STATE, "rel/dir/bskyban/state.json": "{}"})
case("xdg_empty_ignored", LIST, routes=TWO, env=dict(PATHS_ENV, XDG_STATE_HOME=""),
     files={"home/.local/state/bskyban/state.json": XDG_STATE})
case("xdg_unset_uses_home", LIST, routes=TWO, env=PATHS_ENV, files={"home/.local/state/bskyban/state.json": XDG_STATE})
case("state_env_overrides_xdg", LIST, routes=TWO, env=dict(XDG_STATE_HOME="{TMP}/xdg"),
     files={"state.json": XDG_STATE, "xdg/bskyban/state.json": "{}"})
case("home_unset_state_default", LIST, routes=TWO, env=dict(PATHS_ENV, HOME=None))
case("home_unset_audit_default", LIST, routes=TWO, env={"HOME": None, "BSKYBAN_AUDIT": None})
case("home_empty", LIST, routes=TWO, env=dict(PATHS_ENV, HOME=""))
case("home_unset_but_both_overridden", LIST, routes=TWO, env={"HOME": None}, files={"state.json": XDG_STATE})
case("pds_trailing_slash", LIST, routes=TWO, env={"BSKY_PDS": "{PDS}/"})


# ================================================================ Phase 4: decisions, audit, log
def pr(actor, did=None, handle=None, viewer=None, **kw):
    """A getProfile response for `actor` (a handle or DID)."""
    base = actor.replace("did:", "")
    body = {"did": did or ("did:" + base), "handle": handle or (base + ".test"), "displayName": kw.pop("name", base.title()),
            "createdAt": "{AGO:400d}", "postsCount": 10, "followersCount": 20, "followsCount": 30,
            "avatar": "https://cdn/a.jpg", "description": "  Bio   of %s  " % base, "viewer": viewer or {}}
    body.update(kw)
    return {"path": "/xrpc/app.bsky.actor.getProfile", "q": {"actor": actor}, "body": body}


def create(collection, uri=None, status=200, body=None):
    return {"method": "POST", "path": "/xrpc/com.atproto.repo.createRecord", "jbody": {"collection": collection},
            "status": status, "body": body if body is not None else {"uri": uri or "at://did:me/%s/new1" % collection, "cid": "c"}}


def delete(collection, status=200, body=None):
    return {"method": "POST", "path": "/xrpc/com.atproto.repo.deleteRecord", "jbody": {"collection": collection},
            "status": status, "body": body if body is not None else {}}


BLOCK, FOLLOW = "app.bsky.graph.block", "app.bsky.graph.follow"
HAVE = {"entries": {"did:old": {"handle": "old.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}
BLOCKED_VIEW = {"blocking": "at://did:me/app.bsky.graph.block/rkb"}
FOLLOWING_VIEW = {"following": "at://did:me/app.bsky.graph.follow/rkf"}
P_NEW = pr("newbie")
P_BLOCKED = pr("bad", viewer=BLOCKED_VIEW)
P_FRIEND = pr("friend", viewer=FOLLOWING_VIEW)
ALL_WRITES = [create(BLOCK), create(FOLLOW), delete(BLOCK), delete(FOLLOW)]

# ---- block
case("block_new", ["block", "newbie"], routes=[P_NEW, create(BLOCK)])
case("block_new_by_did_with_at_sign", ["block", "@did:newbie"], routes=[pr("did:newbie"), create(BLOCK)])
case("block_dry_run", ["block", "--dry-run", "newbie"], routes=[P_NEW, create(BLOCK)])
case("block_already_blocked", ["block", "bad"], routes=[P_BLOCKED])
case("block_already_blocked_dry_run", ["block", "--dry-run", "bad"], routes=[P_BLOCKED])
case("block_keeps_existing_state_entries", ["block", "newbie"], routes=[P_NEW, create(BLOCK)], files={"state.json": HAVE})
case("block_unknown_actor", ["block", "ghost"], routes=[{"path": "/xrpc/app.bsky.actor.getProfile", "status": 400, "body": {"error": "InvalidRequest", "message": "Profile not found"}}])
case("block_create_fails", ["block", "newbie"], routes=[P_NEW, create(BLOCK, status=500, body={"error": "InternalServerError", "message": "oops"})])
case("block_partial_failure", ["block", "newbie", "ghost", "friend"], routes=[P_NEW, P_FRIEND, create(BLOCK)])
case("block_several", ["block", "newbie", "friend"], routes=[P_NEW, P_FRIEND, create(BLOCK)])
case("block_ids_from_stdin", ["block", "-"], routes=[P_NEW, P_FRIEND, create(BLOCK)], stdin="newbie  extra words\n\n  @friend\n")
case("block_stdin_empty_is_usage", ["block", "-"], routes=[], stdin="\n   \n", cmp="status")
case("block_state_save_fails", ["block", "newbie"], routes=[P_NEW, create(BLOCK)], files={"state.json.tmp/x": "y"}, cmp="no_stderr")
case("block_audit_unwritable", ["block", "newbie"], routes=[P_NEW, create(BLOCK)], env={"BSKYBAN_AUDIT": "{TMP}/state.json.tmp/audit.jsonl"}, files={"state.json.tmp": "a file, not a dir"}, cmp="no_stderr")
case("block_sanitizes_ids_in_errors", ["block", "evil\x1b[31m"], routes=[])

# ---- keep
case("keep_new", ["keep", "newbie"], routes=[P_NEW])
case("keep_dry_run", ["keep", "--dry-run", "newbie"], routes=[P_NEW])
case("keep_over_existing_block_leaves_remote_alone", ["keep", "bad"], routes=[P_BLOCKED], files={"state.json": {"entries": {"did:bad": {"handle": "bad.test", "decision": "blocked", "at": "2024-05-01T10:00:00Z"}}}})

# ---- follow
case("follow_new", ["follow", "newbie"], routes=[P_NEW, create(FOLLOW)])
case("follow_dry_run", ["follow", "--dry-run", "newbie"], routes=[P_NEW, create(FOLLOW)])
case("follow_already_following", ["follow", "friend"], routes=[P_FRIEND])
case("follow_already_following_dry_run", ["follow", "--dry-run", "friend"], routes=[P_FRIEND])
case("follow_create_fails", ["follow", "newbie"], routes=[P_NEW, create(FOLLOW, status=400, body={"error": "InvalidRequest", "message": "bad subject"})])

# ---- unfollow
case("unfollow_decided_follow_clears_state", ["unfollow", "friend"], routes=[P_FRIEND, delete(FOLLOW)],
     files={"state.json": {"entries": {"did:friend": {"handle": "friend.test", "decision": "followed", "at": "2024-05-01T10:00:00Z"}}}})
case("unfollow_keeps_other_decisions", ["unfollow", "friend"], routes=[P_FRIEND, delete(FOLLOW)],
     files={"state.json": {"entries": {"did:friend": {"handle": "friend.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}})
case("unfollow_not_following", ["unfollow", "newbie"], routes=[P_NEW, delete(FOLLOW)])
case("unfollow_dry_run", ["unfollow", "--dry-run", "friend"], routes=[P_FRIEND, delete(FOLLOW)])
case("unfollow_delete_fails", ["unfollow", "friend"], routes=[P_FRIEND, delete(FOLLOW, status=500, body={"error": "InternalServerError", "message": "no"})])

# ---- unblock
case("unblock_blocked", ["unblock", "bad"], routes=[P_BLOCKED, delete(BLOCK)],
     files={"state.json": {"entries": {"did:bad": {"handle": "bad.test", "decision": "blocked", "at": "2024-05-01T10:00:00Z"}}}})
case("unblock_state_only", ["unblock", "newbie"], routes=[P_NEW, delete(BLOCK)], files={"state.json": {"entries": {"did:newbie": {"handle": "newbie.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}})
case("unblock_nothing_recorded_still_writes_state", ["unblock", "newbie"], routes=[P_NEW, delete(BLOCK)])
case("unblock_dry_run", ["unblock", "--dry-run", "bad"], routes=[P_BLOCKED, delete(BLOCK)])
case("unblock_delete_fails", ["unblock", "bad"], routes=[P_BLOCKED, delete(BLOCK, status=500, body={"error": "InternalServerError", "message": "no"})])

# ---- baseline, list
FOLLOWERS_P1 = {"path": "/xrpc/app.bsky.graph.getFollowers", "q": {"actor": "did:me", "limit": "100", "cursor": None},
                "body": {"followers": [prof("did:f1", "f1.test", "F One"), prof("did:f2", "f2.test", "F Two", following="at://x/follow/1"),
                                       prof("did:f3", "f3.test", "F Three"), prof("did:d", "d.test", "Decided")], "cursor": "c2"}}
FOLLOWERS_P2 = {"path": "/xrpc/app.bsky.graph.getFollowers", "q": {"actor": "did:me", "limit": "100", "cursor": "c2"},
                "body": {"followers": [prof("did:f4", "f4.test", "F Four"), prof("did:e", "e.test", "Skipped"), prof("did:blk", "blk.test", "B", blocking="at://x/block/1")]}}
FSTATE = {"entries": {"did:d": {"handle": "d.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"},
                      "did:e": {"handle": "e.test", "decision": "skipped", "at": "2024-05-01T10:00:00Z"}}}
PROFILES = {"path": "/xrpc/app.bsky.actor.getProfiles", "body": {"profiles": [
    {"did": "did:f1", "handle": "f1.test", "displayName": "F One", "createdAt": "{AGO:400d}", "postsCount": 5, "followersCount": 6, "followsCount": 7, "avatar": "x", "description": "bio one"},
    {"did": "did:f3", "handle": "f3.test", "displayName": "F Three", "createdAt": "{AGO:10d}", "postsCount": 0, "followersCount": 0, "followsCount": 0},
    {"did": "did:d", "handle": "d.test", "displayName": "Decided", "createdAt": "{AGO:3h}", "postsCount": 1, "followersCount": 2, "followsCount": 3},
    {"did": "did:e", "handle": "e.test", "createdAt": "{AGO:5m}", "description": "line1\nline2 \x1b[31mred"}]}}
case("baseline_marks_new_followers", ["baseline"], routes=[FOLLOWERS_P1, FOLLOWERS_P2], files={"state.json": FSTATE})
case("baseline_without_state_file", ["baseline"], routes=[FOLLOWERS_P1, FOLLOWERS_P2])
case("baseline_followers_error", ["baseline"], routes=[{"path": "/xrpc/app.bsky.graph.getFollowers", "status": 500, "body": "x"}])
case("baseline_state_save_fails", ["baseline"], routes=[FOLLOWERS_P1, FOLLOWERS_P2], files={"state.json.tmp/x": "y"}, cmp="no_stderr")
for n, a in [("default", []), ("all", ["--all"]), ("backlog", ["--backlog"]), ("since_2", ["--since", "2"]), ("json", ["--json"]), ("all_json", ["--all", "--json"])]:
    case("cmd_list_" + n, ["list"] + a, routes=[FOLLOWERS_P1, FOLLOWERS_P2, PROFILES], files={"state.json": FSTATE}, jsonl=bool("json" in n))
case("cmd_list_profiles_error", ["list"], routes=[FOLLOWERS_P1, FOLLOWERS_P2, {"path": "/xrpc/app.bsky.actor.getProfiles", "status": 502, "body": {"error": "Upstream", "message": "down"}}])
case("cmd_list_nobody_new", ["list"], routes=[{"path": "/xrpc/app.bsky.graph.getFollowers", "body": {"followers": []}}])

# ---- show
RECENT = {"path": "/xrpc/app.bsky.feed.getAuthorFeed", "q": {"filter": "posts_with_replies", "limit": "5"}, "body": {"feed": [
    entry("s1", prof("did:newbie", "newbie.test"), "first recent post", created="{AGO:2h}"),
    entry("s2", prof("did:newbie", "newbie.test"), "reply ", reply={"parent": {}}, created="{AGO:1d}"),
    entry("s3", prof("did:x", "x.test"), "reposted thing", by=prof("did:newbie", "newbie.test"), created="{AGO:9d}", embed=LINK_EMBED)]}}
case("show_basic", ["show", "newbie"], routes=[P_NEW, RECENT])
case("show_no_posts", ["show", "newbie"], routes=[P_NEW, {"path": "/xrpc/app.bsky.feed.getAuthorFeed", "body": {"feed": []}}])
case("show_recent_posts_error_still_succeeds", ["show", "newbie"], routes=[P_NEW, {"path": "/xrpc/app.bsky.feed.getAuthorFeed", "status": 400, "body": {"error": "BlockedActor", "message": "blocked"}}])
case("show_blocked_status_wins", ["show", "bad"], routes=[P_BLOCKED, RECENT], files={"state.json": {"entries": {"did:bad": {"handle": "bad.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}})
case("show_kept_status", ["show", "newbie"], routes=[P_NEW, RECENT], files={"state.json": {"entries": {"did:newbie": {"handle": "newbie.test", "decision": "kept", "at": "2024-05-01T10:00:00Z"}}}})
case("show_bio_with_url_piped_stays_plain", ["show", "linky"], routes=[pr("linky", description="see https://example.com/x. and example.org\x1b[1m!"), RECENT])
case("show_missing_profile_fields", ["show", "bare"], routes=[{"path": "/xrpc/app.bsky.actor.getProfile", "body": {"did": "did:bare", "handle": "bare.test"}}, RECENT])
case("show_unknown_actor", ["show", "ghost"], routes=[])

# ---- log (reads the audit file; no login needed)
NOW_AGO = lambda **kw: "{AGO:%s}" % kw["a"]
AUDIT = "\n".join(json.dumps(e) for e in [
    {"ts": "2024-01-01T10:00:00Z", "action": "block", "did": "did:a", "handle": "alice.test", "source": "cli", "ok": True, "record": "at://r1"},
    {"ts": "2024-01-02T11:30:05Z", "action": "follow", "did": "did:b", "handle": "bob.test", "source": "tui:following", "via": "carol.test", "ok": True},
    {"ts": "2024-01-03T12:00:00Z", "action": "unblock", "did": "did:a", "handle": "alice.test", "source": "tui:reposts", "ok": False, "error": "boom\x1b[31m", "detail": "lifted by kept"},
    {"ts": "2024-01-04T00:00:00Z", "action": "keep", "did": "did:c", "handle": "carol.test", "source": "cli", "dryRun": True, "ok": True},
    {"ts": "not a time", "action": "post", "did": "did:me", "handle": "me.test", "source": "cli", "ok": True, "detail": "weird <&>"},
    {"ts": "2099-01-01T00:00:00Z", "action": "baseline", "source": "cli", "ok": True, "detail": "3 followers marked as old"},
]) + "\n"
NOCRED = dict(NO_CREDS)
case("log_default", ["log"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_json", ["log", "--json"], env=NOCRED, files={"audit.jsonl": AUDIT}, jsonl=True)
case("log_json_bytes", ["log", "--json"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_action_filter", ["log", "--action", "block"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_who_handle_substring", ["log", "--who", "@lice"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_who_did_exact", ["log", "--who", "did:c"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_who_did_partial_does_not_match", ["log", "--who", "did:"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_last_n", ["log", "-n", "2"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_n_zero_means_all", ["log", "-n", "0"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_since_days", ["log", "--since", "7d"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_since_hours", ["log", "--since", "48h"], env=NOCRED, files={"audit.jsonl": AUDIT})
case("log_since_bad", ["log", "--since", "soon"], env=NOCRED, files={"audit.jsonl": AUDIT}, cmp="status")
case("log_missing_file_is_empty", ["log"], env=NOCRED)
case("log_blank_lines_and_null", ["log"], env=NOCRED, files={"audit.jsonl": '\n\nnull\n{"action":"x","source":"cli","ok":true,"handle":"h"}\n  \n'})
case("log_malformed_line", ["log"], env=NOCRED, files={"audit.jsonl": '{"action":"a"}\n{oops\n'}, cmp="stderr_first_line")
case("log_array_line_is_an_error", ["log"], env=NOCRED, files={"audit.jsonl": '{"action":"a"}\n[]\n'}, cmp="stderr_first_line")
case("log_default_path_from_xdg", ["log"], env=dict(NOCRED, BSKYBAN_AUDIT=None, XDG_STATE_HOME="{TMP}/xdg"), files={"xdg/bskyban/audit.jsonl": AUDIT})
case("log_no_home_no_override", ["log"], env=dict(NOCRED, BSKYBAN_AUDIT=None, HOME=None))
case("log_unicode_alignment", ["log"], env=NOCRED, files={"audit.jsonl": json.dumps({"ts": "2024-01-01T10:00:00Z", "action": "block", "handle": "日本語.test", "source": "cli", "ok": True}) + "\n"})

# ---- the decisions leave a trail `log` can read back (Rust-written files must satisfy Go's reader)
case("block_then_log_reads_back", ["block", "newbie"], routes=[P_NEW, create(BLOCK)], jsonl=False)
