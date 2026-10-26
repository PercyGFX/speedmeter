"""Creates a sample history database for screenshots and testing.

Usage:
    python tools/make_demo_history.py <folder>
    set SPEEDMETER_DATA_DIR=<folder>  (then run SpeedMeter)

Writes <folder>/history.db with ~14 months of daily totals (weekday/weekend rhythm,
evening peaks, a few big download days) and two weeks of minute-level detail.
Never touches your real data in %LOCALAPPDATA%\\SpeedMeter.
"""
import math
import random
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path

GB = 1024 ** 3
MB = 1024 ** 2

SCHEMA = """
CREATE TABLE IF NOT EXISTS minute(ts INTEGER PRIMARY KEY, rx INTEGER NOT NULL DEFAULT 0, tx INTEGER NOT NULL DEFAULT 0,
    peak_rx REAL NOT NULL DEFAULT 0, peak_tx REAL NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS day(day TEXT PRIMARY KEY, rx INTEGER NOT NULL DEFAULT 0, tx INTEGER NOT NULL DEFAULT 0,
    peak_rx REAL NOT NULL DEFAULT 0, peak_tx REAL NOT NULL DEFAULT 0);
"""


def hour_weight(h: int) -> float:
    # Quiet overnight, a lunch bump, a big evening peak.
    return 0.15 + 0.5 * math.exp(-((h - 13) ** 2) / 8) + 1.3 * math.exp(-((h - 21) ** 2) / 6)


def main() -> None:
    folder = Path(sys.argv[1] if len(sys.argv) > 1 else "demo-data")
    folder.mkdir(parents=True, exist_ok=True)
    db = folder / "history.db"
    if db.exists():
        db.unlink()
    rng = random.Random(7)
    con = sqlite3.connect(db)
    con.executescript(SCHEMA)

    today = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
    start = today - timedelta(days=430)
    weights = [hour_weight(h) for h in range(24)]
    wsum = sum(weights)
    d = start
    while d <= today:
        weekend = d.weekday() >= 5
        base = (6.5 if weekend else 4.2) * GB
        season = 1.0 + 0.25 * math.sin((d - start).days / 58.0)
        rx = base * season * rng.uniform(0.55, 1.45)
        if rng.random() < 0.06:  # game or OS update day
            rx += rng.uniform(25, 60) * GB
        tx = rx * rng.uniform(0.05, 0.12)
        if d == today:
            frac = datetime.now().hour / 24 + 0.02
            rx *= frac
            tx *= frac
        peak = rng.uniform(4, 11) * MB if rx < 20 * GB else rng.uniform(38, 92) * MB
        con.execute(
            "INSERT INTO day VALUES(?,?,?,?,?)",
            (d.strftime("%Y-%m-%d"), int(rx), int(tx), peak, peak * rng.uniform(0.05, 0.2)),
        )
        # Minute detail for the last 14 days, spread over the day's hours.
        if (today - d).days <= 14:
            for h in range(24):
                hour_start = d + timedelta(hours=h)
                if hour_start > datetime.now():
                    break
                share = weights[h] / wsum
                for m in range(60):
                    ts = int((hour_start + timedelta(minutes=m)).timestamp())
                    if ts > datetime.now().timestamp():
                        break
                    jitter = rng.uniform(0.2, 1.8)
                    mrx = rx * share / 60 * jitter
                    mtx = tx * share / 60 * jitter
                    con.execute(
                        "INSERT INTO minute VALUES(?,?,?,?,?)",
                        (ts, int(mrx), int(mtx), mrx / 60 * rng.uniform(1.5, 4), mtx / 60 * rng.uniform(1.5, 4)),
                    )
        d += timedelta(days=1)
    con.commit()
    con.close()
    print(f"wrote {db}")


if __name__ == "__main__":
    main()
