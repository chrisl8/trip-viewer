#!/usr/bin/env python3
"""
Restore timelapse_jobs rows that vanished when a merge attempt's
phase 1 (row deletions) committed before phase 2 (segment/trip
rewrites) failed and rolled back.

Symptom: a trip in the current DB has zero `timelapse_jobs` rows,
but `<archive>/Timelapses/{trip_id}_{tier}_{channel}.mp4` files for
that trip still exist on disk AND a pre-incident DB backup
(restored from Borg) has the rows.

The script restores those rows verbatim — preserving speed_curve_json,
padded_count, encoder_used, ffmpeg_version, completed_at_ms — but
rewrites `output_path` to the canonical archive-relative form so it
matches the current naming convention. Files on disk are not
touched; the row just points at what's already there.

Usage
-----
    python3 scripts/restore-missing-timelapse-jobs.py \\
        --old-db '/path/to/borg-restored/.tripviewer/tripviewer.db' \\
        --new-db '/mnt/storage/Wolfbox Dashcam/.tripviewer/tripviewer.db' \\
        --new-root '/mnt/storage/Wolfbox Dashcam' \\
        [--apply]

Dry run by default. `--apply` performs the inserts.

Safety
------
- Old DB is opened read-only.
- Only restores rows for trips that currently have ZERO
  timelapse_jobs rows (a clean "lost in phase-2 rollback" shape).
  Trips with partial rows are left alone — they may be mid-encode or
  have been intentionally modified.
- Only restores rows whose on-disk file actually exists at the
  canonical location, so we never invent a row for a missing file.
"""

import argparse
import os
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class RestorePlan:
    trip_id: str
    tier: str
    channel: str
    file_basename: str
    status: str
    speed_curve_json: str | None
    padded_count: int
    encoder_used: str | None
    ffmpeg_version: str | None
    output_size_bytes: int | None


def find_targets(new_db: Path) -> list[str]:
    """Trips in the current DB that have zero timelapse_jobs rows.
    These are the candidates for restoration."""
    conn = sqlite3.connect(f"file:{new_db}?mode=ro", uri=True)
    try:
        rows = conn.execute(
            """
            SELECT t.id FROM trips t
            WHERE NOT EXISTS (
                SELECT 1 FROM timelapse_jobs j WHERE j.trip_id = t.id
            )
            ORDER BY t.start_time_ms
            """
        ).fetchall()
        return [r[0] for r in rows]
    finally:
        conn.close()


def collect_plans(
    old_db: Path,
    new_root: Path,
    candidate_trips: list[str],
) -> tuple[list[RestorePlan], list[str]]:
    """For each candidate trip, look up all old rows and produce a
    RestorePlan per (tier, channel) whose on-disk file exists at the
    canonical location. Returns (plans, warnings)."""
    if not candidate_trips:
        return ([], [])
    conn = sqlite3.connect(f"file:{old_db}?mode=ro", uri=True)
    warnings: list[str] = []
    plans: list[RestorePlan] = []
    try:
        timelapses_dir = new_root / "Timelapses"
        placeholders = ",".join("?" * len(candidate_trips))
        rows = conn.execute(
            f"""
            SELECT trip_id, tier, channel, status, speed_curve_json,
                   padded_count, encoder_used, ffmpeg_version,
                   output_size_bytes, output_path
            FROM timelapse_jobs
            WHERE trip_id IN ({placeholders})
            """,
            candidate_trips,
        ).fetchall()
        for (
            trip_id,
            tier,
            channel,
            status,
            curve,
            padded,
            encoder,
            version,
            size,
            output_path,
        ) in rows:
            # The old DB's `output_path` may reference a pre-rewrite
            # filename (e.g. `{old_trip_id}_8x_F.mp4`) because trip
            # IDs were changed by `rebuild_for_cross_os` without
            # renaming files in lockstep. We deliberately *don't*
            # check that the old basename matches the canonical new
            # name — by the time this restore runs, an earlier
            # recovery pass (`recover-timelapses-from-old-db.py`) has
            # already renamed files to the new convention. The only
            # thing that matters here is that the file at the
            # canonical name exists on disk; the old metadata is
            # still correct content-wise (same encoder, same speed
            # curve, same source segments).
            canonical = f"{trip_id}_{tier}_{channel}.mp4"
            file_path = timelapses_dir / canonical
            if not file_path.exists():
                warnings.append(
                    f"file missing for {trip_id} {tier}/{channel}: "
                    f"{file_path.name} (old_path={output_path!r})"
                )
                continue
            plans.append(
                RestorePlan(
                    trip_id=trip_id,
                    tier=tier,
                    channel=channel,
                    file_basename=canonical,
                    status=status,
                    speed_curve_json=curve,
                    padded_count=padded if padded is not None else 0,
                    encoder_used=encoder,
                    ffmpeg_version=version,
                    output_size_bytes=size,
                )
            )
        return (plans, warnings)
    finally:
        conn.close()


