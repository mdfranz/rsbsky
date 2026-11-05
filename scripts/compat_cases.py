"""Compatibility matrix cases (Phase 0 and 1).

Each case is run against the Go and the Rust binary with an isolated HOME/state/audit and a
scripted fake PDS. Fields: name, args (Rust syntax; go_args overrides), env (None unsets),
files (relative to the temp dir), routes (fake PDS), stdin, tty (columns), jsonl (compare
stdout as parsed JSON lines), cmp (exact | status | stderr_first_line), defer (phase).

Deliberate differences from Go (see docs/compat.md) are expressed with cmp="status".
"""

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
case("log_empty", ["log"], env=NO_CREDS, defer="phase 4")
case("themes_preview", ["themes"], env=NO_CREDS, defer="phase 2")
case("themes_with_args", ["themes", "x"], env=NO_CREDS, cmp="status")
case("feed_tui_stub", ["feed"], routes=TIMELINE, defer="phase 2")
case("block_command", ["block", "someone"], defer="phase 4")

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
case("list_json_without_list_is_tui", ["--json"], routes=TIMELINE, defer="phase 2")
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
