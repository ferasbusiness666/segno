"""Live check of the lyrics and search sources Segno uses, run on GitHub's servers (they can
reach YouTube Music and NetEase; the dev sandbox can't). Prints a summary and saves the raw
answers to out/ so tests can be written against real ones. Standard library only."""
import json, os, sys, time, urllib.parse, urllib.request

OUT = sys.argv[1] if len(sys.argv) > 1 else "out"
os.makedirs(OUT, exist_ok=True)
YTM = "https://music.youtube.com/youtubei/v1/"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
WEB_REMIX = {"clientName": "WEB_REMIX", "clientVersion": "1." + time.strftime("%Y%m%d", time.gmtime()) + ".01.00", "hl": "en", "gl": "US"}
ANDROID_MUSIC = {"clientName": "ANDROID_MUSIC", "clientVersion": "7.21.50", "hl": "en", "gl": "US"}
SONGS = "EgWKAQIIAWoMEA4QChADEAQQCRAF"
VIDEOS = "EgWKAQIQAWoMEA4QChADEAQQCRAF"

def save(name, data):
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=1)

def post(endpoint, client, body):
    body = dict(body, context={"client": client})
    req = urllib.request.Request(YTM + endpoint + "?prettyPrint=false", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", "User-Agent": UA, "Origin": "https://music.youtube.com"})
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.load(r)

def get(url, headers=None):
    req = urllib.request.Request(url, headers=dict({"User-Agent": UA}, **(headers or {})))
    with urllib.request.urlopen(req, timeout=15) as r:
        return r.read().decode("utf-8", "replace")

def walk(v, key, out):
    if isinstance(v, dict):
        if key in v:
            out.append(v[key])
        for x in v.values():
            walk(x, key, out)
    elif isinstance(v, list):
        for x in v:
            walk(x, key, out)
    return out

def runs(t):
    if isinstance(t, str):
        return t
    if not isinstance(t, dict):
        return ""
    return t.get("simpleText") or "".join(r.get("text", "") for r in t.get("runs", []))

def rows(answer):
    out = []
    for r in walk(answer, "musicResponsiveListItemRenderer", []):
        cols = [runs(c["musicResponsiveListItemFlexColumnRenderer"].get("text")) for c in r.get("flexColumns", [])]
        vid = (r.get("playlistItemData") or {}).get("videoId")
        out.append((vid, cols))
    return out

def probe(label, query):
    print(f"\n=== {label}: {query}")
    for kind, params in (("songs", SONGS), ("videos", VIDEOS)):
        try:
            a = post("search", WEB_REMIX, {"query": query, "params": params})
        except Exception as e:
            print(f"  search {kind}: FAILED {e}"); continue
        save(f"{label}-search-{kind}.json", a)
        rs = rows(a)
        print(f"  search {kind}: {len(rs)} rows")
        for vid, cols in rs[:3]:
            print(f"    {vid} | " + " | ".join(cols))
        if not rs or not rs[0][0]:
            continue
        vid = rs[0][0]
        try:
            n = post("next", WEB_REMIX, {"videoId": vid, "isAudioOnly": True, "enablePersistentPlaylistPanel": True, "tunerSettingValue": "AUTOMIX_SETTING_NORMAL", "playlistId": "RDAMVM" + vid})
        except Exception as e:
            print(f"  next {vid}: FAILED {e}"); continue
        save(f"{label}-next-{kind}.json", n)
        tabs = walk(n, "tabRenderer", [])
        lyr = None
        for t in tabs:
            be = (t.get("endpoint") or {}).get("browseEndpoint") or {}
            pt = (((be.get("browseEndpointContextSupportedConfigs") or {}).get("browseEndpointContextMusicConfig") or {}).get("pageType"))
            print(f"    tab {runs(t.get('title')) or t.get('title')}: {pt} {be.get('browseId')}")
            if pt == "MUSIC_PAGE_TYPE_TRACK_LYRICS":
                lyr = be.get("browseId")
        panels = walk(n, "playlistPanelVideoRenderer", [])
        if panels:
            p = panels[0]
            vt = walk(p.get("navigationEndpoint", {}), "musicVideoType", [])
            print(f"    first in queue: {p.get('videoId')} {runs(p.get('title'))} | {runs(p.get('longBylineText'))} | {runs(p.get('lengthText'))} | {vt}")
        cps = walk(n, "counterpart", [])
        for c in cps[:1]:
            cp = walk(c, "playlistPanelVideoRenderer", [])
            if cp:
                print(f"    counterpart: {cp[0].get('videoId')} {runs(cp[0].get('title'))} | {runs(cp[0].get('lengthText'))} | {walk(cp[0].get('navigationEndpoint', {}), 'musicVideoType', [])}")
        if lyr:
            for cname, client in (("mobile", ANDROID_MUSIC), ("web", WEB_REMIX)):
                try:
                    b = post("browse", client, {"browseId": lyr})
                except Exception as e:
                    print(f"    lyrics {cname}: FAILED {e}"); continue
                save(f"{label}-lyrics-{kind}-{cname}.json", b)
                timed = walk(b, "timedLyricsData", [])
                desc = walk(b, "musicDescriptionShelfRenderer", [])
                if timed:
                    lines = timed[0]
                    print(f"    lyrics {cname}: TIMED {len(lines)} lines; first: {lines[0].get('lyricLine')!r} {lines[0].get('cueRange')}")
                elif desc:
                    print(f"    lyrics {cname}: plain, {len(runs(desc[0].get('description')))} chars; footer: {runs(desc[0].get('footer'))}")
                else:
                    print(f"    lyrics {cname}: none; keys {list(b.keys())}")

def netease(label, query):
    print(f"\n=== NetEase {label}: {query}")
    try:
        t = get("https://music.163.com/api/search/get/web?" + urllib.parse.urlencode({"s": query, "type": 1, "offset": 0, "limit": 5}), {"Referer": "https://music.163.com/"})
        a = json.loads(t)
        save(f"netease-{label}-search.json", a)
        songs = ((a.get("result") or {}).get("songs")) or []
        print(f"  search: code {a.get('code')}, {len(songs)} songs")
        for s in songs[:3]:
            print(f"    {s.get('id')} | {s.get('name')} | {', '.join(x.get('name','') for x in s.get('artists', []))} | {s.get('duration')} ms")
        if songs:
            l = json.loads(get(f"https://music.163.com/api/song/lyric?id={songs[0]['id']}&lv=1&kv=1&tv=-1", {"Referer": "https://music.163.com/"}))
            save(f"netease-{label}-lyric.json", l)
            lrc = (l.get("lrc") or {}).get("lyric") or ""
            print(f"  lyric: code {l.get('code')}, {len(lrc.splitlines())} lines; starts {lrc[:80]!r}")
    except Exception as e:
        print(f"  FAILED {e}")

for label, q in (("getlucky", "daft punk get lucky"), ("thickofit", "ksi thick of it"), ("shoftkalam", "marwan pablo shoft kalam"), ("blindinglights", "the weeknd blinding lights")):
    try:
        probe(label, q)
    except Exception as e:
        import traceback; traceback.print_exc()
for label, q in (("getlucky", "get lucky daft punk"), ("blindinglights", "blinding lights the weeknd"), ("shoftkalam", "marwan pablo")):
    netease(label, q)
