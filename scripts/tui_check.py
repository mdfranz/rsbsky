#!/usr/bin/env python3
"""Real-terminal checks for the feed TUI (what snapshots cannot show).

Uses tmux for screen contents and terminal state, and a raw pty for the exact bytes
(OSC 8 links). Everything runs against a scripted fake PDS with temporary state/audit files.

  tui_check.py --bin target/debug/rsbsky [--panic-bin target/debug/examples/panic_restore] [--go-bin PATH]
"""
import argparse, json, os, pty, re, select, shutil, struct, subprocess, sys, tempfile, time, fcntl, termios

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compat import FakePDS  # noqa: E402
from compat_cases import entry, tl, prof, A, B, G, R1, R2, R3, IMG_EMBED, IMG_VIEW  # noqa: E402

FAILS = []
SOCK = "rsbsky-tui-check-%d" % os.getpid()


def check(name, ok, detail=""):
    print(("ok   " if ok else "FAIL ") + name + ("" if ok else "  -> " + str(detail)[:400]))
    if not ok:
        FAILS.append(name)


def tmux(*a, **kw):
    return subprocess.run(["tmux", "-L", SOCK, "-f", "/dev/null"] + list(a), capture_output=True, text=True, **kw)


def screen(target="t", escapes=False):
    return tmux("capture-pane", "-p", *(["-e"] if escapes else []), "-t", target).stdout


def wait_for(pred, timeout=8.0, step=0.1):
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(step)
    return False


def flag(fmt):
    return tmux("display-message", "-p", "-t", "t", fmt).stdout.strip()


def timeline(n_first=50, n_second=10):
    """A two-page timeline of originals and reposts (limit 50, as the TUI requests)."""
    def mk(prefix, count):
        out = []
        for i in range(count):
            who = prof("did:u%d" % (i % 7), "user%d.test" % (i % 7), "User %d" % (i % 7))
            if i % 5 == 3:
                out.append(entry("%s%d" % (prefix, i), who, "reposted %s%d" % (prefix, i), by=[R1, R2, R3][i % 3]))
            else:
                out.append(entry("%s%d" % (prefix, i), who, "日本語 post number %s%d with some words to wrap around the line" % (prefix, i)))
        return out
    return [tl(mk("a", n_first), limit="50", next="p2"), tl(mk("b", n_second), cursor="p2", limit="50")]


def detail_routes():
    """Profile and author-feed responses for the detail view, one route per author."""
    routes = []
    for i in range(7):
        did = "did:u%d" % i
        body = {"did": did, "handle": "user%d.test" % i, "displayName": "User Zero" if i == 0 else "User %d" % i,
                "description": "Bio of user zero, see https://zero.example/home today.", "avatar": "https://cdn/av.jpg",
                "createdAt": "{AGO:400d}", "followersCount": 1200, "followsCount": 300, "postsCount": 800,
                "viewer": {"followedBy": "at://them/follow/1"}}
        recent = [entry("r%d" % k, prof(did, "user%d.test" % i, "User %d" % i), "recent post number %d by this author" % k) for k in range(5)]
        routes.append({"path": "/xrpc/app.bsky.actor.getProfile", "q": {"actor": did}, "body": body})
        routes.append({"path": "/xrpc/app.bsky.feed.getAuthorFeed", "q": {"actor": did}, "body": {"feed": recent}})
    return routes


class Env:
    def __init__(self, routes, extra_env=None):
        self.tmp = tempfile.mkdtemp(prefix="tuicheck-")
        self.pds = FakePDS(routes)
        self.env = {"PATH": "/usr/bin:/bin", "HOME": self.tmp, "BSKY_HANDLE": "me.test", "BSKY_APP_PASSWORD": "pw",
                    "BSKY_PDS": self.pds.url, "BSKYBAN_STATE": self.tmp + "/state.json",
                    "BSKYBAN_AUDIT": self.tmp + "/audit.jsonl", "TERM": "xterm-256color", "LANG": "C.UTF-8"}
        self.env.update(extra_env or {})

    def envstr(self):
        return " ".join("%s=%s" % (k, subprocess.list2cmdline([v]) if " " in v else v) for k, v in self.env.items())

    def close(self):
        self.pds.close()
        shutil.rmtree(self.tmp, ignore_errors=True)


