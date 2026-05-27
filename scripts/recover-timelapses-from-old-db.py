#!/usr/bin/env python3
"""
Recover orphan timelapse files by reading a pre-migration-0013 DB
backup and renaming on-disk files to match the current DB.

Background
----------
A previous version of the path-normalization migration (0013) rewrote
`timelapse_jobs.output_path` using the row's CURRENT trip_id column.
For archives whose trip_ids had been remapped by an earlier
`rebuild_for_cross_os` pass — without renaming the on-disk MP4 files
in lockstep — that destroyed the only column that still recorded the
on-disk filename. The DB row now points at a file that doesn't exist;
the actual file sits on disk under the trip_id that the encoder used
at write time.

This script recovers the mapping by reading a pre-0013 DB backup
where `timelapse_jobs.output_path` still holds the original absolute
path — which has the OLD on-disk trip_id sitting plainly in its
basename. We don't need to recompute any UUIDs: the old DB has the
filename directly.

Algorithm
---------
1. Open the old DB read-only. Build a `(trip_id, tier, channel) ->
   old_filename` map by extracting basenames from every non-null
   `output_path`. The trip_id column is the NEW (post-`rebuild_for_
   cross_os`) value — `rebuild_for_cross_os` cascades the rewrite
   through timelapse_jobs.trip_id but never touches output_path —
   so this map keys cleanly against the current DB.

2. Open the current DB. For every `failed` row, look up its
   (trip_id, tier, channel) in the map. If a candidate file exists
   under `<archive>/Timelapses/{old_filename}`, plan a rename to
   `{trip_id}_{tier}_{channel}.mp4` so the current row's
   `output_path` resolves correctly.

3. Flag each plan as `archive-only` if the trip has no live segments
   in the current DB — those are the ones that CANNOT be re-encoded
   and are the irreplaceable population this script saves.

Usage
-----
    python3 scripts/recover-timelapses-from-old-db.py \\
        --old-db /path/to/restored/.tripviewer/tripviewer.db \\
        --new-db '/mnt/storage/Wolfbox Dashcam/.tripviewer/tripviewer.db' \\
        --new-root '/mnt/storage/Wolfbox Dashcam' \\
        [--apply]

By default this runs a DRY RUN. Pass `--apply` to perform renames
and flip the matching DB rows from `failed` back to `done`.

Safety
------
- Old DB opened read-only.
- Default is dry run.
- Never overwrites a file: if the target name already exists the
  rename is skipped and reported.
- Renames + DB updates run one row at a time so a single filesystem
  error doesn't abort the rest of the recovery.
"""

import argparse
import os
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path


def load_old_filename_map(old_db: Path) -> dict[tuple[str, str, str], str]:
    """Walk every old `timelapse_jobs` row with a non-null output_path
    and extract the basename. Returns (trip_id, tier, channel) ->
    basename. The trip_id column in the old DB is already the
    post-`rebuild_for_cross_os` value (cascade ran), so it keys
    against the current DB without any further translation."""
    uri = f"file:{old_db}?mode=ro"
    conn = sqlite3.connect(uri, uri=True)
    try:
        rows = conn.execute(
            """
            SELECT trip_id, tier, channel, output_path
            FROM timelapse_jobs
            WHERE output_path IS NOT NULL AND output_path != ''
            """
        ).fetchall()
        out: dict[tuple[str, str, str], str] = {}
        for trip_id, tier, channel, output_path in rows:
            # Accept either '/' or '\' separators in the stored string —
            # the OLD DB might have been written on a different OS or
            # with normalized separators. Basename is the last segment
            # either way.
            normalized = output_path.replace("\\", "/")
            basename = normalized.rsplit("/", 1)[-1]
            if not basename:
                continue
            out[(trip_id, tier, channel)] = basename
        return out
    finally:
        conn.close()


@dataclass
class RenamePlan:
    new_trip_id: str
    tier: str
    channel: str
    src_path: Path
    dst_path: Path
    archive_only: bool


