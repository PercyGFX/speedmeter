"""Creates a sample history database for screenshots and testing.

Usage:
    python tools/make_demo_history.py <folder>
    set SPEEDMETER_DATA_DIR=<folder>  (then run SpeedMeter)

Writes <folder>/history.db with ~14 months of daily totals (weekday/weekend rhythm,
evening peaks, a few big download days), two weeks of minute-level detail, and
per-app usage for the last 45 days (as if per-app counting was turned on then).
Never touches your real data in %LOCALAPPDATA%\\SpeedMeter.
"""
import glob
import math
import os
import random
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path

GB = 1024 ** 3
MB = 1024 ** 2

# Keep in step with history.rs.
SCHEMA = """
CREATE TABLE IF NOT EXISTS minute(ts INTEGER PRIMARY KEY, rx INTEGER NOT NULL DEFAULT 0, tx INTEGER NOT NULL DEFAULT 0,
    peak_rx REAL NOT NULL DEFAULT 0, peak_tx REAL NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS day(day TEXT PRIMARY KEY, rx INTEGER NOT NULL DEFAULT 0, tx INTEGER NOT NULL DEFAULT 0,
    peak_rx REAL NOT NULL DEFAULT 0, peak_tx REAL NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS app(id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
    file TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS app_day(day TEXT NOT NULL, app INTEGER NOT NULL, rx INTEGER NOT NULL DEFAULT 0,
    tx INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day, app)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS app_hour(ts INTEGER NOT NULL, app INTEGER NOT NULL, rx INTEGER NOT NULL DEFAULT 0,
    tx INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(ts, app)) WITHOUT ROWID;
"""

def find(*candidates: str) -> str:
    """The first of these paths (globs allowed) that exists on this PC, as an app key.
    Apps that exist show their real icons; the last candidate is the fallback."""
    for c in candidates:
        hits = sorted(glob.glob(os.path.expandvars(c)))
        if hits:
            return hits[-1].lower()
    return os.path.expandvars(candidates[-1]).lower()


# (key, name, file, download weight, upload weight)
APPS = [
    (find(r"C:\Program Files\Mozilla Firefox\firefox.exe"), "Firefox", "firefox.exe", 0.30, 0.22),
    (find(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"), "Microsoft Edge", "msedge.exe", 0.09, 0.07),
    (find(r"C:\Program Files (x86)\Steam\steam.exe"), "Steam", "steam.exe", 0.10, 0.02),
    (find(r"%LOCALAPPDATA%\Discord\app-*\Discord.exe"), "Discord", "Discord.exe", 0.05, 0.12),
    (find(r"%APPDATA%\Telegram Desktop\Telegram.exe"), "Telegram Desktop", "Telegram.exe", 0.07, 0.03),
    (find(r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe", r"C:\Program Files\Microsoft VS Code\Code.exe"),
     "Visual Studio Code", "Code.exe", 0.03, 0.03),
    (find(r"%LOCALAPPDATA%\Microsoft\OneDrive\OneDrive.exe", r"C:\Program Files\Microsoft OneDrive\OneDrive.exe"),
     "Microsoft OneDrive", "OneDrive.exe", 0.04, 0.28),
    ("svc:bits,wuauserv", "Background Intelligent Transfer Service, Windows Update", "svchost.exe", 0.05, 0.01),
    ("svc:dosvc", "Delivery Optimization", "svchost.exe", 0.03, 0.04),
    ("system", "System", "", 0.01, 0.02),
    ("", "Other", "", 0.03, 0.06),
]
STEAM, UPDATE = 2, 7
APP_DAYS = 45


def hour_weight(h: int) -> float:
    # Quiet overnight, a lunch bump, a big evening peak.
    return 0.15 + 0.5 * math.exp(-((h - 13) ** 2) / 8) + 1.3 * math.exp(-((h - 21) ** 2) / 6)


def split(rng, rx, tx, big):
    """Shares one period's bytes among the apps. `big` (bytes) goes to a game or an update."""
    wr = [a[3] * rng.uniform(0.4, 1.6) for a in APPS]
    wt = [a[4] * rng.uniform(0.4, 1.6) for a in APPS]
    sr, st = sum(wr), sum(wt)
    out = [[(rx - big) * w / sr, tx * t / st] for w, t in zip(wr, wt)]
    if big:
        out[STEAM if rng.random() < 0.6 else UPDATE][0] += big
    return out


def add_apps(con, table, key, shares):
    for i, (r, t) in enumerate(shares):
        if r + t >= 1:
            con.execute(f"INSERT INTO {table} VALUES(?,?,?,?)", (key, i + 1, int(r), int(t)))


def main() -> None:
    folder = Path(sys.argv[1] if len(sys.argv) > 1 else "demo-data")
    folder.mkdir(parents=True, exist_ok=True)
    db = folder / "history.db"
    if db.exists():
        db.unlink()
    rng = random.Random(7)
    con = sqlite3.connect(db)
    con.executescript(SCHEMA)
    for i, (key, name, file, _, _) in enumerate(APPS):
        con.execute("INSERT INTO app VALUES(?,?,?,?)", (i + 1, key, name, file))

    now = datetime.now()
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    start = today - timedelta(days=430)
    weights = [hour_weight(h) for h in range(24)]
    wsum = sum(weights)
    d = start
    while d <= today:
        weekend = d.weekday() >= 5
        base = (6.5 if weekend else 4.2) * GB
        season = 1.0 + 0.25 * math.sin((d - start).days / 58.0)
        rx = base * season * rng.uniform(0.55, 1.45)
        big = rng.uniform(25, 60) * GB if rng.random() < 0.06 else 0.0  # game or OS update day
        rx += big
        tx = rx * rng.uniform(0.05, 0.12)
        if d == today:
            frac = now.hour / 24 + 0.02
            rx, tx, big = rx * frac, tx * frac, big * frac
        peak = rng.uniform(4, 11) * MB if rx < 20 * GB else rng.uniform(38, 92) * MB
        day = d.strftime("%Y-%m-%d")
        con.execute("INSERT INTO day VALUES(?,?,?,?,?)", (day, int(rx), int(tx), peak, peak * rng.uniform(0.05, 0.2)))
        with_apps = (today - d).days < APP_DAYS
        if with_apps:
            add_apps(con, "app_day", day, split(rng, rx, tx, big))
        # Minute detail for the last 14 days, spread over the day's hours.
        if (today - d).days <= 14:
            for h in range(24):
                hour_start = d + timedelta(hours=h)
                if hour_start > now:
                    break
                share = weights[h] / wsum
                hrx = htx = 0.0
                for m in range(60):
                    ts = int((hour_start + timedelta(minutes=m)).timestamp())
                    if ts > now.timestamp():
                        break
                    jitter = rng.uniform(0.2, 1.8)
                    mrx = rx * share / 60 * jitter
                    mtx = tx * share / 60 * jitter
                    hrx += mrx
                    htx += mtx
                    con.execute(
                        "INSERT INTO minute VALUES(?,?,?,?,?)",
                        (ts, int(mrx), int(mtx), mrx / 60 * rng.uniform(1.5, 4), mtx / 60 * rng.uniform(1.5, 4)),
                    )
                if with_apps:
                    add_apps(con, "app_hour", int(hour_start.timestamp()), split(rng, hrx, htx, big * share))
        d += timedelta(days=1)
    con.commit()
    con.close()
    print(f"wrote {db}")


if __name__ == "__main__":
    main()