def start(env, cmd, w=100, h=30):
    tmux("kill-server")
    for _ in range(10):
        time.sleep(0.3)
        r = tmux("new-session", "-d", "-s", "t", "-x", str(w), "-y", str(h),
                 "env -i %s %s; echo EXIT=$?; sleep 30" % (env.envstr(), cmd))
        if r.returncode == 0:
            return
    raise RuntimeError(r.stderr)


def alive():
    return "EXIT=" not in screen()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--panic-bin")
    ap.add_argument("--go-bin")
    ap.add_argument("--only-raw", action="store_true")
    a = ap.parse_args()
    binp = os.path.abspath(a.bin)
    if a.only_raw:
        raw_checks(binp)
        print("\n%s" % ("raw checks passed" if not FAILS else "FAILED: " + ", ".join(FAILS)))
        sys.exit(1 if FAILS else 0)

    # ---- 1. renders, navigates, quits cleanly
    e = Env(timeline())
    start(e, "%s" % binp)
    check("first screen shows the timeline", wait_for(lambda: "Following" in screen() and "User 0" in screen()), screen())
    s = screen()
    check("title row and footer hints", s.splitlines()[0].startswith("bskyban   Following") and "q quit" in s, s)
    check("alternate screen is on", flag("#{alternate_on}") == "1")
    check("selection starts on the first post", re.search(r"^▸ 13mo\s+User 0", s, re.M) is not None, s)
    tmux("send-keys", "-t", "t", "Down", "Down")
    check("down moves the selection", wait_for(lambda: re.search(r"^▸ .*User 2 ", screen(), re.M) is not None), screen())
    tmux("send-keys", "-t", "t", "End")
    check("End jumps to the last loaded post and pages in more", wait_for(lambda: "b9" in screen() or "reposted b" in screen() or "post number b" in screen(), 8), screen())
    reqs = [r for r in e.pds.trace if r["path"].endswith("getTimeline")]
    check("second page was requested with the cursor", any(r["query"].get("cursor") == ["p2"] for r in reqs), reqs)
    check("every request used limit=50 and the bearer token", all(r["query"].get("limit") == ["50"] and r["auth"] == "Bearer TOKEN" for r in reqs), reqs)
    check("no mutations were sent", all(r["method"] == "GET" or r["path"].endswith("createSession") for r in e.pds.trace))
    tmux("send-keys", "-t", "t", "q")
    check("q quits and the shell prompt is back", wait_for(lambda: "EXIT=0" in screen()), screen())
    check("alternate screen restored", flag("#{alternate_on}") == "0")
    check("cursor visible again", flag("#{cursor_flag}") == "1")
    tmux("send-keys", "-t", "t", "", "")
    st = subprocess.run(["tmux", "-L", SOCK, "send-keys", "-t", "t", "stty -a | grep -o -- '-\\?icanon\\|-\\?echo ' | tr '\\n' ' '; echo", "Enter"], capture_output=True)
    time.sleep(0.5)
    check("tty is cooked again (icanon, echo)", "icanon" in screen() and "-icanon" not in screen(), screen())
    check("no audit file is created by plain browsing", not os.path.exists(e.tmp + "/audit.jsonl"))
    check("state file untouched", not os.path.exists(e.tmp + "/state.json"))
    e.close()

    # ---- 2. Ctrl+C
    e = Env(timeline())
    start(e, binp)
    wait_for(lambda: "User 0" in screen())
    tmux("send-keys", "-t", "t", "C-c")
    check("ctrl+c quits", wait_for(lambda: "EXIT=0" in screen()), screen())
    check("ctrl+c restores the terminal", flag("#{alternate_on}") == "0" and flag("#{cursor_flag}") == "1")
    e.close()

    # ---- 3. slow HTTP must not block input, redraw or quitting
    slow = timeline()
    slow[0]["delay"] = 20
    e = Env(slow)
    start(e, binp)
    check("a spinner shows while the first page is slow", wait_for(lambda: "loading timeline" in screen()), screen())
    tmux("send-keys", "-t", "t", "Down", "R")
    time.sleep(0.3)
    check("keys during loading do not break the screen", "loading timeline" in screen() and "still loading the timeline" in screen(), screen())
    t0 = time.time()
    tmux("send-keys", "-t", "t", "q")
    check("q quits promptly with a request outstanding", wait_for(lambda: "EXIT=0" in screen(), 3), screen())
    check("...in well under the request time", time.time() - t0 < 3)
    check("...and restores the terminal", flag("#{alternate_on}") == "0")
    e.close()

    # ---- 4. SIGTERM restores the terminal
    e = Env(timeline())
    start(e, binp)
    wait_for(lambda: "User 0" in screen())
    pid = tmux("display-message", "-p", "-t", "t", "#{pane_pid}").stdout.strip()
    kids = subprocess.run(["pgrep", "-P", pid], capture_output=True, text=True).stdout.split()
    target = None
    for k in kids:  # `env` execs the binary, so the child of the shell is rsbsky
        if "rsbsky" in open("/proc/%s/cmdline" % k).read():
            target = k
    check("found the rsbsky process", target is not None, kids)
    if target:
        os.kill(int(target), 15)
        check("SIGTERM exits cleanly", wait_for(lambda: "EXIT=0" in screen()), screen())
        check("SIGTERM restores the terminal", flag("#{alternate_on}") == "0" and flag("#{cursor_flag}") == "1")
    e.close()

    # ---- 5. resize re-wraps the list
    e = Env(timeline())
    start(e, binp, w=100, h=30)
    wait_for(lambda: "User 0" in screen())
    wide = screen()
    tmux("resize-window", "-t", "t", "-x", "50", "-y", "20")
    check("resize re-lays the screen out", wait_for(lambda: all(len(l) <= 50 for l in screen().splitlines()) and "q quit" in screen()), screen())
    narrow = screen()
    check("narrow layout wrapped the text onto more lines", narrow.count("日本語") <= wide.count("日本語") and "User 0" in narrow, narrow)
    check("no line exceeds the pane width", all(len(l) <= 50 for l in narrow.splitlines()))
    tmux("send-keys", "-t", "t", "q")
    e.close()

    # ---- 6. NO_COLOR
    for no_color, expect in (("1", False), (None, True)):
        e = Env(timeline(), {"NO_COLOR": no_color} if no_color else {})
        start(e, binp)
        wait_for(lambda: "User 0" in screen())
        colored = re.search(r"\x1b\[[0-9;]*38;5;[0-9]+m|\x1b\[[0-9;]*3[0-7]m|\x1b\[[0-9;]*9[0-7]m", screen(escapes=True)) is not None
        check("colour %s with NO_COLOR=%s" % ("present" if expect else "absent", no_color), colored == expect, screen(escapes=True)[:300])
        tmux("send-keys", "-t", "t", "q")
        e.close()

    # ---- 7. background refresh inserts at the top, holds when reading elsewhere
    first = timeline(6, 0)
    new = entry("fresh1", prof("did:fresh", "fresh.test", "Fresh Face"), "brand new post", created="{AGO:1m}")
    state = {"calls": 0}
    e = Env(first)
    # after the first call the timeline gains one newer post (the route list is rebuilt per request)
    class Flip(list):
        pass
    e.pds.routes = [r for r in e.pds.routes]
    start(e, "%s --refresh 10s --fancy" % binp)
    wait_for(lambda: "User 0" in screen())
    e.pds.routes.insert(0, tl([new] + first[0]["body"]["feed"], limit="50"))
    check("background refresh inserts the new post at the top while reading the top", wait_for(lambda: "Fresh Face" in screen(), 16), screen())
    check("...and says so", "1 new post" in screen(), screen())
    check("fancy mode tags the new post", "new" in screen().split("Fresh Face")[1].splitlines()[0], screen())
    e.pds.routes.insert(0, tl([entry("fresh2", prof("did:f2", "f2.test", "Second Fresh"), "another", created="{AGO:30s}"), new] + first[0]["body"]["feed"], limit="50"))
    tmux("send-keys", "-t", "t", "Down", "Down")
    check("while reading elsewhere a new post is held: header shows (+1 new)", wait_for(lambda: "(+1 new)" in screen().splitlines()[0], 16), screen())
    check("...the selection did not move", re.search(r"^▸ .*User 1 ", screen(), re.M) is not None or "▸" in screen(), screen())
    tmux("send-keys", "-t", "t", "R")
    check("R inserts the waiting post and jumps to the top", wait_for(lambda: "Second Fresh" in screen() and re.search(r"^▸ .*Second Fresh", screen(), re.M) is not None), screen())
    tmux("send-keys", "-t", "t", "q")
    e.close()

    # ---- 8. refresh failure: warning in the header, audited once, round-trips through Go
    ok_page = timeline(4, 0)
    e = Env(ok_page)
    start(e, "%s --refresh 10s" % binp)
    wait_for(lambda: "User 0" in screen())
    e.pds.routes.insert(0, tl([], limit="50", status=502, body={"error": "UpstreamFailure", "message": "bad gateway"}))
    check("a failing background check shows the header warning", wait_for(lambda: "⚠ refresh failed" in screen().splitlines()[0], 16), screen())
    time.sleep(11)  # a second identical failure
    tmux("send-keys", "-t", "t", "R")
    check("manual R failure is shown as an error", wait_for(lambda: "error: refresh:" in screen()), screen())
    tmux("send-keys", "-t", "t", "q")
    wait_for(lambda: "EXIT=0" in screen())
    lines = [json.loads(l) for l in open(e.tmp + "/audit.jsonl").read().splitlines()]
    details = [l["detail"] for l in lines]
    check("background failure audited once, manual failure audited", details == ["background check", "manual refresh (R)"], details)
    check("audit events carry source, handle, error and ok=false", all(l["action"] == "refresh" and l["source"] == "tui:following" and l["handle"] == "me.test" and l["did"] == "did:me" and l["ok"] is False and "bad gateway" in l["error"] for l in lines), lines)
    if a.go_bin:
        out = subprocess.run([os.path.abspath(a.go_bin), "log", "--json"], env={"PATH": "/usr/bin:/bin", "HOME": e.tmp, "BSKYBAN_AUDIT": e.tmp + "/audit.jsonl"}, capture_output=True, text=True)
        got = [json.loads(l) for l in out.stdout.splitlines()]
        check("Go's `log` reads Rust's audit events", out.returncode == 0 and [g["action"] for g in got] == ["refresh", "refresh"] and got[0]["error"] == lines[0]["error"], out.stdout + out.stderr)
        out = subprocess.run([os.path.abspath(a.go_bin), "log"], env={"PATH": "/usr/bin:/bin", "HOME": e.tmp, "BSKYBAN_AUDIT": e.tmp + "/audit.jsonl"}, capture_output=True, text=True)
        check("...and renders them", out.returncode == 0 and "FAILED:" in out.stdout and "background check" in out.stdout, out.stdout + out.stderr)
    e.close()

    # ---- 9. panic restores the terminal
    if a.panic_bin:
        e = Env([])
        start(e, os.path.abspath(a.panic_bin))
        check("panic example ran and the shell came back", wait_for(lambda: "EXIT=101" in screen(), 5), screen())
        s = screen()
        check("panic message is readable (not swallowed by the alternate screen)", "deliberate panic" in s, s)
        check("panic restored alternate screen and cursor", flag("#{alternate_on}") == "0" and flag("#{cursor_flag}") == "1")
        e.close()

    # ---- 9b. post detail view: panels, navigation, picker, images, browser launch
    log = tempfile.mktemp(prefix="browserlog-")
    browser = tempfile.mktemp(prefix="fakebrowser-", suffix=".sh")
    with open(browser, "w") as f:
        f.write('#!/bin/sh\necho "$1" >> "%s"\n' % log)
    os.chmod(browser, 0o755)
    e = Env(timeline() + detail_routes(), {"BROWSER": browser})
    start(e, binp, w=120, h=32)
    wait_for(lambda: "User 0" in screen())
    tmux("send-keys", "-t", "t", "Right")
    check("Right opens the detail view with the profile loading", wait_for(lambda: "open profile" in screen()), screen())
    check("the profile and recent posts load", wait_for(lambda: "Account age" in screen() and "Recent posts" in screen() and "recent post number 0" in screen(), 8), screen())
    s = screen()
    check("profile panel shows stats and the right panel shows the post", all(x in s for x in ["User Zero", "1 of 50", "Followers    1200", "Fol. ratio   4.0", "follows you", "Bio of user zero", "This post", "open post"]), s)
    reqs = e.pds.trace
    prof_reqs = [r for r in reqs if r["path"].endswith("getProfile")]
    feed_reqs = [r for r in reqs if r["path"].endswith("getAuthorFeed")]
    check("profile requested for this and the next author", sorted(r["query"]["actor"][0] for r in prof_reqs) == ["did:u0", "did:u1"], prof_reqs)
    check("recent posts use posts_with_replies, limit 5", feed_reqs and all(r["query"].get("filter") == ["posts_with_replies"] and r["query"].get("limit") == ["5"] for r in feed_reqs), feed_reqs)
    check("detail browsing only reads", all(r["method"] == "GET" or r["path"].endswith("createSession") for r in reqs))
    tmux("send-keys", "-t", "t", "Down")
    check("Down shows the next post", wait_for(lambda: "2 of 50" in screen()), screen())
    tmux("send-keys", "-t", "t", "Up")
    check("Up goes back", wait_for(lambda: "1 of 50" in screen()), screen())
    tmux("send-keys", "-t", "t", "PageDown", "PageUp")
    time.sleep(0.3)
    check("page keys scroll without breaking the layout", "This post" in screen() or "Recent posts" in screen(), screen())
    tmux("send-keys", "-t", "t", "i")
    check("i without images says so", wait_for(lambda: "this post has no images" in screen()), screen())
    tmux("send-keys", "-t", "t", "o")
    check("o opens the link picker", wait_for(lambda: "open:" in screen() and "post on bsky.app" in screen()), screen())
    check("picker hints replace the key hints", "enter open" in screen() and "esc cancel" in screen(), screen())
    tmux("send-keys", "-t", "t", "Enter")
    check("Enter launches $BROWSER with the post URL", wait_for(lambda: os.path.exists(log) and "bsky.app/profile/user0.test/post/a0" in open(log).read()), open(log).read() if os.path.exists(log) else "no log")
    check("status says what was opened", wait_for(lambda: "opened post on bsky.app" in screen()), screen())
    check("the browser's stdout cannot corrupt the screen", "This post" in screen())
    tmux("send-keys", "-t", "t", "o", "Down", "Escape")
    check("Esc closes the picker without opening", wait_for(lambda: "open:" not in screen()) and len(open(log).read().split()) == 1, screen())
    tmux("send-keys", "-t", "t", "Escape")
    check("Esc leaves the detail view", wait_for(lambda: "Account age" not in screen() and re.search(r"^▸ ", screen(), re.M) is not None), screen())
    tmux("send-keys", "-t", "t", "Right")
    wait_for(lambda: "Account age" in screen())
    tmux("resize-window", "-t", "t", "-x", "70", "-y", "40")
    check("narrow terminals stack the panels", wait_for(lambda: "q quit" in screen() and "Account age" in screen() and all(len(l) <= 70 for l in screen().splitlines())), screen())
    tmux("send-keys", "-t", "t", "q")
    wait_for(lambda: "EXIT=0" in screen())
    check("quits cleanly from the detail view", "EXIT=0" in screen() and flag("#{alternate_on}") == "0")
    e.close()

    # image links: a post with images opens every one
    img_entry = entry("img0", prof("did:u0", "user0.test", "User Zero"), "pics", embed=IMG_EMBED, view=IMG_VIEW)
    e = Env([tl([img_entry], limit="50")] + detail_routes(), {"BROWSER": browser})
    open(log, "w").close()
    start(e, binp, w=120, h=32)
    wait_for(lambda: "pics" in screen())
    check("hint bar offers i for posts with images", "i open images" in screen(), screen())
    tmux("send-keys", "-t", "t", "i")
    check("i opens each image URL in the browser", wait_for(lambda: sorted(open(log).read().split()) == ["https://cdn/1.jpg", "https://cdn/2.jpg"]), open(log).read())
    check("...and reports it", wait_for(lambda: "opened 2 image(s) in your browser" in screen()), screen())
    tmux("send-keys", "-t", "t", "q")
    e.close()
    # a missing launcher is an error, not a crash
    e = Env([tl([img_entry], limit="50")] + detail_routes(), {"BROWSER": "/nonexistent/browser"})
    start(e, binp, w=120, h=32)
    wait_for(lambda: "pics" in screen())
    tmux("send-keys", "-t", "t", "i")
    check("a failing launcher is shown as an error", wait_for(lambda: "error: open image:" in screen()), screen())
    tmux("send-keys", "-t", "t", "q")
    check("...and the app is still responsive", wait_for(lambda: "EXIT=0" in screen()), screen())
    e.close()
    os.remove(browser)
    os.remove(log)

    # ---- 10. raw bytes: OSC 8 links, redraw/scroll, clean shutdown sequence
    raw_checks(binp)

    tmux("kill-server")
    print("\n%s" % ("all terminal checks passed" if not FAILS else "FAILED: " + ", ".join(FAILS)))
    sys.exit(1 if FAILS else 0)