def apply_plans(new_db: Path, new_root: Path, plans: list[RestorePlan]) -> int:
    """Insert the restored rows. We always re-stat the on-disk file
    so output_size_bytes reflects current reality."""
    conn = sqlite3.connect(new_db)
    inserted = 0
    try:
        with conn:
            now_ms = int(__import__("time").time() * 1000)
            for p in plans:
                # Stat the file under the supplied --new-root so the
                # stored size matches the file the row points at, even
                # if the old DB's recorded size has drifted.
                file_path = new_root / "Timelapses" / p.file_basename
                actual_size = (
                    file_path.stat().st_size
                    if file_path.exists()
                    else p.output_size_bytes
                )
                conn.execute(
                    """
                    INSERT INTO timelapse_jobs
                        (trip_id, tier, channel, status, output_path,
                         ffmpeg_version, encoder_used, padded_count,
                         speed_curve_json, created_at_ms, completed_at_ms,
                         output_size_bytes)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                    """,
                    (
                        p.trip_id,
                        p.tier,
                        p.channel,
                        p.status,
                        f"Timelapses/{p.file_basename}",
                        p.ffmpeg_version,
                        p.encoder_used,
                        p.padded_count,
                        p.speed_curve_json,
                        now_ms,
                        now_ms,
                        actual_size,
                    ),
                )
                inserted += 1
        return inserted
    finally:
        conn.close()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Restore timelapse_jobs rows from a pre-incident DB backup.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--old-db", required=True, type=Path)
    parser.add_argument("--new-db", required=True, type=Path)
    parser.add_argument("--new-root", required=True, type=Path)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()

    for label, path in (("old DB", args.old_db), ("new DB", args.new_db)):
        if not path.is_file():
            print(f"[fatal] {label} not found: {path}", file=sys.stderr)
            return 1
    if not args.new_root.is_dir():
        print(
            f"[fatal] new root is not a directory: {args.new_root}",
            file=sys.stderr,
        )
        return 1

    candidates = find_targets(args.new_db)
    print(
        f"Trips in current DB with zero timelapse_jobs rows: {len(candidates)}",
        file=sys.stderr,
    )

    plans, warnings = collect_plans(args.old_db, args.new_root, candidates)

    if warnings:
        print("", file=sys.stderr)
        print(f"Warnings ({len(warnings)}):", file=sys.stderr)
        for w in warnings[:20]:
            print(f"  • {w}", file=sys.stderr)
        if len(warnings) > 20:
            print(f"  … and {len(warnings) - 20} more", file=sys.stderr)

    print("", file=sys.stderr)
    print("Restore plan:", file=sys.stderr)
    print(f"  {len(plans)} rows to insert", file=sys.stderr)
    by_trip: dict[str, int] = {}
    for p in plans:
        by_trip[p.trip_id] = by_trip.get(p.trip_id, 0) + 1
    for tid, n in sorted(by_trip.items()):
        print(f"    {tid}: {n} rows", file=sys.stderr)

    if not args.apply:
        print("", file=sys.stderr)
        print(
            "Dry run complete. Re-run with --apply to insert the rows.",
            file=sys.stderr,
        )
        return 0

    if not plans:
        print("Nothing to apply.", file=sys.stderr)
        return 0

    print("", file=sys.stderr)
    n = apply_plans(args.new_db, args.new_root, plans)
    print(f"Done. Inserted {n} row(s).", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