def plan_renames(
    new_db: Path,
    new_root: Path,
    old_filenames: dict[tuple[str, str, str], str],
) -> tuple[list[RenamePlan], list[str], dict[str, int]]:
    """For every `failed` row in the current DB, find the matching old
    filename and check if it exists on disk. Returns (plans, warnings,
    stats)."""
    conn = sqlite3.connect(f"file:{new_db}?mode=ro", uri=True)
    plans: list[RenamePlan] = []
    warnings: list[str] = []
    stats = {
        "failed_rows_total": 0,
        "no_old_mapping": 0,
        "old_file_missing": 0,
        "target_exists": 0,
        "already_correct_name": 0,
    }
    try:
        failed = conn.execute(
            "SELECT trip_id, tier, channel FROM timelapse_jobs WHERE status = 'failed'"
        ).fetchall()
        stats["failed_rows_total"] = len(failed)

        # Trips with no real segments in the current DB — the ones
        # for which re-encoding is impossible. The script's primary
        # value is recovering these.
        archive_only_ids: set[str] = set()
        rows = conn.execute(
            """
            SELECT id FROM trips
            WHERE NOT EXISTS (
                SELECT 1 FROM segments s
                WHERE s.trip_id = trips.id
                AND s.is_tombstone = 0
                AND s.master_path != ''
            )
            """
        ).fetchall()
        for (tid,) in rows:
            archive_only_ids.add(tid)

        timelapses_dir = new_root / "Timelapses"
        for new_trip_id, tier, channel in failed:
            key = (new_trip_id, tier, channel)
            old_basename = old_filenames.get(key)
            if old_basename is None:
                stats["no_old_mapping"] += 1
                continue
            new_basename = f"{new_trip_id}_{tier}_{channel}.mp4"
            if old_basename == new_basename:
                # The old DB already had the new naming for this row
                # (i.e. this row was first encoded AFTER the UUID rewrite
                # — nothing to rename). The fact that the current row is
                # 'failed' means the file is missing under the new name
                # too. Nothing this script can do.
                stats["already_correct_name"] += 1
                continue
            src = timelapses_dir / old_basename
            dst = timelapses_dir / new_basename
            if not src.exists():
                stats["old_file_missing"] += 1
                warnings.append(
                    f"old file missing: {old_basename} "
                    f"(was expected for {new_trip_id} {tier}/{channel})"
                )
                continue
            if dst.exists():
                stats["target_exists"] += 1
                warnings.append(
                    f"target already exists, skipping: {new_basename}"
                )
                continue
            plans.append(
                RenamePlan(
                    new_trip_id=new_trip_id,
                    tier=tier,
                    channel=channel,
                    src_path=src,
                    dst_path=dst,
                    archive_only=new_trip_id in archive_only_ids,
                )
            )
        return plans, warnings, stats
    finally:
        conn.close()