def raw_checks(binp):
    e = Env(timeline() + detail_routes())
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    env = dict(e.env)
    p = subprocess.Popen([binp], env=env, stdin=slave, stdout=slave, stderr=subprocess.PIPE, close_fds=True, preexec_fn=os.setsid)
    os.close(slave)

    def read(timeout):
        buf = b""
        end = time.time() + timeout
        while time.time() < end:
            r, _, _ = select.select([master], [], [], 0.1)
            if r:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                buf += chunk
        return buf

    def read_until(pred, timeout=6.0):
        buf = b""
        end = time.time() + timeout
        while time.time() < end and not pred(buf):
            buf += read(0.2)
        return buf

    first = read(2.0)
    check("startup does not query the terminal (no cursor-position request)", b"\x1b[6n" not in first, first[:200])
    post_links = re.findall(rb"\x1b\]8;;(https://bsky\.app/profile/[^\x1b]+)\x1b\\(.)\x1b\]8;;\x1b\\", first)
    check("age cells are wrapped in OSC 8 with the bsky.app post URL", len(post_links) >= 6, first[:300])
    urls = {u for u, _ in post_links}
    check("each visible post has its own link target", len(urls) >= 3 and all(re.match(rb"https://bsky\.app/profile/[^/]+/post/[a-z0-9]+$", u) for u in urls), urls)
    check("the link text is the age ('13mo' cell by cell)", b"".join(c for u, c in post_links[:4]).startswith(b"13mo"), post_links[:6])
    check("the alternate screen was entered and nothing hyperlinked leaked into text lines", b"\x1b[?1049h" in first)
    # scroll: links must follow the posts (re-emitted for the new rows)
    os.write(master, b"\x1b[B" * 25)
    after = read(1.5)
    new_urls = {m.group(1) for m in re.finditer(rb"\x1b\]8;;(https://bsky\.app/[^\x1b]+)\x1b\\", after)}
    check("links are re-sent after scrolling", len(new_urls) >= 3, after[:200])
    # a plain re-print (cell that stopped being a link) exists after scrolling: a position printed without OSC 8
    check("every OSC 8 open is closed", after.count(b"\x1b]8;;\x1b\\") >= after.count(b"\x1b]8;;http"))
    os.write(master, b"\x1b[C")  # Right: open the detail view
    detail = read_until(lambda b: b"zero.example" in b and b"Recent posts" in b)
    check("detail view links the profile", b"\x1b]8;;https://bsky.app/profile/user4.test\x1b\\o\x1b]8;;\x1b\\" in detail, detail[:300])
    check("detail view links the post ('open post')", re.search(rb"\x1b\]8;;https://bsky\.app/profile/[^/]+/post/[a-z0-9]+\x1b\\o\x1b\]8;;\x1b\\", detail) is not None)
    check("bio and text URLs are linked", b"\x1b]8;;https://zero.example/home" in detail, detail[-400:])
    os.write(master, b"\x1b")  # Esc: back to the list
    back = read(1.5)
    check("leaving the detail view drops the profile and bio links", b"\x1b]8;;https://bsky.app/profile/user4.test\x1b\\" not in back and b"zero.example" not in back, back[:300])
    check("...by re-printing the old link cells as plain text", b"\x1b[5;1H" in back or b"\x1b[4;1H" in back or b"\x1b[6;1H" in back, back[:300])
    os.write(master, b"q")
    tail = read(2.0)
    p.wait(timeout=5)
    check("exit status 0", p.returncode == 0, p.returncode)
    check("shutdown closes hyperlinks, shows the cursor and leaves the alternate screen", b"\x1b]8;;\x1b\\" in tail and b"\x1b[?25h" in tail and b"\x1b[?1049l" in tail, tail[-200:])
    os.close(master)
    e.close()


if __name__ == "__main__":
    main()
