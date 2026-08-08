#!/usr/bin/env -S uv run --script
# /// script
# dependencies = []
# requires-python = ">=3.12"
# ///

# One-off import of album-data/all_albums_export.csv's `rating` column into
# albums.db. Runs exactly once against real data; correctness is established
# by inspecting the result (spec.md §6), not by a test suite. Must run after
# a `scan`, since it only updates rows that already exist.

import argparse
import csv
import sqlite3
import sys
from datetime import datetime, timezone


def now_rfc3339() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def read_rows(csv_path: str) -> list[dict]:
    with open(csv_path, newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def check_all_paths_exist(conn: sqlite3.Connection, rows: list[dict]) -> None:
    known = {r[0] for r in conn.execute("SELECT path FROM albums")}
    missing = sorted({row["file"].rstrip("/") for row in rows} - known)
    if missing:
        print(
            f"error: {len(missing)} CSV path(s) not found in albums.path — "
            "the library has moved since spec.md §2.6 measured this as safe. "
            "Stopping without writing anything:",
            file=sys.stderr,
        )
        for path in missing:
            print(f"  {path}", file=sys.stderr)
        sys.exit(1)


def apply_rating(conn: sqlite3.Connection, path: str, value: str, rated_at: str) -> str:
    value = value.strip()
    if value == "":
        return "blank"
    if value == "?":
        conn.execute(
            "UPDATE albums SET rating = NULL, needs_relisten = 1 WHERE path = ?",
            (path,),
        )
        return "needs_relisten"
    if value == "c":
        conn.execute(
            "UPDATE albums SET is_classical = 1 WHERE path = ?",
            (path,),
        )
        return "classical"
    if value in {"0", "1", "2", "3", "4", "5"}:
        conn.execute(
            "UPDATE albums SET rating = ?, rated_at = ? WHERE path = ?",
            (int(value), rated_at, path),
        )
        return "rated"
    print(
        f"error: unexpected rating value {value!r} for path {path!r}", file=sys.stderr
    )
    sys.exit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv_path", help="Path to all_albums_export.csv")
    parser.add_argument("db_path", help="Path to albums.db")
    args = parser.parse_args()

    rows = read_rows(args.csv_path)
    conn = sqlite3.connect(args.db_path)
    conn.execute("PRAGMA foreign_keys = ON")

    check_all_paths_exist(conn, rows)

    rated_at = now_rfc3339()
    counts = {"blank": 0, "rated": 0, "needs_relisten": 0, "classical": 0}
    seen_paths: set[str] = set()
    duplicates: set[str] = set()

    for row in rows:
        path = row["file"].rstrip("/")
        if path in seen_paths:
            duplicates.add(path)
        seen_paths.add(path)
        outcome = apply_rating(conn, path, row["rating"], rated_at)
        counts[outcome] += 1

    conn.commit()
    conn.close()

    print(f"processed {len(rows)} rows from {args.csv_path}")
    print(f"  rated:          {counts['rated']}")
    print(f"  is_classical:   {counts['classical']}")
    print(f"  needs_relisten: {counts['needs_relisten']}")
    print(f"  blank (no-op):  {counts['blank']}")
    if duplicates:
        print(f"  duplicate paths (last row wins): {sorted(duplicates)}")


if __name__ == "__main__":
    main()