def apply_plans(new_db: Path, plans: list[RenamePlan]) -> tuple[int, int]:
    """Perform the renames and flip the corresponding DB rows from
    `failed` back to `done`. Returns (renamed, db_rows_updated)."""
    renamed = 0
    db_updated = 0
    conn = sqlite3.connect(new_db)
    try:
        for p in plans:
            try:
                os.rename(p.src_path, p.dst_path)
            except OSError as e:
                print(
                    f"[error] rename {p.src_path} -> {p.dst_path} failed: {e}",
                    file=sys.stderr,
                )
                continue
            renamed += 1
            size = p.dst_path.stat().st_size
            with conn:
                conn.execute(
                    """
                    UPDATE timelapse_jobs
                    SET status = 'done',
                        output_path = ?,
                        error_message = NULL,
                        output_size_bytes = ?
                    WHERE trip_id = ? AND tier = ? AND channel = ?
                    """,
                    (
                        f"Timelapses/{p.dst_path.name}",
                        size,
                        p.new_trip_id,
                        p.tier,
                        p.channel,
                    ),
                )
                db_updated += 1
        return renamed, db_updated
    finally:
        conn.close()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Recover orphan timelapse files using a pre-rewrite DB backup.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--old-db", required=True, type=Path,
        help="Path to a pre-migration-0013 tripviewer.db backup.",
    )
    parser.add_argument(
        "--new-db", required=True, type=Path,
        help="Path to the current tripviewer.db (modified with --apply).",
    )
    parser.add_argument(
        "--new-root", required=True, type=Path,
        help="Current archive root. Files live under <root>/Timelapses/.",
    )
    parser.add_argument(
        "--apply", action="store_true",
        help="Perform renames + DB updates. Default is a dry run.",
    )
    args = parser.parse_args()

    if not args.old_db.is_file():
        print(f"[fatal] old DB not found: {args.old_db}", file=sys.stderr)
        return 1
    if not args.new_db.is_file():
        print(f"[fatal] new DB not found: {args.new_db}", file=sys.stderr)
        return 1
    if not args.new_root.is_dir():
        print(f"[fatal] new root is not a directory: {args.new_root}", file=sys.stderr)
        return 1

    print(f"Loading old DB: {args.old_db}", file=sys.stderr)
    old_filenames = load_old_filename_map(args.old_db)
    print(
        f"  {len(old_filenames)} (trip_id, tier, channel) -> filename entries",
        file=sys.stderr,
    )

    # Quick sanity diagnostic: detect whether the old DB looks pre-0013
    # (basenames contain UUIDs different from the row's trip_id) or
    # post-0013 (basenames match the trip_id, so there's no recovery
    # value here).
    distinct_from_trip = 0
    matches_trip = 0
    for (tid, _t, _c), basename in old_filenames.items():
        if basename.startswith(tid):
            matches_trip += 1
        else:
            distinct_from_trip += 1
    if distinct_from_trip == 0 and matches_trip > 0:
        print(
            "[warn] every basename in the old DB matches its row's trip_id — "
            "the old DB was already touched by migration 0013, so there's "
            "nothing the script can recover from it. Restore an OLDER backup.",
            file=sys.stderr,
        )
    else:
        print(
            f"  basenames containing pre-rewrite trip_ids: {distinct_from_trip}",
            file=sys.stderr,
        )
        print(
            f"  basenames already matching row.trip_id: {matches_trip}",
            file=sys.stderr,
        )

    plans, warnings, stats = plan_renames(args.new_db, args.new_root, old_filenames)

    print("", file=sys.stderr)
    print("Match summary against the current DB:", file=sys.stderr)
    print(f"  failed rows total:           {stats['failed_rows_total']}", file=sys.stderr)
    print(f"  no entry in old DB:          {stats['no_old_mapping']}", file=sys.stderr)
    print(f"  old file already at new name: {stats['already_correct_name']}", file=sys.stderr)
    print(f"  old file missing on disk:    {stats['old_file_missing']}", file=sys.stderr)
    print(f"  target name already taken:   {stats['target_exists']}", file=sys.stderr)
    print(f"  recoverable:                 {len(plans)}", file=sys.stderr)

    if warnings:
        print("", file=sys.stderr)
        print(f"Warnings ({len(warnings)}):", file=sys.stderr)
        for w in warnings[:15]:
            print(f"  • {w}", file=sys.stderr)
        if len(warnings) > 15:
            print(f"  … and {len(warnings) - 15} more", file=sys.stderr)

    archive_only = [p for p in plans if p.archive_only]
    encodable = [p for p in plans if not p.archive_only]

    print("", file=sys.stderr)
    print("Recovery plan:", file=sys.stderr)
    print(f"  total renames:  {len(plans)}", file=sys.stderr)
    print(f"  archive-only:   {len(archive_only)}  (originals deleted — IRREPLACEABLE)", file=sys.stderr)
    print(f"  re-encodable:   {len(encodable)}  (could also be recovered by a fresh encode pass)", file=sys.stderr)

    sample = plans[:5]
    if sample:
        print("", file=sys.stderr)
        print("Sample renames:", file=sys.stderr)
        for p in sample:
            tag = "  [archive-only]" if p.archive_only else ""
            print(f"  {p.src_path.name}", file=sys.stderr)
            print(f"   -> {p.dst_path.name}{tag}", file=sys.stderr)

    if not args.apply:
        print("", file=sys.stderr)
        print("Dry run complete. Re-run with --apply to perform the renames.", file=sys.stderr)
        return 0

    if not plans:
        print("Nothing to apply.", file=sys.stderr)
        return 0

    print("", file=sys.stderr)
    print(f"Applying {len(plans)} rename(s) + DB update(s)…", file=sys.stderr)
    renamed, db_updated = apply_plans(args.new_db, plans)
    print(
        f"Done. renamed={renamed} db_rows_updated={db_updated}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
